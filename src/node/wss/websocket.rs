use crate::node::blockchain::Blockchain;
use crate::node::blocks::block::Block;
use crate::node::transactions::transaction::Transaction;
use crate::node::p2p_server::{GossipMessageType, P2PServer, P2PServerCommand};
use crate::node::rlp_encoding::encode;
use futures::{stream::StreamExt, SinkExt};
use tracing::{error, info, warn};
use std::error::Error;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::protocol::Message;
use hex;

pub struct WebSocket;

impl WebSocket {
    pub async fn run(
        addr: &str,
        blockchain: Arc<Mutex<Blockchain>>,
        command_tx_p2p: tokio::sync::mpsc::Sender<P2PServerCommand>,
    ) -> Result<(), Box<dyn Error>> {
        let listener = TcpListener::bind(addr).await?;
        info!("WebSocket server started on {}", addr);

        while let Ok((stream, _)) = listener.accept().await {
            let blockchain = Arc::clone(&blockchain);
            let command_tx_p2p = command_tx_p2p.clone();
            tokio::spawn(async move {
                if let Err(e) = Self::handle_connection(stream, blockchain, command_tx_p2p).await {
                    error!("Error handling connection: {}", e);
                }
            });
        }

        Ok(())
    }

    async fn handle_connection(
        stream: TcpStream,
        blockchain: Arc<Mutex<Blockchain>>,
        command_tx_p2p: tokio::sync::mpsc::Sender<P2PServerCommand>,
    ) -> Result<(), Box<dyn Error>> {
        let ws_stream = accept_async(stream).await?;
        let (mut ws_sender, mut ws_receiver) = ws_stream.split();

        while let Some(message) = ws_receiver.next().await {
            match message {
                Ok(Message::Text(text)) => {
                    info!("Received from websocket: {}", text);
                    if let Some(response) = Self::handle_json_rpc_request(&text, &blockchain, command_tx_p2p.clone()).await {
                        if let Err(e) = ws_sender.send(Message::Text(response)).await {
                            error!("Error sending message: {}", e);
                            return Err(Box::new(e));
                        }
                    }
                }
                Ok(_) => { /* Handle other message types if necessary */ }
                Err(e) => {
                    error!("Error receiving message: {}", e);
                    return Err(Box::new(e));
                }
            }
        }

        Ok(())
    }

    async fn handle_json_rpc_request(
        request_str: &str,
        blockchain: &Arc<Mutex<Blockchain>>,
        command_tx_p2p: tokio::sync::mpsc::Sender<P2PServerCommand>,
    ) -> Option<String> {
        let request_value: serde_json::Value = match serde_json::from_str(request_str) {
            Ok(val) => val,
            Err(e) => {
                warn!("Failed to parse JSON-RPC request: {}. Error: {}", request_str, e);
                return Some(json_rpc_error_response(-32700, "Parse error", serde_json::Value::Null));
            }
        };

        let method = match request_value.get("method").and_then(|v| v.as_str()) {
            Some(m) => m,
            None => {
                warn!("Missing 'method' field in request: {}", request_str);
                let id = request_value.get("id").cloned().unwrap_or(serde_json::Value::Null);
                return Some(json_rpc_error_response(-32600, "Invalid Request", id));
            }
        };

        let params = request_value.get("params").cloned().unwrap_or(serde_json::Value::Null);
        let id = request_value.get("id").cloned().unwrap_or(serde_json::Value::Null);

        match method {
            "send_transaction" => {
                Self::handle_send_transaction(params, id, blockchain, command_tx_p2p).await
            }
            "send_raw_transaction" => {
                Self::handle_send_raw_transaction(params, id, blockchain, command_tx_p2p).await
            }
            "import_block" => {
                Self::handle_import_block(params, id, blockchain, command_tx_p2p).await
            }
            "author_new_block" => {
                Self::handle_author_new_block(id, blockchain, command_tx_p2p).await
            }
            "get_next_nonce" => {
                Self::handle_get_next_nonce(params, id, blockchain).await
            }
            "get_account_balance" => {
                Self::handle_get_account_balance(params, id, blockchain).await
            }
            _ => {
                warn!("Unknown method '{}' in request: {}", method, request_str);
                Some(json_rpc_error_response(-32601, "Method not found", id))
            }
        }
    }

    async fn handle_send_transaction(
        params: serde_json::Value,
        id: serde_json::Value,
        blockchain: &Arc<Mutex<Blockchain>>,
        command_tx_p2p: tokio::sync::mpsc::Sender<P2PServerCommand>,
    ) -> Option<String> {
        let transaction: Transaction = match serde_json::from_value(params) {
            Ok(tx) => tx,
            Err(e) => {
                let error_msg = format!("Invalid params for 'send_transaction': {}", e);
                warn!("{}", error_msg);
                return Some(json_rpc_error_response(-32602, &error_msg, id));
            }
        };

        let blockchain = blockchain.lock().await;
        if let Err(e) = blockchain.add_transaction_to_pool(&transaction) {
            let error_msg = format!("Failed to add transaction: {}", e);
            error!("{}", error_msg);
            return Some(json_rpc_error_response(-32000, &error_msg, id));
        }

        info!("Transaction added to pool from WebSocket.");

        // Gossip transaction
        let encoded_tx = encode(&transaction);
        P2PServer::gossip_message_command(command_tx_p2p, GossipMessageType::Transaction, &encoded_tx).await;

        Some(json_rpc_success_response(serde_json::json!("Transaction imported"), id))
    }

    async fn handle_send_raw_transaction(
        params: serde_json::Value,
        id: serde_json::Value,
        blockchain: &Arc<Mutex<Blockchain>>,
        command_tx_p2p: tokio::sync::mpsc::Sender<P2PServerCommand>,
    ) -> Option<String> {
        // Expect params to be a hex string (RLP encoded)
        let hex_str = match params.as_str() {
            Some(s) => s,
            None => {
                let error_msg = "Invalid params: expected hex string for raw transaction";
                warn!("{}", error_msg);
                return Some(json_rpc_error_response(-32602, error_msg, id));
            }
        };
        let tx_bytes = match hex::decode(hex_str.trim_start_matches("0x")) {
            Ok(bytes) => bytes,
            Err(e) => {
                let error_msg = format!("Failed to decode hex: {}", e);
                warn!("{}", error_msg);
                return Some(json_rpc_error_response(-32602, &error_msg, id));
            }
        };
        // Decode RLP to Transaction
        let transaction: Transaction = match crate::node::rlp_encoding::decode(&tx_bytes) {
            Ok(tx) => tx,
            Err(e) => {
                let error_msg = format!("Failed to decode RLP transaction: {}", e);
                warn!("{}", error_msg);
                return Some(json_rpc_error_response(-32602, &error_msg, id));
            }
        };
        let blockchain = blockchain.lock().await;
        if let Err(e) = blockchain.add_transaction_to_pool(&transaction) {
            let error_msg = format!("Failed to add transaction: {}", e);
            error!("{}", error_msg);
            return Some(json_rpc_error_response(-32000, &error_msg, id));
        }
        info!("Transaction added to pool from WebSocket.");
        // Gossip transaction
        let encoded_tx = encode(&transaction);
        P2PServer::gossip_message_command(command_tx_p2p, GossipMessageType::Transaction, &encoded_tx).await;
        Some(json_rpc_success_response(serde_json::json!("Transaction imported"), id))
    }

    async fn handle_import_block(
        params: serde_json::Value,
        id: serde_json::Value,
        blockchain: &Arc<Mutex<Blockchain>>,
        command_tx_p2p: tokio::sync::mpsc::Sender<P2PServerCommand>,
    ) -> Option<String> {
        let block: Block = match serde_json::from_value(params) {
            Ok(b) => b,
            Err(e) => {
                warn!("Invalid params for 'import_block': {}", e);
                return Some(json_rpc_error_response(-32602, "Invalid params", id));
            }
        };

        let blockchain = blockchain.lock().await;
        if let Err(e) = blockchain.import_block(&block) {
            error!("Failed to import block: {}", e);
            return Some(json_rpc_error_response(-32000, &format!("Failed to import block: {}", e), id));
        }

        info!("Block imported to blockchain from WebSocket.");

        // Gossip block
        let encoded_block = encode(&block);
        P2PServer::gossip_message_command(command_tx_p2p, GossipMessageType::Block, &encoded_block).await;

        Some(json_rpc_success_response(serde_json::json!("Block imported"), id))
    }

    async fn handle_author_new_block(
        id: serde_json::Value,
        blockchain: &Arc<Mutex<Blockchain>>,
        command_tx_p2p: tokio::sync::mpsc::Sender<P2PServerCommand>,
    ) -> Option<String> {
        let blockchain = blockchain.lock().await;
        let new_block = match blockchain.author_new_block() {
            Ok(block) => block,
            Err(e) => {
                error!("Failed to author new block: {}", e);
                return Some(json_rpc_error_response(-32000, &format!("Failed to author new block: {}", e), id));
            }
        };

        info!("New block authored and added to the blockchain from WebSocket.");

        // Gossip new block
        let encoded_block = encode(&new_block);
        P2PServer::gossip_message_command(command_tx_p2p, GossipMessageType::Block, &encoded_block).await;

        Some(json_rpc_success_response(serde_json::json!("New block authored"), id))
    }

    async fn handle_get_next_nonce(
        params: serde_json::Value,
        id: serde_json::Value,
        blockchain: &Arc<Mutex<Blockchain>>,
    ) -> Option<String> {

        #[derive(serde::Deserialize)]
        struct GetNonceParams {
            address: String,
        }
        // Parse params as an object
        let params: GetNonceParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                let error_msg = format!("Invalid params: expected object with 'address' field: {}", e);
                warn!("{}", error_msg);
                return Some(json_rpc_error_response(-32602, &error_msg, id));
            }
        };

        // Get the blockchain lock
        let blockchain = blockchain.lock().await;
        
        match blockchain.get_current_nonce(&params.address) {
            Ok(nonce) => {
                let next_nonce = nonce + 1;
                Some(json_rpc_success_response(serde_json::json!({ "nonce": next_nonce }), id))
            }
            Err(e) => {
                let error_msg = format!("Failed to get next nonce for address {}: {}", params.address, e);
                error!("{}", error_msg);
                Some(json_rpc_error_response(-32000, &error_msg, id))
            }
        }
    }

    async fn handle_get_account_balance(
        params: serde_json::Value,
        id: serde_json::Value,
        blockchain: &Arc<Mutex<Blockchain>>,
    ) -> Option<String> {
        #[derive(serde::Deserialize)]
        struct GetBalanceParams {
            address: String,
        }

        let params: GetBalanceParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                let error_msg = format!("Invalid params: expected object with 'address' field: {}", e);
                warn!("{}", error_msg);
                return Some(json_rpc_error_response(-32602, &error_msg, id));
            }
        };

        let blockchain = blockchain.lock().await;
        let balance = blockchain.get_account_balance(&params.address);
        Some(json_rpc_success_response(serde_json::json!({ "balance": balance }), id))
    }
}

fn json_rpc_error_response(code: i32, message: &str, id: serde_json::Value) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "error": { "code": code, "message": message },
        "id": id
    })
    .to_string()
}

fn json_rpc_success_response(result: serde_json::Value, id: serde_json::Value) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "result": result,
        "id": id
    })
    .to_string()
}
