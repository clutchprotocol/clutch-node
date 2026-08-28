//! What our peers say the tip is.
//!
//! A node that is behind does not know it in any way it can report. `get_chain_info` answers with
//! whatever block this node has reached, and a caller has no way to tell "this is the tip" from
//! "this is how far I have got". On stage that cost a full day: the treasury read a node mid-sync,
//! saw a supply frozen near genesis, judged its reserve against it, and submitted mints into it —
//! every reading internally consistent, nothing anywhere saying "behind".
//!
//! The information already existed. Every handshake carries the peer's `latest_block_index`, and
//! the sync path compares it against ours to decide whether to request blocks. It was simply
//! discarded afterwards. This keeps it so the RPC can report it.
//!
//! # Why the maximum, and why it never decreases
//!
//! Highest-seen, not latest-seen: a peer that is itself behind must not lower our idea of the tip,
//! or two lagging nodes would agree they are both fine. The value therefore only rises, which fails
//! SAFE — the worst case is a node reporting `is_syncing` while it is actually current, which is
//! visible and self-correcting as soon as it reaches that height. The opposite error, a syncing
//! node reporting itself as the tip, is the one that already cost a day.
//!
//! Genesis hashes are checked before a handshake reaches here, so a peer on an unrelated chain
//! cannot inflate this.

use std::sync::atomic::{AtomicUsize, Ordering};

/// Highest `latest_block_index` any peer has reported since this process started.
static BEST_PEER_BLOCK_INDEX: AtomicUsize = AtomicUsize::new(0);

/// Record a peer's height from its handshake. Keeps the maximum.
pub fn record_peer_height(height: usize) {
    BEST_PEER_BLOCK_INDEX.fetch_max(height, Ordering::Relaxed);
}

/// The highest height any peer has claimed. Zero means no peer has been heard from — which is not
/// the same as "the tip is zero", and `is_syncing` treats it accordingly.
pub fn best_peer_height() -> usize {
    BEST_PEER_BLOCK_INDEX.load(Ordering::Relaxed)
}

/// How far behind the best peer this node is. Saturating: being ahead is not negative.
pub fn blocks_behind(our_height: usize) -> usize {
    best_peer_height().saturating_sub(our_height)
}

/// Is this node behind its peers by enough that its answers should not be trusted as the tip?
///
/// Returns false when no peer has been heard from. A single isolated node is not "syncing", it is
/// alone, and reporting a permanent `is_syncing` for a one-node development stack would make the
/// flag useless in exactly the setup where people first meet it.
pub fn is_syncing(our_height: usize) -> bool {
    let best = best_peer_height();
    best > 0 && our_height + SYNC_TOLERANCE_BLOCKS < best
}

/// Slack before a node calls itself behind. Blocks propagate; being one or two back is normal and
/// momentary, and flagging that would train callers to ignore the flag.
pub const SYNC_TOLERANCE_BLOCKS: usize = 5;

#[cfg(test)]
mod tests {
    use super::*;

    // These tests share one process-global, so they run as a single test to keep them ordered.
    // Splitting them would let cargo's threads interleave the writes.
    #[test]
    fn tracks_the_highest_peer_and_never_lowers_it() {
        assert_eq!(best_peer_height(), 0, "starts with no peers heard from");
        assert!(!is_syncing(0), "a node with no peers is alone, not syncing");
        assert!(!is_syncing(12_345), "still not syncing with no peer to compare against");

        record_peer_height(1_000);
        assert_eq!(best_peer_height(), 1_000);
        assert!(is_syncing(100), "100 against a peer at 1000 is behind");
        assert_eq!(blocks_behind(100), 900);

        // A peer that is itself behind must not lower our idea of the tip, or two lagging nodes
        // would agree they are both fine.
        record_peer_height(10);
        assert_eq!(best_peer_height(), 1_000, "a lagging peer must not lower the tip");

        record_peer_height(2_000);
        assert_eq!(best_peer_height(), 2_000);

        // Caught up.
        assert!(!is_syncing(2_000));
        assert_eq!(blocks_behind(2_000), 0);

        // Within tolerance: normal propagation, not a sync.
        assert!(!is_syncing(2_000 - SYNC_TOLERANCE_BLOCKS));
        assert!(is_syncing(2_000 - SYNC_TOLERANCE_BLOCKS - 1));

        // Ahead of every peer is zero, not an underflow.
        assert_eq!(blocks_behind(5_000), 0);
        assert!(!is_syncing(5_000));
    }
}
