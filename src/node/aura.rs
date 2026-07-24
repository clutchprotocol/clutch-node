use crate::node::consensus::Consensus;

use super::{blocks::block::Block, time_utils::get_current_timespan};

#[derive(Debug)]
pub struct Aura {
    pub authorities: Vec<String>, // List of validators
    pub step_duration: u64,       // Duration of each step in seconds
}

impl Aura {
    pub fn new(authorities: Vec<String>, step_duration: u64) -> Self {
        Self {
            authorities,
            step_duration,
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
        let block_slot = self.slot_at_time(block.timestamp);

        // Bind the block's slot to real time. Without this, an authority can pick any
        // future timestamp that maps to a slot it owns and author out of turn. Only the
        // future is bounded (1 slot of clock-skew tolerance); past slots must stay valid
        // so historical blocks still import during sync.
        let current_slot = self.current_slot();
        if block_slot > current_slot.saturating_add(1) {
            return Err(format!(
                "Block slot {} is too far ahead of the current slot {}",
                block_slot, current_slot
            ));
        }

        let expected_author = self.author_at_slot(block_slot);

        if &block.author == expected_author {
            Ok(())
        } else {
            Err(format!(
                "Block author verification failed: expected author {}, but found {}",
                expected_author, block.author
            ))
        }
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
    fn rejects_block_slot_far_in_future() {
        let aura = Aura::new(vec!["node_1".to_string(), "node_2".to_string()], 20);
        let mut block = Block::new_block(1, "0".to_string(), vec![]);
        block.timestamp = u64::MAX; // slot far beyond current_slot + 1
        block.author = "node_1".to_string();
        assert!(aura.verify_block_author(&block).is_err());
    }
}
