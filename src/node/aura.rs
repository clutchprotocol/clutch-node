use serde::Deserialize;

use crate::node::consensus::Consensus;

use super::{blocks::block::Block, time_utils::get_current_timespan};

/// An authority set the chain has moved on from: the set for every block up to and including
/// `until_block`. Configured as `[[previous_authorities]]` tables, oldest first.
///
/// The author check reads the set that was valid at the BLOCK's height, not only today's. Without
/// these, rotating a key out leaves the blocks it signed failing the check, so a node that is
/// behind the rotation, or new, can never sync past it.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PastAuthoritySet {
    /// The last block this set authored.
    pub until_block: u64,
    pub authorities: Vec<String>,
}

#[derive(Debug)]
pub struct Aura {
    pub authorities: Vec<String>, // List of validators
    pub step_duration: u64,       // Duration of each step in seconds
    /// Sets the chain had before `authorities`, oldest first. Empty on a chain whose set never
    /// changed: every block is then checked against `authorities`, as it always was.
    pub past_sets: Vec<PastAuthoritySet>,
}

impl Aura {
    pub fn new(authorities: Vec<String>, step_duration: u64) -> Self {
        Self {
            authorities,
            step_duration,
            past_sets: Vec::new(),
        }
    }

    /// Slot length for a set of this size: one full round of the set takes 60 seconds.
    pub fn step_duration_for(set_size: usize) -> u64 {
        60 / set_size as u64
    }

    /// The authorities and slot length that governed block `index`: the oldest past set that still
    /// reaches it, else the current one.
    fn set_at(&self, index: u64) -> (&[String], u64) {
        match self.past_sets.iter().find(|set| index <= set.until_block) {
            Some(set) => (
                set.authorities.as_slice(),
                Self::step_duration_for(set.authorities.len()),
            ),
            None => (self.authorities.as_slice(), self.step_duration),
        }
    }

    // Determine the slot number based on a given timestamp
    fn slot_at_time(&self, timestamp: u64) -> u64 {
        timestamp / self.step_duration
    }

    // Determine the current slot number based on the system time
    fn current_slot(&self) -> u64 {
        let current_timespan = get_current_timespan();
        self.slot_at_time(current_timespan)
    }

    // Determine the author based on a given slot number
    fn author_at_slot(&self, slot: u64) -> &String {
        &self.authorities[slot as usize % self.authorities.len()]
    }
}

impl Consensus for Aura {
    fn current_author(&self) -> &String {
        let current_slot = self.current_slot();
        self.author_at_slot(current_slot)
    }

    fn verify_block_author(&self, block: &Block) -> Result<(), String> {
        // The set that governed THIS block. One signed before a rotation is checked against the set
        // that signed it, so a node that is behind the rotation can still sync across it. With no
        // past sets this is `authorities` and `step_duration`, exactly as before.
        let (authorities, step_duration) = self.set_at(block.index as u64);
        let block_slot = block.timestamp / step_duration;

        // Bind the block's slot to real time. Without this, an authority can pick any
        // future timestamp that maps to a slot it owns and author out of turn. Only the
        // future is bounded (1 slot of clock-skew tolerance); past slots must stay valid
        // so historical blocks still import during sync.
        let current_slot = get_current_timespan() / step_duration;
        if block_slot > current_slot.saturating_add(1) {
            return Err(format!(
                "Block slot {} is too far ahead of the current slot {}",
                block_slot, current_slot
            ));
        }

        let expected_author = &authorities[block_slot as usize % authorities.len()];

        if &block.author == expected_author {
            Ok(())
        } else {
            Err(format!(
                "Block author verification failed: expected author {}, but found {}",
                expected_author, block.author
            ))
        }
    }

    fn block_is_in_current_slot(&self, block: &Block) -> bool {
        self.slot_at_time(block.timestamp) == self.current_slot()
    }
}

#[cfg(test)]
mod tests {
    use tracing::error;

    use super::*;

    #[test]
    fn test_current_author() {
        let aura = Aura::new(vec!["node_1".to_string(), "node_2".to_string()], 60);
        let slot = aura.current_slot() as usize;
        let expected_author = &aura.authorities[slot % aura.authorities.len()];
        error!(
            "current slot: {:?}, expected_author: {:?}",
            slot, expected_author
        );
        assert_eq!(aura.current_author(), expected_author);
    }

    #[test]
    fn block_is_in_current_slot_distinguishes_now_from_long_ago() {
        let aura = Aura::new(vec!["0xauthority".to_string()], 60);

        // A block authored right now is in the current slot — this is what stops the 1-second
        // authoring loop from emitting an empty block on every tick.
        let now_block = Block::new_block(1, "0".to_string(), vec![]);
        assert!(aura.block_is_in_current_slot(&now_block));

        // A block from epoch 0 is not, so the next slot is free to carry a fresh heartbeat.
        let mut old_block = Block::new_block(1, "0".to_string(), vec![]);
        old_block.timestamp = 0;
        assert!(!aura.block_is_in_current_slot(&old_block));
    }

    #[test]
    fn rejects_block_slot_far_in_future() {
        let aura = Aura::new(vec!["node_1".to_string(), "node_2".to_string()], 20);
        let mut block = Block::new_block(1, "0".to_string(), vec![]);
        block.timestamp = u64::MAX; // slot far beyond current_slot + 1
        block.author = "node_1".to_string();
        assert!(aura.verify_block_author(&block).is_err());
    }

    fn authored(index: usize, timestamp: u64, author: &str) -> Block {
        let mut block = Block::new_block(index, "0".to_string(), vec![]);
        block.timestamp = timestamp;
        block.author = author.to_string();
        block
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    /// The stage rotation in miniature: three authorities replaced by three others after block 100.
    /// Slots are 20 seconds, so the slot of timestamp T is T / 20 and its author is index slot % 3.
    fn rotated() -> Aura {
        let mut aura = Aura::new(names(&["new_1", "new_2", "new_3"]), 20);
        aura.past_sets = vec![PastAuthoritySet {
            until_block: 100,
            authorities: names(&["old_1", "old_2", "old_3"]),
        }];
        aura
    }

    #[test]
    fn an_old_block_is_checked_against_the_set_that_authored_it() {
        let aura = rotated();
        // Timestamp 80 is slot 4, which is index 1 of the set: old_2 then, new_2 now.
        assert!(aura.verify_block_author(&authored(50, 80, "old_2")).is_ok());
        // The new key may not sign history it was not an authority for.
        assert!(aura.verify_block_author(&authored(50, 80, "new_2")).is_err());
    }

    #[test]
    fn without_the_old_set_a_rotated_out_authors_block_is_refused() {
        // What a node did before past sets existed: every block against today's set. This is the
        // failure that left stage node1 stuck behind the rotation of 2026-10-29.
        let aura = Aura::new(names(&["new_1", "new_2", "new_3"]), 20);
        let err = aura
            .verify_block_author(&authored(50, 80, "old_2"))
            .expect_err("the old key is not in today's set");
        assert!(err.contains("expected author new_2"), "got: {err}");
    }

    #[test]
    fn the_last_block_of_an_old_set_is_inclusive_and_the_next_one_is_not() {
        let aura = rotated();
        // Block 100 at timestamp 100 is slot 5, index 2: old_3. Block 101 at 120 is slot 6, index 0.
        assert!(aura.verify_block_author(&authored(100, 100, "old_3")).is_ok());
        assert!(aura.verify_block_author(&authored(100, 100, "new_3")).is_err());
        assert!(aura.verify_block_author(&authored(101, 120, "new_1")).is_ok());
        // An old key can no longer author at the head. That is what the rotation is for.
        assert!(aura.verify_block_author(&authored(101, 120, "old_1")).is_err());
    }

    #[test]
    fn each_of_several_past_sets_covers_its_own_range() {
        let mut aura = Aura::new(names(&["c_1", "c_2", "c_3"]), 20);
        aura.past_sets = vec![
            PastAuthoritySet {
                until_block: 10,
                authorities: names(&["a_1", "a_2", "a_3"]),
            },
            PastAuthoritySet {
                until_block: 20,
                authorities: names(&["b_1", "b_2", "b_3"]),
            },
        ];
        // Timestamp 60 is slot 3, which is index 0 of any set of three.
        assert!(aura.verify_block_author(&authored(5, 60, "a_1")).is_ok());
        assert!(aura.verify_block_author(&authored(15, 60, "b_1")).is_ok());
        assert!(aura.verify_block_author(&authored(25, 60, "c_1")).is_ok());
        assert!(aura.verify_block_author(&authored(15, 60, "a_1")).is_err());
        assert!(aura.verify_block_author(&authored(25, 60, "b_1")).is_err());
    }

    #[test]
    fn a_past_set_of_another_size_keeps_its_own_slot_length() {
        assert_eq!(Aura::step_duration_for(2), 30);
        assert_eq!(Aura::step_duration_for(3), 20);

        let mut aura = Aura::new(names(&["n_1", "n_2", "n_3"]), 20);
        aura.past_sets = vec![PastAuthoritySet {
            until_block: 10,
            authorities: names(&["p_1", "p_2"]),
        }];
        // Two authorities own 30-second slots: timestamp 90 is slot 3, index 1. Under today's 20-second
        // slots it would be slot 4 and a different author.
        assert!(aura.verify_block_author(&authored(5, 90, "p_2")).is_ok());
        assert!(aura.verify_block_author(&authored(5, 90, "p_1")).is_err());
    }

    #[test]
    fn a_block_of_an_old_set_is_still_bound_to_real_time() {
        let aura = rotated();
        // The bound on the future applies to old sets too, so a peer cannot hand a node an
        // out-of-turn block by claiming a low height.
        assert!(aura.verify_block_author(&authored(50, u64::MAX, "old_1")).is_err());
    }
}
