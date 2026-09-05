//! The per-block transaction cap.
//!
//! One transaction per sender per block is already enforced by `drop_intra_block_conflicts`, so
//! the cap only bites once there are more distinct senders in the pool than the cap allows. Both
//! tests below therefore fund a second account first: with a single sender there is nothing for a
//! cap to do.

use clutch_node::node::{
    blockchain::Blockchain,
    transactions::{
        chain_init::ChainInit, function_call::FunctionCall, transaction::Transaction,
        transfer::Transfer,
    },
};

const FAUCET_PK: &str = "0xdeb4cfb63db134698e1879ea24904df074726cc0";
const FAUCET_SK: &str = "d2c446110cfcecbdf05b2be528e72483de5b6f7ef9c7856df2f81f48e9f2748f";
const SECOND_PK: &str = "0x9b6e8afff8329743cac73dbef83ca3cbf9a74c20";
const SECOND_SK: &str = "0883ddd3d07303b87c954b0c9383f7b78f45e002520fc03a8adc80595dbf6509";
const SINK: &str = "0x8f19077627cde4848b090c53c83b12956837d5e9";

fn ci() -> ChainInit {
    ChainInit {
        chain_id: 2077,
        is_testnet: true,
        tx_fee: 1000,
        ride_request_referrer_fee_bps: 200,
        ride_offer_referrer_fee_bps: 200,
        mint_authority: FAUCET_PK.to_string(),
        faucet_address: FAUCET_PK.to_string(),
        faucet_allocation: 1_000_000_000_000_000,
    }
}

fn transfer(from: &str, sk: &str, nonce: u64, to: &str, value: u64) -> Transaction {
    let mut tx = Transaction::new_transaction(
        from.to_string(),
        nonce,
        2077,
        FunctionCall::Transfer(Transfer { to: to.to_string(), value }),
    );
    tx.sign(sk);
    tx
}

/// Two senders in the pool, a cap of one: the block takes one and the other waits. Without the
/// cap both land in the same block, because they are different senders and nothing else stops
/// them sharing one.
#[test]
fn a_block_never_carries_more_transactions_than_the_cap() {
    let mut chain = Blockchain::new(
        "clutch-node-tx-cap-test".to_string(),
        SECOND_PK.to_string(),
        SECOND_SK.to_string(),
        true,
        vec![SECOND_PK.to_string()],
        ci(),
    )
    .with_max_block_transactions(1);

    // Fund the second sender, so the next pool really does hold two distinct senders.
    chain
        .add_transaction_to_pool(&transfer(FAUCET_PK, FAUCET_SK, 1, SECOND_PK, 50_000_000))
        .expect("funding tx rejected");
    chain.author_new_block().expect("funding block");

    chain
        .add_transaction_to_pool(&transfer(FAUCET_PK, FAUCET_SK, 2, SINK, 1_000))
        .expect("faucet tx rejected");
    chain
        .add_transaction_to_pool(&transfer(SECOND_PK, SECOND_SK, 1, SINK, 1_000))
        .expect("second-sender tx rejected");

    let block = chain.author_new_block().expect("capped block");
    assert_eq!(
        block.transactions.len(),
        1,
        "two eligible senders and a cap of 1 must produce a one-transaction block"
    );

    // The one left behind is still in the pool, not dropped, and lands in the next block.
    let next = chain.author_new_block().expect("follow-up block");
    assert_eq!(
        next.transactions.len(),
        1,
        "the transaction the cap deferred must be authored next, not discarded"
    );

    chain.shutdown_blockchain();
}

/// The default must not change what an existing deployment does. Nothing calls the setter here,
/// so both senders belong in the same block.
#[test]
fn without_a_cap_both_senders_share_one_block() {
    let mut chain = Blockchain::new(
        "clutch-node-tx-cap-uncapped-test".to_string(),
        SECOND_PK.to_string(),
        SECOND_SK.to_string(),
        true,
        vec![SECOND_PK.to_string()],
        ci(),
    );

    chain
        .add_transaction_to_pool(&transfer(FAUCET_PK, FAUCET_SK, 1, SECOND_PK, 50_000_000))
        .expect("funding tx rejected");
    chain.author_new_block().expect("funding block");

    chain
        .add_transaction_to_pool(&transfer(FAUCET_PK, FAUCET_SK, 2, SINK, 1_000))
        .expect("faucet tx rejected");
    chain
        .add_transaction_to_pool(&transfer(SECOND_PK, SECOND_SK, 1, SINK, 1_000))
        .expect("second-sender tx rejected");

    let block = chain.author_new_block().expect("uncapped block");
    assert_eq!(
        block.transactions.len(),
        2,
        "with no cap, two distinct senders belong in the same block"
    );

    chain.shutdown_blockchain();
}
