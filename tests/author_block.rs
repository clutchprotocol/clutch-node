use std::vec;

use clutch_node::node::{blockchain::Blockchain, transactions::{function_call::FunctionCall, transaction::Transaction, transfer::Transfer}};
use ::tracing::info;

const BLOCKCHAIN_NAME: &str = "clutch-node-transfer-test";
const FROM_ADDRESS_KEY: &str = "0xdeb4cfb63db134698e1879ea24904df074726cc0";
const FROM_SECRET_KEY: &str = "d2c446110cfcecbdf05b2be528e72483de5b6f7ef9c7856df2f81f48e9f2748f";
const TO_ADDRESS_KEY: &str = "0x8f19077627cde4848b090c53c83b12956837d5e9";
const AUTHOR_PUBLIC_KEY: &str = "0x9b6e8afff8329743cac73dbef83ca3cbf9a74c20";
const AUTHOR_SECRET_KEY: &str = "0883ddd3d07303b87c954b0c9383f7b78f45e002520fc03a8adc80595dbf6509";
const BLOCK_REWARD_AMOUNT: u64 = 50;

#[test]
fn author_block() {
    let authorities = vec![AUTHOR_PUBLIC_KEY.to_string()];
    let mut blockchain = Blockchain::new(
        BLOCKCHAIN_NAME.to_string(),
        AUTHOR_PUBLIC_KEY.to_string(),
        AUTHOR_SECRET_KEY.to_string(),
        true,
        authorities,
        BLOCK_REWARD_AMOUNT,
    );

    let transfer_tx = transfer_transaction(1, 20);

    blockchain
        .add_transaction_to_pool(&transfer_tx)
        .expect("Failed to add transaction to pool");

    blockchain
        .author_new_block()
        .expect("failed to author new block");

    let latest_block = blockchain
        .get_latest_block()
        .expect("Failed to get the latest block");

    info!(
        "Blockchain name: {:#?}, latest block index: {}",
        blockchain.name, latest_block.index,
    );

    let from_account_state = blockchain.get_account_state(&FROM_ADDRESS_KEY.to_string());
    info!("From account state: {:#?}", from_account_state);

    let author_account_state = blockchain.get_account_state(&AUTHOR_PUBLIC_KEY.to_string());
    assert_eq!(
        author_account_state.balance, BLOCK_REWARD_AMOUNT,
        "author should receive exactly one block reward",
    );

    blockchain.shutdown_blockchain();
}

fn transfer_transaction(nonce: u64, transfer_value: u64) -> Transaction {
    let transfer = Transfer {
        to: TO_ADDRESS_KEY.to_string(),
        value: transfer_value,
    };

    let mut transfer_transaction = Transaction::new_transaction(
        FROM_ADDRESS_KEY.to_string(),
        nonce,        
        FunctionCall::Transfer(transfer),
    );
    transfer_transaction.sign(FROM_SECRET_KEY);
    transfer_transaction
}
