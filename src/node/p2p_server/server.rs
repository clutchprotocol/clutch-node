use crate::node::blockchain::Blockchain;
use futures::stream::StreamExt;
use futures::FutureExt;
use libp2p::{
    gossipsub::{self, Event as GossipsubEvent, IdentTopic, MessageId},
    mdns::{self, Event as MdnsEvent},
    noise,
    request_response::{
        cbor::Behaviour as RequestResponseBehavior, Config as RequestResponseConfig,
        OutboundRequestId, ProtocolSupport as RequestResponseProtocolSupport,
    },
    swarm::{Swarm, SwarmEvent},
    tcp, yamux, Multiaddr, PeerId, StreamProtocol,
};
use tracing::{info,error};

use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
use std::error::Error as StdError;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::Sender;
use tokio::{
    io, select,
    sync::{oneshot, Mutex},
};

use super::{
    behaviour::{DirectMessageRequest, DirectMessageResponse, P2PBehaviourEvent},
    commands::DirectMessageType,
    gossipsub_handler::handle_gossipsub_message,
    request_response_handler::handle_request_response,
    GossipMessageType, P2PBehaviour, P2PServerCommand,
};

pub struct P2PServer {
    pub behaviour: Swarm<P2PBehaviour>,
    pub topic: IdentTopic,
}

impl P2PServer {
    pub fn new(
        topic_name: &str,
        listen_addrs: &[&str],
        peer_addrs: &[&str],
    ) -> Result<Self, Box<dyn StdError>> {
        let mut swarm = Self::build_swarm(listen_addrs)?;
        let topic = Self::setup_gossipsub_topic(&mut swarm, topic_name)?;

        for peer in peer_addrs {
            let addr: Multiaddr = peer.parse()?;
            Swarm::dial(&mut swarm, addr)?;
        }

        Ok(Self {
            behaviour: swarm,
            topic,
        })
    }

    pub async fn gossip_message_command(
        command_tx_p2p: Sender<P2PServerCommand>,
        message_type: GossipMessageType,
        message: &Vec<u8>,
    ) {
        let mut message_with_type = vec![message_type.as_byte()];
        message_with_type.extend(message);

        let (response_tx, response_rx) = oneshot::channel();
        command_tx_p2p
            .send(P2PServerCommand::SendGossipMessage {
                message: message_with_type,
                response_tx,
            })
            .await
            .unwrap();

        match response_rx.await {
            Ok(result) => match result {
                Ok(message_id) => info!("Message sent with id: {:?}", message_id),
                Err(e) => error!("Failed to send message: {:?}", e),
            },
            Err(e) => error!("Failed to receive response: {:?}", e),
        }
    }

    pub async fn send_direct_message_command(
        command_tx_p2p: Sender<P2PServerCommand>,
        peer_id: PeerId,
        message_type: DirectMessageType,
        message: &Vec<u8>,
    ) -> Result<OutboundRequestId, Box<dyn StdError>> {
        let (response_tx, response_rx) = oneshot::channel();

        let mut message_with_type = vec![message_type.as_byte()];
        message_with_type.extend(message);

        let direct_message = DirectMessageRequest {
            message: message_with_type,
        };

        command_tx_p2p
            .send(P2PServerCommand::SendDirectMessage {
                peer_id,
                message: direct_message,
                response_tx,
            })
            .await?;

        // Await the response and return the OutboundRequestId
        match response_rx.await {
            Ok(request_id) => Ok(request_id),
            Err(e) => Err(Box::new(e)),
        }
    }

    pub async fn get_local_peer_id_command(command_tx_p2p: Sender<P2PServerCommand>) -> PeerId {
        let (response_tx, response_rx) = oneshot::channel();

        command_tx_p2p
            .send(P2PServerCommand::GetLocalPeerId { response_tx })
            .await
            .unwrap();

        response_rx.await.unwrap()
    }

    pub async fn get_connected_peers_command(
        command_tx_p2p: Sender<P2PServerCommand>,
    ) -> Result<HashSet<PeerId>, Box<dyn StdError>> {
        let (response_tx, response_rx) = oneshot::channel();
        command_tx_p2p
            .send(P2PServerCommand::GetConnectedPeers { response_tx })
            .await?;

        let peers = response_rx.await?;
        Ok(peers)
    }

    pub async fn run(
        &mut self,
        blockchain: Arc<Mutex<Blockchain>>,
        mut command_rx: tokio::sync::mpsc::Receiver<P2PServerCommand>,
    ) -> Result<(), Box<dyn StdError>> {      
        self.process_messages(blockchain, &mut command_rx).await
    }

    fn build_swarm(listen_addrs: &[&str]) -> Result<Swarm<P2PBehaviour>, Box<dyn StdError>> {
        let mut swarm = libp2p::SwarmBuilder::with_new_identity()
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                noise::Config::new,
                yamux::Config::default,
            )?
            .with_quic()
            .with_behaviour(|key| {
                let message_id_fn = |message: &gossipsub::Message| {
                    let mut s = DefaultHasher::new();
                    message.data.hash(&mut s);
                    gossipsub::MessageId::from(s.finish().to_string())
                };

                let gossipsub_config = gossipsub::ConfigBuilder::default()
                    .heartbeat_interval(Duration::from_secs(10))
                    .validation_mode(gossipsub::ValidationMode::Strict)
                    .message_id_fn(message_id_fn)
                    .build()
                    .map_err(|msg| io::Error::new(io::ErrorKind::Other, msg))?;

                let gossipsub = gossipsub::Behaviour::new(
                    gossipsub::MessageAuthenticity::Signed(key.clone()),
                    gossipsub_config,
                )?;

                let mdns = mdns::tokio::Behaviour::new(
                    mdns::Config::default(),
                    key.public().to_peer_id(),
                )?;

                let rr_config = RequestResponseConfig::default();
                let rr_protocol = StreamProtocol::new("/agent/message/1.0.0");
                let rr_behavior =
                    RequestResponseBehavior::<DirectMessageRequest, DirectMessageResponse>::new(
                        [(rr_protocol, RequestResponseProtocolSupport::Full)],
                        rr_config,
                    );

                Ok(P2PBehaviour {
                    gossipsub,
                    mdns,
                    request_response: rr_behavior,
                })
            })?
            .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
            .build();

        for addr in listen_addrs {
            swarm.listen_on(addr.parse()?)?;
        }

        Ok(swarm)
    }

    fn setup_gossipsub_topic(
        swarm: &mut Swarm<P2PBehaviour>,
        topic_name: &str,
    ) -> Result<IdentTopic, Box<dyn StdError>> {
        let topic = IdentTopic::new(topic_name);
        swarm.behaviour_mut().gossipsub.subscribe(&topic)?;
        Ok(topic)
    }

    async fn process_messages(
        &mut self,
        blockchain: Arc<Mutex<Blockchain>>,
        command_rx: &mut tokio::sync::mpsc::Receiver<P2PServerCommand>,
    ) -> Result<(), Box<dyn StdError>> {
        loop {
            select! {
                event = self.behaviour.select_next_some().fuse() => {
                    Self::handle_swarm_event(event, &mut self.behaviour, &blockchain).await;
                },
                command = command_rx.recv() => {
                    if let Some(command) = command {
                        match command {
                            P2PServerCommand::SendGossipMessage { message, response_tx } => {
                                let result = self.send_gossip_message(message);
                                let _ = response_tx.send(result);
                            },
                            P2PServerCommand::GetConnectedPeers { response_tx } => {
                                let peers = self.get_connected_peers();
                                let _ = response_tx.send(peers);
                            },
                            P2PServerCommand::SendDirectMessage { peer_id, message, response_tx } => {
                                let result = self.send_direct_message(&peer_id, message);
                                let _ = response_tx.send(result);
                            },
                            P2PServerCommand::GetLocalPeerId { response_tx } => {
                                let peer_id = self.get_local_peer_id();
                                let _ = response_tx.send(peer_id);
                            },
                        }
                    }
                },
            }
        }
    }

    fn send_gossip_message(
        &mut self,
        message: Vec<u8>,
    ) -> Result<MessageId, gossipsub::PublishError> {
        self.behaviour
            .behaviour_mut()
            .gossipsub
            .publish(self.topic.clone(), message)
    }

    fn get_connected_peers(&self) -> HashSet<PeerId> {
        self.behaviour.connected_peers().cloned().collect()
    }

    fn send_direct_message(
        &mut self,
        peer_id: &PeerId,
        message: DirectMessageRequest,
    ) -> libp2p::request_response::OutboundRequestId {
        self.behaviour
            .behaviour_mut()
            .request_response
            .send_request(&peer_id, message)
    }

    fn get_local_peer_id(&self) -> PeerId {
        *self.behaviour.local_peer_id()
    }

    async fn handle_swarm_event(
        event: SwarmEvent<P2PBehaviourEvent>,
        swarm: &mut Swarm<P2PBehaviour>,
        blockchain: &Arc<Mutex<Blockchain>>,
    ) {
        match event {
            SwarmEvent::Behaviour(P2PBehaviourEvent::Mdns(MdnsEvent::Discovered(list))) => {
                Self::handle_mdns_discovered(swarm, list);
            }
            SwarmEvent::Behaviour(P2PBehaviourEvent::Mdns(MdnsEvent::Expired(list))) => {
                Self::handle_mdns_expired(swarm, list);
            }
            SwarmEvent::Behaviour(P2PBehaviourEvent::Gossipsub(GossipsubEvent::Message {
                propagation_source: peer_id,
                message_id: id,
                message,
            })) => {
                handle_gossipsub_message(peer_id, id, message, blockchain).await;
            }
            SwarmEvent::Behaviour(P2PBehaviourEvent::RequestResponse(event)) => {
                handle_request_response(event, swarm, blockchain).await;
            }
            SwarmEvent::NewListenAddr { address, .. } => {
                info!("Local node is listening on {address}");
            }
            _ => {}
        }
    }

    fn handle_mdns_discovered(swarm: &mut Swarm<P2PBehaviour>, list: Vec<(PeerId, Multiaddr)>) {
        for (peer_id, _multiaddr) in list {
            info!("mDNS discovered a new peer: {peer_id}");
            swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
        }
    }

    fn handle_mdns_expired(swarm: &mut Swarm<P2PBehaviour>, list: Vec<(PeerId, Multiaddr)>) {
        for (peer_id, _multiaddr) in list {
            info!("mDNS discover peer has expired: {peer_id}");
            swarm
                .behaviour_mut()
                .gossipsub
                .remove_explicit_peer(&peer_id);
        }
    }
}
