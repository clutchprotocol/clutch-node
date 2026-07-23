use crate::node::blockchain::Blockchain;
use crate::node::transactions::ride_request::MapBounds;
use crate::node::transactions::transaction::Transaction;
use crate::node::p2p_server::{GossipMessageType, P2PServer, P2PServerCommand};
use crate::node::rlp_encoding::encode;
use futures::{stream::StreamExt, SinkExt};
use tracing::{error, info, warn};
use std::error::Error;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, Semaphore};
use tokio_tungstenite::accept_async_with_config;
use tokio_tungstenite::tungstenite::protocol::{Message, WebSocketConfig};
use hex;

// Bound per-connection message size and total concurrent connections so an
// unauthenticated peer can't exhaust memory or tasks (default frame cap is 64 MiB).
const MAX_WS_MESSAGE_BYTES: usize = 1 << 20; // 1 MiB
const MAX_WS_CONNECTIONS: usize = 256;

pub struct WebSocket;

impl WebSocket {
    pub async fn run(
        addr: &str,
        blockchain: Arc<Mutex<Blockchain>>,
        command_tx_p2p: tokio::sync::mpsc::Sender<P2PServerCommand>,
    ) -> Result<(), Box<dyn Error>> {
        let listener = TcpListener::bind(addr).await?;
        info!("WebSocket server started on {}", addr);

        let connections = Arc::new(Semaphore::new(MAX_WS_CONNECTIONS));

        while let Ok((stream, _)) = listener.accept().await {
            let permit = match Arc::clone(&connections).try_acquire_owned() {
                Ok(permit) => permit,
                Err(_) => {
                    warn!(
                        "WebSocket connection limit ({}) reached; dropping peer",
                        MAX_WS_CONNECTIONS
                    );
                    continue;
                }
            };
            let blockchain = Arc::clone(&blockchain);
            let command_tx_p2p = command_tx_p2p.clone();
            tokio::spawn(async move {
                let _permit = permit; // released when the connection ends
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
        let mut config = WebSocketConfig::default();
        config.max_message_size = Some(MAX_WS_MESSAGE_BYTES);
        config.max_frame_size = Some(MAX_WS_MESSAGE_BYTES);
        let ws_stream = accept_async_with_config(stream, Some(config)).await?;
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
            "get_next_nonce" => {
                Self::handle_get_next_nonce(params, id, blockchain).await
            }
            "get_account_balance" => {
                Self::handle_get_account_balance(params, id, blockchain).await
            }
            "get_account_balance_effects" => {
                Self::handle_get_account_balance_effects(params, id, blockchain).await
            }
            "get_block_by_index" => {
                Self::handle_get_block_by_index(params, id, blockchain).await
            }
            "list_ride_requests" => {
                Self::handle_list_ride_requests(params, id, blockchain).await
            }
            "list_ride_offers" => {
                Self::handle_list_ride_offers(params, id, blockchain).await
            }
            "list_active_trips" => {
                Self::handle_list_active_trips(params, id, blockchain).await
            }
            "list_completed_trips" => {
                Self::handle_list_completed_trips(params, id, blockchain).await
            }
            "list_recent_trips" => {
                Self::handle_list_recent_trips(params, id, blockchain).await
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

    async fn handle_get_account_balance_effects(
        params: serde_json::Value,
        id: serde_json::Value,
        blockchain: &Arc<Mutex<Blockchain>>,
    ) -> Option<String> {
        #[derive(serde::Deserialize)]
        struct GetAccountBalanceEffectsParams {
            address: String,
            #[serde(default = "default_limit")]
            limit: usize,
            #[serde(default)]
            offset: usize,
        }

        fn default_limit() -> usize {
            20
        }

        let params: GetAccountBalanceEffectsParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                let error_msg = format!(
                    "Invalid params: expected object with 'address' field: {}",
                    e
                );
                warn!("{}", error_msg);
                return Some(json_rpc_error_response(-32602, &error_msg, id));
            }
        };

        let blockchain = blockchain.lock().await;
        let effects = blockchain.get_account_balance_effects(
            &params.address,
            params.limit,
            params.offset,
        );
        Some(json_rpc_success_response(
            serde_json::json!({ "items": effects }),
            id,
        ))
    }

    async fn handle_get_block_by_index(
        params: serde_json::Value,
        id: serde_json::Value,
        blockchain: &Arc<Mutex<Blockchain>>,
    ) -> Option<String> {
        #[derive(serde::Deserialize)]
        struct GetBlockByIndexParams {
            index: usize,
        }

        let params: GetBlockByIndexParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                let error_msg = format!("Invalid params: expected object with 'index' field: {}", e);
                warn!("{}", error_msg);
                return Some(json_rpc_error_response(-32602, &error_msg, id));
            }
        };

        let blockchain = blockchain.lock().await;
        match blockchain.get_blocks_by_indexes(vec![params.index]) {
            Ok(blocks) => {
                if let Some(block) = blocks.into_iter().next() {
                    let block_reward = if block.index == 0 {
                        0
                    } else {
                        blockchain.block_reward_amount()
                    };
                    let reward_recipient = block.author.clone();
                    let mut block_value =
                        serde_json::to_value(&block).unwrap_or(serde_json::Value::Null);
                    if let Some(obj) = block_value.as_object_mut() {
                        obj.insert(
                            "block_reward".to_string(),
                            serde_json::Value::from(block_reward),
                        );
                        obj.insert(
                            "reward_recipient".to_string(),
                            serde_json::Value::from(reward_recipient),
                        );

                        let block_effects =
                            blockchain.get_block_balance_effects(block.index as u64);
                        if !block_effects.is_empty() {
                            obj.insert(
                                "balance_effects".to_string(),
                                serde_json::to_value(&block_effects)
                                    .unwrap_or(serde_json::Value::Array(vec![])),
                            );
                        }

                        if let Some(txs) = obj.get_mut("transactions").and_then(|v| v.as_array_mut())
                        {
                            for (idx, tx_val) in txs.iter_mut().enumerate() {
                                if let Some(tx_obj) = tx_val.as_object_mut() {
                                    let tx_hash = tx_obj
                                        .get("hash")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("");
                                    if !tx_hash.is_empty() {
                                        let effects =
                                            blockchain.get_tx_balance_effects(tx_hash);
                                        if !effects.is_empty() {
                                            tx_obj.insert(
                                                "balance_effects".to_string(),
                                                serde_json::to_value(&effects)
                                                    .unwrap_or(serde_json::Value::Array(vec![])),
                                            );
                                        }
                                    }
                                    let _ = idx;
                                }
                            }
                        }
                    }
                    Some(json_rpc_success_response(
                        block_value,
                        id,
                    ))
                } else {
                    Some(json_rpc_error_response(-32004, "Block not found", id))
                }
            }
            Err(e) => {
                let error_msg = format!("Failed to get block by index {}: {}", params.index, e);
                error!("{}", error_msg);
                Some(json_rpc_error_response(-32000, &error_msg, id))
            }
        }
    }

    async fn handle_list_ride_requests(
        params: serde_json::Value,
        id: serde_json::Value,
        blockchain: &Arc<Mutex<Blockchain>>,
    ) -> Option<String> {
        // Optional bounds: { minLat, maxLat, minLng, maxLng } - all optional, omit for no filter
        let bounds: Option<MapBounds> = if params.is_object() && !params.as_object().unwrap().is_empty() {
            match serde_json::from_value(params) {
                Ok(b) => Some(b),
                Err(e) => {
                    let error_msg = format!("Invalid params for 'list_ride_requests': expected {{ minLat, maxLat, minLng, maxLng }}: {}", e);
                    warn!("{}", error_msg);
                    return Some(json_rpc_error_response(-32602, &error_msg, id));
                }
            }
        } else {
            None
        };

        let blockchain = blockchain.lock().await;
        match blockchain.list_available_ride_requests(bounds) {
            Ok(requests) => {
                let result = serde_json::to_value(requests).unwrap_or(serde_json::Value::Array(vec![]));
                Some(json_rpc_success_response(result, id))
            }
            Err(e) => {
                let error_msg = format!("Failed to list ride requests: {}", e);
                error!("{}", error_msg);
                Some(json_rpc_error_response(-32000, &error_msg, id))
            }
        }
    }

    async fn handle_list_ride_offers(
        params: serde_json::Value,
        id: serde_json::Value,
        blockchain: &Arc<Mutex<Blockchain>>,
    ) -> Option<String> {
        #[derive(serde::Deserialize)]
        struct GetRideOffersParams {
            ride_request_tx_hash: Option<String>,
        }

        let parsed_params: Option<GetRideOffersParams> = if params.is_object() && !params.as_object().unwrap().is_empty() {
            match serde_json::from_value(params) {
                Ok(p) => Some(p),
                Err(e) => {
                    let error_msg = format!("Invalid params: expected object with optional 'ride_request_tx_hash' field: {}", e);
                    warn!("{}", error_msg);
                    return Some(json_rpc_error_response(-32602, &error_msg, id));
                }
            }
        } else {
            None
        };

        let ride_request_tx_hash = parsed_params.and_then(|p| p.ride_request_tx_hash);

        let blockchain = blockchain.lock().await;
        match blockchain.list_ride_offers_for_request(ride_request_tx_hash.as_deref()) {
            Ok(offers) => {
                let result = serde_json::to_value(offers).unwrap_or(serde_json::Value::Array(vec![]));
                Some(json_rpc_success_response(result, id))
            }
            Err(e) => {
                let error_msg = format!("Failed to list ride offers: {}", e);
                error!("{}", error_msg);
                Some(json_rpc_error_response(-32000, &error_msg, id))
            }
        }
    }

    async fn handle_list_active_trips(
        params: serde_json::Value,
        id: serde_json::Value,
        blockchain: &Arc<Mutex<Blockchain>>,
    ) -> Option<String> {
        #[derive(serde::Deserialize)]
        struct ListActiveTripsParams {
            driver_address: Option<String>,
            passenger_address: Option<String>,
        }

        let parsed: ListActiveTripsParams = if params.is_object() {
            serde_json::from_value(params).unwrap_or(ListActiveTripsParams {
                driver_address: None,
                passenger_address: None,
            })
        } else {
            ListActiveTripsParams {
                driver_address: None,
                passenger_address: None,
            }
        };

        let blockchain = blockchain.lock().await;
        match blockchain.list_active_trips(
            parsed.driver_address.as_deref(),
            parsed.passenger_address.as_deref(),
        ) {
            Ok(trips) => {
                let result = serde_json::to_value(trips).unwrap_or(serde_json::Value::Array(vec![]));
                Some(json_rpc_success_response(result, id))
            }
            Err(e) => {
                let error_msg = format!("Failed to list active trips: {}", e);
                error!("{}", error_msg);
                Some(json_rpc_error_response(-32000, &error_msg, id))
            }
        }
    }

    async fn handle_list_completed_trips(
        params: serde_json::Value,
        id: serde_json::Value,
        blockchain: &Arc<Mutex<Blockchain>>,
    ) -> Option<String> {
        #[derive(serde::Deserialize)]
        struct ListCompletedTripsParams {
            driver_address: Option<String>,
            passenger_address: Option<String>,
        }

        let parsed: ListCompletedTripsParams = if params.is_object() {
            serde_json::from_value(params).unwrap_or(ListCompletedTripsParams {
                driver_address: None,
                passenger_address: None,
            })
        } else {
            ListCompletedTripsParams {
                driver_address: None,
                passenger_address: None,
            }
        };

        let blockchain = blockchain.lock().await;
        match blockchain.list_completed_trips(
            parsed.driver_address.as_deref(),
            parsed.passenger_address.as_deref(),
        ) {
            Ok(trips) => {
                let result = serde_json::to_value(trips).unwrap_or(serde_json::Value::Array(vec![]));
                Some(json_rpc_success_response(result, id))
            }
            Err(e) => {
                let error_msg = format!("Failed to list completed trips: {}", e);
                error!("{}", error_msg);
                Some(json_rpc_error_response(-32000, &error_msg, id))
            }
        }
    }

    async fn handle_list_recent_trips(
        params: serde_json::Value,
        id: serde_json::Value,
        blockchain: &Arc<Mutex<Blockchain>>,
    ) -> Option<String> {
        #[derive(serde::Deserialize)]
        struct ListRecentTripsParams {
            driver_address: Option<String>,
            passenger_address: Option<String>,
        }

        let parsed: ListRecentTripsParams = if params.is_object() {
            serde_json::from_value(params).unwrap_or(ListRecentTripsParams {
                driver_address: None,
                passenger_address: None,
            })
        } else {
            ListRecentTripsParams {
                driver_address: None,
                passenger_address: None,
            }
        };

        let blockchain = blockchain.lock().await;
        match blockchain.list_recent_trips(
            parsed.driver_address.as_deref(),
            parsed.passenger_address.as_deref(),
        ) {
            Ok(trips) => {
                let result = serde_json::to_value(trips).unwrap_or(serde_json::Value::Array(vec![]));
                Some(json_rpc_success_response(result, id))
            }
            Err(e) => {
                let error_msg = format!("Failed to list recent trips: {}", e);
                error!("{}", error_msg);
                Some(json_rpc_error_response(-32000, &error_msg, id))
            }
        }
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
