use crate::node::blockchain::Blockchain;
use crate::node::configuration::AppConfig;
use crate::node::metric::serve_metrics;
use crate::node::p2p_server::commands::DirectMessageType;
use crate::node::p2p_server::{GossipMessageType, P2PServer, P2PServerCommand};
use crate::node::rlp_encoding::encode;
use crate::node::wss::websocket::WebSocket;

use std::sync::Arc;
use std::time::Duration;
use tokio::signal;
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{debug, error, info};

pub struct NodeServices;

impl NodeServices {
    pub async fn start_services(config: &AppConfig, blockchain: Blockchain) {
        let blockchain_arc = Arc::new(Mutex::new(blockchain));

        if config.serve_metric_enabled {
            serve_metrics(config);
        }

        let (libp2p_shutdown_tx, libp2p_shutdown_rx) = oneshot::channel();
        let (command_tx_p2p, command_rx_p2p) = mpsc::channel(32);
        Self::start_libp2p(
            config,
            Arc::clone(&blockchain_arc),
            libp2p_shutdown_tx,
            command_rx_p2p,
        );

        let (websocket_shutdown_tx, websocket_shutdown_rx) = oneshot::channel();
        Self::start_websocket(
            config,
            Arc::clone(&blockchain_arc),
            command_tx_p2p.clone(),
            websocket_shutdown_tx,
        );

        if config.block_authoring_enabled {
            Self::start_authoring_job(Arc::clone(&blockchain_arc), 1, command_tx_p2p.clone());
        }

        if config.sync_enabled {
            Self::start_sync(Arc::clone(&blockchain_arc), command_tx_p2p.clone());
        }

        Self::wait_for_shutdown_signal(
            libp2p_shutdown_rx,
            websocket_shutdown_rx,
            Arc::clone(&blockchain_arc),
        )
        .await;
    }

    async fn wait_for_shutdown_signal(
        libp2p_shutdown_rx: oneshot::Receiver<()>,
        websocket_shutdown_rx: oneshot::Receiver<()>,
        blockchain: Arc<Mutex<Blockchain>>,
    ) {
        tokio::select! {
            _ = signal::ctrl_c() => {
                info!("Received Ctrl+C, shutting down.");
            }
            _ = libp2p_shutdown_rx => {
                info!("Libp2p service completed, shutting down.");
            }
            _ = websocket_shutdown_rx => {
                info!("WebSocket service completed, shutting down.");
            }
        }

        let mut blockchain = blockchain.lock().await;
        blockchain.shutdown_blockchain();
    }

    fn start_libp2p(
        config: &AppConfig,
        blockchain: Arc<Mutex<Blockchain>>,
        libp2p_shutdown_tx: oneshot::Sender<()>,
        command_rx: tokio::sync::mpsc::Receiver<P2PServerCommand>,
    ) {
        let listen_addrs: Vec<&str> = config.listen_addrs.iter().map(|s| s.as_str()).collect();
        let bootstrap_nodes: Vec<&str> =
            config.bootstrap_nodes.iter().map(|s| s.as_str()).collect();

        let mut p2p_server =
            match P2PServer::new(&config.libp2p_topic_name, &listen_addrs, &bootstrap_nodes) {
                Ok(server) => server,
                Err(e) => {
                    error!("Failed to create P2PServer: {}", e);
                    return;
                }
            };

        tokio::spawn(async move {
            {
                if let Err(e) = p2p_server.run(Arc::clone(&blockchain), command_rx).await {
                    error!("Error running libp2p: {}", e);
                }
            }
            let _ = libp2p_shutdown_tx.send(());
        });
    }

    fn start_websocket(
        config: &AppConfig,
        blockchain: Arc<Mutex<Blockchain>>,
        command_tx_p2p: tokio::sync::mpsc::Sender<P2PServerCommand>,
        websocket_shutdown_tx: oneshot::Sender<()>,
    ) {
        let websocket_addr = config.websocket_addr.clone();

        tokio::spawn(async move {
            if let Err(e) = WebSocket::run(&websocket_addr, blockchain, command_tx_p2p).await {
                error!("Error starting WebSocket server: {}", e);
            }
            let _ = websocket_shutdown_tx.send(());
        });
    }

    pub fn start_authoring_job(
        blockchain: Arc<Mutex<Blockchain>>,
        interval_secs: u64,
        command_tx_p2p: tokio::sync::mpsc::Sender<P2PServerCommand>,
    ) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
            loop {
                interval.tick().await;
                let blockchain = blockchain.lock().await;
                match blockchain.author_new_block() {
                    Ok(block) => {
                        let encoded_block = encode(&block);
                        P2PServer::gossip_message_command(
                            command_tx_p2p.clone(),
                            GossipMessageType::Block,
                            &encoded_block,
                        )
                        .await;
                    }
                    Err(e) => {
                        debug!("Error authoring new block: {:?}", e);
                    }
                }
            }
        });
    }

    pub fn start_sync(
        blockchain: Arc<Mutex<Blockchain>>,
        command_tx_p2p: tokio::sync::mpsc::Sender<P2PServerCommand>,
    ) {
        tokio::spawn(async move {
            // Initial delay to allow libp2p connections to establish
            tokio::time::sleep(Duration::from_secs(3)).await;

            // Retry until at least one peer is connected (e.g. bootstrap node)
            const MAX_RETRIES: u32 = 15;
            const RETRY_INTERVAL_SECS: u64 = 2;

            let mut peer_id = None;
            for attempt in 1..=MAX_RETRIES {
                let peers_result = P2PServer::get_connected_peers_command(command_tx_p2p.clone())
                    .await
                    .map_err(|e| e.to_string());
                match peers_result {
                    Ok(connected_peers) => {
                        peer_id = connected_peers.iter().next().cloned();
                        if peer_id.is_some() {
                            info!("connected peers: {:?}", connected_peers);
                            break;
                        }
                        if attempt < MAX_RETRIES {
                            debug!(
                                "No peers connected yet (attempt {}/{}), retrying in {}s",
                                attempt, MAX_RETRIES, RETRY_INTERVAL_SECS
                            );
                            tokio::time::sleep(Duration::from_secs(RETRY_INTERVAL_SECS)).await;
                        }
                    }
                    Err(msg) => {
                        error!("Failed to get connected peers: {}", msg);
                        if attempt < MAX_RETRIES {
                            tokio::time::sleep(Duration::from_secs(RETRY_INTERVAL_SECS)).await;
                        }
                    }
                }
            }

            if let Some(pid) = peer_id {
                debug!("Selected peer for synchronization: {:?}", pid);

                let blockchain = blockchain.lock().await;
                let handshake = blockchain.handshake().unwrap();
                let encoded_handshake = encode(&handshake);

                if let Err(e) = P2PServer::send_direct_message_command(
                    command_tx_p2p.clone(),
                    pid,
                    DirectMessageType::Handshake,
                    &encoded_handshake,
                )
                .await
                {
                    error!("Failed to send handshake: {}", e);
                }
            } else {
                error!("Failed to select a peer from the connected peers set after {} attempts. Ensure bootstrap_nodes are correct and node1 is reachable.", MAX_RETRIES);
            }
        });
    }
}
