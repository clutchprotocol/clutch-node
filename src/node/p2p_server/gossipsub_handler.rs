use crate::node::{blockchain::Blockchain, blocks::block::Block};
use crate::node::rlp_encoding::decode;
use crate::node::transactions::transaction::Transaction;
use crate::node::p2p_server::GossipMessageType;

use libp2p::{
    gossipsub::{self, MessageId},
    PeerId,
};
use tracing::{error, info};
use std::sync::Arc;
use tokio::sync::Mutex;

pub async fn handle_gossipsub_message(
    peer_id: PeerId,
    id: MessageId,
    message: gossipsub::Message,
    blockchain: &Arc<Mutex<Blockchain>>,
) {
    info!(
        "Received gossip message from peer: {} with id:'{}': {} ",
        peer_id,
        id,
        String::from_utf8_lossy(&message.data),
    );

    if message.data.is_empty() {
        error!("Received empty gossip message from peer: {}", peer_id);
        return;
    }
    let message_type = GossipMessageType::from_byte(message.data[0]);
    let payload = &message.data[1..];

    match message_type {
        Some(GossipMessageType::Transaction) => match decode::<Transaction>(payload) {
            Ok(transaction) => {
                info!("Decoded transaction: {:?}", &transaction);
                handle_received_transaction(&transaction, blockchain).await;
            }
            Err(e) => {
                error!("Failed to decode transaction: {:?}", e);
            }
        },
        Some(GossipMessageType::Block) => match decode::<Block>(payload) {
            Ok(block) => {
                info!("Decoded block: {:?}", &block);
                handle_received_block(&block, blockchain).await;
            }
            Err(e) => {
                error!("Failed to decode block: {:?}", e);
            }
        },
        _ => {
            error!("Unknown message type: {:?}", message_type);
        }
    }
}

async fn handle_received_transaction(
    transaction: &Transaction,
    blockchain: &Arc<Mutex<Blockchain>>,
) {
    let result = {
        let blockchain = blockchain.lock().await;
        blockchain.add_transaction_to_pool(&transaction)
    };

    match result {
        Ok(_) => info!("Transaction added to mempool from P2P"),
        Err(e) => error!("Failed to add transaction to pool: {:?}", e),
    }
}

async fn handle_received_block(block: &Block, blockchain: &Arc<Mutex<Blockchain>>) {
    let result = {
        let blockchain = blockchain.lock().await;
        blockchain.import_block(&block)
    };

    match result {
        Ok(_) => info!("Block added to blockchain from P2P"),
        Err(e) => error!("Failed to add block to blockchain: {:?}", e),
    }
}
