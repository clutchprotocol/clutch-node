//! Syncing across an authority rotation.
//!
//! The chain ran under one authority, which was then replaced. A node that starts afterwards, or
//! was behind, imports blocks signed by a key that is no longer in its current set. Before past
//! sets existed it refused every one of them and could never catch up: stage node1 sat at block
//! 144,511 for that reason after the rotation of 2026-10-29.

use clutch_node::node::{
    aura::PastAuthoritySet,
    blockchain::Blockchain,
    transactions::{
        chain_init::ChainInit, function_call::FunctionCall, transaction::Transaction,
        transfer::Transfer,
    },
};
use serial_test::serial;

// The public dev keys, as in the other tests.
const OLD_AUTHORITY: &str = "0x9b6e8afff8329743cac73dbef83ca3cbf9a74c20";
const OLD_AUTHORITY_SECRET: &str =
    "0883ddd3d07303b87c954b0c9383f7b78f45e002520fc03a8adc80595dbf6509";
// The authority that replaced it. A node of this set only imports here, so its secret is never used.
const NEW_AUTHORITY: &str = "0x6fc11ba44483201f6e9c5eba6435805bb94ad080";
const FAUCET: &str = "0xdeb4cfb63db134698e1879ea24904df074726cc0";
const FAUCET_SECRET: &str = "d2c446110cfcecbdf05b2be528e72483de5b6f7ef9c7856df2f81f48e9f2748f";
const RECIPIENT: &str = "0x8f19077627cde4848b090c53c83b12956837d5e9";

fn chain_init() -> ChainInit {
    ChainInit {
        chain_id: 2077,
        is_testnet: true,
        tx_fee: 1000,
        ride_request_referrer_fee_bps: 2,
        ride_offer_referrer_fee_bps: 2,
        mint_authority: OLD_AUTHORITY.to_string(),
        faucet_address: FAUCET.to_string(),
        faucet_allocation: 1_000_000_000_000_000,
        mint_cosigners: Vec::new(),
        mint_threshold: 0,
        ride_auto_release_secs: 0,
    }
}

/// A node whose current authority set is the single `authority`.
fn chain(name: &str, authority: &str) -> Blockchain {
    Blockchain::new(
        name.to_string(),
        authority.to_string(),
        OLD_AUTHORITY_SECRET.to_string(),
        true,
        vec![authority.to_string()],
        chain_init(),
    )
}

fn transfer(nonce: u64) -> Transaction {
    let mut tx = Transaction::new_transaction(
        FAUCET.to_string(),
        nonce,
        2077,
        FunctionCall::Transfer(Transfer {
            to: RECIPIENT.to_string(),
            value: 20,
        }),
    );
    tx.sign(FAUCET_SECRET);
    tx
}

#[test]
#[serial]
fn a_node_that_missed_the_rotation_syncs_the_blocks_of_the_old_authority() {
    // The chain as it was before the rotation: OLD_AUTHORITY authors blocks 1 and 2.
    let mut before = chain("clutch-node-rotation-before", OLD_AUTHORITY);
    before.add_transaction_to_pool(&transfer(1)).expect("pool");
    let block_1 = before.author_new_block().expect("block 1");
    before.add_transaction_to_pool(&transfer(2)).expect("pool");
    let block_2 = before.author_new_block().expect("block 2");

    // A node configured after the rotation that says nothing about the old set refuses block 1.
    let mut without = chain("clutch-node-rotation-without", NEW_AUTHORITY);
    let err = without
        .import_block(&block_1)
        .expect_err("today's set alone does not know the old key");
    assert!(err.contains("Block author verification failed"), "got: {err}");

    // The same node, with the old set declared through block 1, imports block 1 ...
    let mut with = chain("clutch-node-rotation-with", NEW_AUTHORITY).with_previous_authorities(vec![
        PastAuthoritySet {
            until_block: 1,
            authorities: vec![OLD_AUTHORITY.to_string()],
        },
    ]);
    with.import_block(&block_1)
        .expect("block 1 was signed by the set declared through block 1");
    let head = with.get_latest_block().expect("db read").expect("a block");
    assert_eq!(head.index, 1);

    // ... and still refuses the old key from block 2 on, so the rotation does what it is for.
    let err = with
        .import_block(&block_2)
        .expect_err("block 2 is past the old set");
    assert!(err.contains("Block author verification failed"), "got: {err}");

    before.shutdown_blockchain();
    without.shutdown_blockchain();
    with.shutdown_blockchain();
}
