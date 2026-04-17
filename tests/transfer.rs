use std::vec;

use ::tracing::{error, info};
use clutch_node::node::{
    blockchain::Blockchain,
    blocks::block::Block,
    transactions::{function_call::FunctionCall, transaction::Transaction, transfer::Transfer},
};

const BLOCKCHAIN_NAME: &str = "clutch-node-test";
const FROM_ADDRESS_KEY: &str = "0xdeb4cfb63db134698e1879ea24904df074726cc0";
const FROM_SECRET_KEY: &str = "d2c446110cfcecbdf05b2be528e72483de5b6f7ef9c7856df2f81f48e9f2748f";
const TO_ADDRESS_KEY: &str = "0x8f19077627cde4848b090c53c83b12956837d5e9";
const AUTHOR_PUBLIC_KEY: &str = "0x9b6e8afff8329743cac73dbef83ca3cbf9a74c20";
const AUTHOR_SECRET_KEY: &str = "0883ddd3d07303b87c954b0c9383f7b78f45e002520fc03a8adc80595dbf6509";
const BLOCK_REWARD_AMOUNT: u64 = 50;

#[test]
fn transfer_founds() {
    let authorities = vec![AUTHOR_PUBLIC_KEY.to_string()];
    let mut blockchain = Blockchain::new(
        BLOCKCHAIN_NAME.to_string(),
        AUTHOR_PUBLIC_KEY.to_string(),
        AUTHOR_SECRET_KEY.to_string(),
        true,
        authorities,
        BLOCK_REWARD_AMOUNT,
    );

    let blocks = [|| transfer_block(1, 1, 20)];

    for block_creator in blocks.iter() {
        let mut block = block_creator();
        if let Err(e) = import_block(&mut blockchain, &mut block) {
            error!("Error importing block: {}", e);
            continue;
        }
    }

    let latest_block = blockchain
        .get_latest_block()
        .expect("Failed to get the latest block");

    info!(
        "Blockchain name: {:#?}, latest block index: {}",
        blockchain.name, latest_block.index,
    );

    let from_account_state = blockchain.get_account_state(&FROM_ADDRESS_KEY.to_string());
    info!("From account state: {:#?}", from_account_state);

    blockchain.shutdown_blockchain();
}

fn import_block(blockchain: &mut Blockchain, block: &mut Block) -> Result<(), String> {
    block.previous_hash = blockchain
        .get_latest_block()
        .expect("Failed to get the latest block")
        .hash;

    blockchain.import_block(block)
}

fn transfer_block(index: usize, nonce: u64, transfer_value: u64) -> Block {
    let mut transfer_transaction = Transaction::new_transaction(
        FROM_ADDRESS_KEY.to_string(),
        nonce,
        FunctionCall::Transfer(Transfer {
            to: TO_ADDRESS_KEY.to_string(),
            value: transfer_value,
        }),
    );
    transfer_transaction.sign(FROM_SECRET_KEY);

    let mut block = Block::new_block(index, String::new(), vec![transfer_transaction]);

    block.sign(AUTHOR_PUBLIC_KEY, AUTHOR_SECRET_KEY);
    block
}
