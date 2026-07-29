//! An idle chain must keep producing blocks.
//!
//! `Transaction::validate_transactions` used to reject an empty transaction list outright, so a
//! chain with nothing being submitted stopped dead at genesis. That was not cosmetic: confirmation
//! depth is counted in blocks, so anything waiting for `confirmations` blocks of depth needed
//! *later* blocks to exist — and later blocks needed more transactions. A single Mint on an
//! otherwise-quiet chain therefore never reached confirmed depth and never got credited, stalling
//! the treasury's entire credit path.
//!
//! Emptiness is not a state question, so it does not belong in a state validator. Whether an empty
//! block is *wanted* is an authoring decision, and lives in `Blockchain::author_new_block`.

use clutch_node::node::{
    blockchain::Blockchain,
    blocks::block::Block,
    transactions::chain_init::ChainInit,
};
use serial_test::serial;

const BLOCKCHAIN_NAME: &str = "clutch-node-empty-block-test";
const FAUCET_ADDRESS: &str = "0xdeb4cfb63db134698e1879ea24904df074726cc0";
const AUTHOR_PUBLIC_KEY: &str = "0x9b6e8afff8329743cac73dbef83ca3cbf9a74c20";
const AUTHOR_SECRET_KEY: &str = "0883ddd3d07303b87c954b0c9383f7b78f45e002520fc03a8adc80595dbf6509";

fn ci() -> ChainInit {
    ChainInit {
        chain_id: 2077,
        is_testnet: true,
        tx_fee: 1000,
        ride_request_referrer_fee_bps: 2,
        ride_offer_referrer_fee_bps: 2,
        mint_authority: AUTHOR_PUBLIC_KEY.to_string(),
        faucet_address: FAUCET_ADDRESS.to_string(),
        faucet_allocation: 1_000_000_000_000_000,
    }
}

fn chain(name: &str) -> Blockchain {
    Blockchain::new(
        name.to_string(),
        AUTHOR_PUBLIC_KEY.to_string(),
        AUTHOR_SECRET_KEY.to_string(),
        true,
        vec![AUTHOR_PUBLIC_KEY.to_string()],
        ci(),
    )
}

/// The defect fix, proven end to end: a block carrying NO transactions imports and advances the
/// chain. Goes through the real `import_block` (author check, block validation,
/// `validate_transactions`, `add_block_to_chain`) rather than calling the validator directly, so
/// the whole path is covered — an empty block used to be rejected partway through.
#[test]
#[serial]
fn an_empty_block_imports_and_advances_the_chain() {
    let mut blockchain = chain(BLOCKCHAIN_NAME);

    let genesis = blockchain
        .get_latest_block()
        .expect("db read failed")
        .expect("genesis must exist");

    let mut empty_block = Block::new_block(genesis.index + 1, genesis.hash.clone(), vec![]);
    empty_block.sign(&AUTHOR_PUBLIC_KEY.to_string(), &AUTHOR_SECRET_KEY.to_string());

    blockchain
        .import_block(&empty_block)
        .expect("an empty block must be a valid Aura heartbeat, not an error");

    let latest = blockchain
        .get_latest_block()
        .expect("db read failed")
        .expect("latest must exist");
    assert_eq!(latest.index, genesis.index + 1, "the chain must advance on an empty block");
    assert!(latest.transactions.is_empty(), "the heartbeat block carries no transactions");

    // No transactions means no fees, so the author must NOT be credited — an empty block cannot
    // become a source of new CLT.
    let author = blockchain.get_account_state(&AUTHOR_PUBLIC_KEY.to_string());
    assert_eq!(author.balance, 0, "an empty block pays no tx fee and must not mint anything");

    blockchain.shutdown_blockchain();
}

/// The counterpart guard: empty blocks are a heartbeat, not throughput. The authoring loop ticks
/// every second while a slot lasts `step_duration` seconds (60 with a single authority), so a fresh
/// chain whose genesis sits in the current slot must REFUSE to author another empty block —
/// otherwise allowing empty blocks would emit one per second.
#[test]
#[serial]
fn author_refuses_a_second_empty_block_in_the_same_slot() {
    let mut blockchain = chain("clutch-node-empty-block-slot-test");

    // Measured, not assumed to be 0: a previous aborted run can leave this database behind
    // (developer_mode only cleans up on a normal shutdown, which a panicking test skips). The
    // property under test is "the height did not move", which holds from any starting height.
    let before = blockchain
        .get_latest_block()
        .expect("db read failed")
        .expect("a chain must exist")
        .index;

    // The latest block was just written, so it is in the current slot, and the pool is empty.
    let result = blockchain.author_new_block();

    let after = blockchain
        .get_latest_block()
        .expect("db read failed")
        .expect("a chain must exist")
        .index;
    // Assert the height first: it is the property with consequences, and checking it before
    // unwrapping the error means a regression reports "it produced a block" rather than a
    // confusing message about an unexpected Ok.
    assert_eq!(after, before, "no block may be produced when this slot already has one");

    let err = result.expect_err("a second empty block in one slot must be refused");
    assert!(
        err.contains("this slot already has a block"),
        "refusal must be the slot guard, not an unrelated failure: {err}"
    );

    blockchain.shutdown_blockchain();
}
