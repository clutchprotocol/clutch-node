use clutch_node::node::{
    blockchain::Blockchain,
    transactions::{function_call::FunctionCall, transaction::Transaction, transfer::Transfer},
};
use serial_test::serial;

const BLOCK_REWARD_AMOUNT: u64 = 50;
const AUTHOR_PUBLIC_KEY: &str = "0x9b6e8afff8329743cac73dbef83ca3cbf9a74c20";
const AUTHOR_SECRET_KEY: &str = "0883ddd3d07303b87c954b0c9383f7b78f45e002520fc03a8adc80595dbf6509";
const FROM_ADDRESS_KEY: &str = "0xdeb4cfb63db134698e1879ea24904df074726cc0";
const FROM_SECRET_KEY: &str = "d2c446110cfcecbdf05b2be528e72483de5b6f7ef9c7856df2f81f48e9f2748f";
const TO_ADDRESS_KEY: &str = "0x8f19077627cde4848b090c53c83b12956837d5e9";

fn new_blockchain(name: &str) -> Blockchain {
    Blockchain::new(
        name.to_string(),
        AUTHOR_PUBLIC_KEY.to_string(),
        AUTHOR_SECRET_KEY.to_string(),
        true,
        vec![AUTHOR_PUBLIC_KEY.to_string()],
        BLOCK_REWARD_AMOUNT,
    )
}

#[test]
#[serial]
fn author_gets_block_reward_on_authored_block() {
    let mut blockchain = new_blockchain("clutch-node-block-reward-author-test");
    let mut transfer_transaction = Transaction::new_transaction(
        FROM_ADDRESS_KEY.to_string(),
        1,
        FunctionCall::Transfer(Transfer {
            to: TO_ADDRESS_KEY.to_string(),
            value: 1,
        }),
    );
    transfer_transaction.sign(FROM_SECRET_KEY);

    blockchain
        .add_transaction_to_pool(&transfer_transaction)
        .expect("failed to add tx to pool");

    blockchain
        .author_new_block()
        .expect("failed to author block with reward");

    let author_balance = blockchain.get_account_balance(&AUTHOR_PUBLIC_KEY.to_string());
    assert_eq!(author_balance, BLOCK_REWARD_AMOUNT);

    blockchain.shutdown_blockchain();
}

#[test]
#[serial]
fn genesis_block_does_not_mint_author_reward() {
    let mut blockchain = new_blockchain("clutch-node-block-reward-genesis-test");
    let author_balance = blockchain.get_account_balance(&AUTHOR_PUBLIC_KEY.to_string());
    assert_eq!(author_balance, 0);
    blockchain.shutdown_blockchain();
}
