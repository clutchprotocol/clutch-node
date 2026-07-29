use super::blocks::block::Block;


pub trait Consensus {
    fn current_author(&self) -> &String;
    fn verify_block_author(&self, block: &Block) -> Result<(), String>;
    /// Was `block` authored in the slot that is current right now?
    ///
    /// Needed to bound *empty* (heartbeat) blocks to one per slot. The authoring loop ticks
    /// every second while a slot lasts `step_duration` seconds, so without this an idle chain
    /// would emit a block every second instead of one per slot.
    fn block_is_in_current_slot(&self, block: &Block) -> bool;
}