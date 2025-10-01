use futures::stream::StreamExt;
use libp2p::{
    gossipsub, kad, mdns, noise, request_response,
    swarm::{SwarmEvent},
    tcp, yamux, PeerId, Swarm,
};
use std::collections::HashMap as StdHashMap;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    time::Duration,
};
use anyhow::Result;
use log::{info, warn, debug};
use tokio::sync::{mpsc, watch};
use once_cell::sync::OnceCell;

use protocol::libp2p_constants::BEEMESH_CLUSTER;

mod request_response_codec;
pub use request_response_codec::{ApplyCodec, HandshakeCodec};

use crate::libp2p_beemesh::{behaviour::{MyBehaviour, MyBehaviourEvent}, control::Libp2pControl};

pub mod control;
mod behaviour;
pub mod dht_manager;
pub mod dht_helpers;
pub mod security;
#[allow(dead_code)]
pub mod dht_usage_example;

// Handshake state used by the handshake behaviour handlers
#[derive(Debug)]
pub struct HandshakeState {
    pub attempts: u8,
    pub last_attempt: tokio::time::Instant,
    pub confirmed: bool,
}

pub fn setup_libp2p_node()
    -> Result<(
        Swarm<MyBehaviour>,
        gossipsub::IdentTopic,
        watch::Receiver<Vec<String>>,
        watch::Sender<Vec<String>>,
    )> {
    let mut swarm = libp2p::SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_tcp(tcp::Config::default(), noise::Config::new, || {
            yamux::Config::default()
        })?
        .with_quic()
        .with_behaviour(|key| {
            info!("Local PeerId: {}", key.public().to_peer_id());
            let message_id_fn = |message: &gossipsub::Message| {
                let mut s = DefaultHasher::new();
                message.data.hash(&mut s);
                gossipsub::MessageId::from(s.finish().to_string())
            };
            let gossipsub_config = gossipsub::ConfigBuilder::default()
                .heartbeat_interval(Duration::from_secs(10))
                // require validation so we have an opportunity to reject invalid messages early
                .validation_mode(gossipsub::ValidationMode::Strict)
                .mesh_n_low(0) // don’t try to maintain minimum peers
                .mesh_n(0) // target mesh size = 0
                .mesh_n_high(0)
                .message_id_fn(message_id_fn)
                .allow_self_origin(true)
                .build()
                .map_err(|e| std::io::Error::other(e))?;
            let gossipsub = gossipsub::Behaviour::new(
                gossipsub::MessageAuthenticity::Signed(key.clone()),
                gossipsub_config,
            )?;
            let mdns =
                mdns::tokio::Behaviour::new(mdns::Config::default(), key.public().to_peer_id())?;

            // Create the request-response behavior for apply protocol
            let apply_rr = request_response::Behaviour::new(
                std::iter::once((
                    "/beemesh/apply/1.0.0",
                    request_response::ProtocolSupport::Full,
                )),
                request_response::Config::default(),
            );

            // Create the request-response behavior for handshake protocol
            let handshake_rr = request_response::Behaviour::new(
                std::iter::once((
                    "/beemesh/handshake/1.0.0",
                    request_response::ProtocolSupport::Full,
                )),
                request_response::Config::default(),
            );

            // Create the request-response behavior for key-share protocol
            let keyshare_rr = request_response::Behaviour::new(
                std::iter::once((
                    "/beemesh/keyshare/1.0.0",
                    request_response::ProtocolSupport::Full,
                )),
                request_response::Config::default(),
            );

            // Create Kademlia DHT behavior
            let store = kad::store::MemoryStore::new(key.public().to_peer_id());
            let kademlia = kad::Behaviour::new(key.public().to_peer_id(), store);

            Ok(MyBehaviour {
                gossipsub,
                mdns,
                apply_rr,
                handshake_rr,
                keyshare_rr,
                kademlia,
            })
        })?
        .build();

    let topic = gossipsub::IdentTopic::new(BEEMESH_CLUSTER);
    info!("Subscribing to topic: {}", topic.hash());
    swarm.behaviour_mut().gossipsub.subscribe(&topic)?;
    // Ensure local host is an explicit mesh peer for the topic so publish() finds at least one subscriber
    let local_peer = swarm.local_peer_id().clone();
    swarm
        .behaviour_mut()
        .gossipsub
        .add_explicit_peer(&local_peer);

    swarm.listen_on("/ip4/0.0.0.0/udp/0/quic-v1".parse()?)?;
    swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;

    let (peer_tx, peer_rx) = watch::channel(Vec::new());
    info!("Libp2p gossip node started. Listening for messages...");
    Ok((swarm, topic, peer_rx, peer_tx))
}

// Global node keypair set at startup by machine::main
pub static NODE_KEYPAIR: OnceCell<Option<(Vec<u8>, Vec<u8>)>> = OnceCell::new();

pub fn set_node_keypair(pair: Option<(Vec<u8>, Vec<u8>)>) {
    let _ = NODE_KEYPAIR.set(pair);
}

pub async fn start_libp2p_node(
    mut swarm: Swarm<MyBehaviour>,
    topic: gossipsub::IdentTopic,
    peer_tx: watch::Sender<Vec<String>>,
    mut control_rx: mpsc::UnboundedReceiver<Libp2pControl>,
) -> Result<()> {
    use std::collections::HashMap;
    use tokio::time::Instant;

    // pending queries: map request_id -> vec of reply_senders
    let mut pending_queries: StdHashMap<String, Vec<mpsc::UnboundedSender<String>>> =
        StdHashMap::new();

    let mut handshake_states: HashMap<PeerId, HandshakeState> = HashMap::new();
    let mut handshake_interval = tokio::time::interval(Duration::from_secs(1));
    //let mut mesh_alive_interval = tokio::time::interval(Duration::from_secs(1));

    loop {
        tokio::select! {
            // control messages from other parts of the host (REST handlers)
            maybe_msg = control_rx.recv() => {
                if let Some(msg) = maybe_msg {
                    control::handle_control_message(msg, &mut swarm, &topic, &mut pending_queries).await;
                    } else {
                    // sender was dropped, exit loop
                    info!("control channel closed");
                    break;
                }
            }
            event = swarm.select_next_some() => {
                match event {
                    SwarmEvent::Behaviour(MyBehaviourEvent::HandshakeRr(request_response::Event::Message { message, peer, connection_id: _ })) => {
                        behaviour::handshake_message_event(message, peer, &mut swarm, &mut handshake_states);
                    }
                    SwarmEvent::Behaviour(MyBehaviourEvent::HandshakeRr(request_response::Event::OutboundFailure { peer, error, .. })) => {
                        behaviour::handshake_outbound_failure(peer, error);
                    }
                    SwarmEvent::Behaviour(MyBehaviourEvent::HandshakeRr(request_response::Event::InboundFailure { peer, error, .. })) => {
                        behaviour::handshake_inbound_failure(peer, error);
                    }
                    SwarmEvent::Behaviour(MyBehaviourEvent::ApplyRr(request_response::Event::Message { message, peer, connection_id: _ })) => {
                        let local_peer = *swarm.local_peer_id();
                        behaviour::apply_message(message, peer, &mut swarm, local_peer);
                    }
                    SwarmEvent::Behaviour(MyBehaviourEvent::ApplyRr(request_response::Event::OutboundFailure { peer, error, .. })) => {
                        behaviour::apply_outbound_failure(peer, error);
                    }
                    SwarmEvent::Behaviour(MyBehaviourEvent::ApplyRr(request_response::Event::InboundFailure { peer, error, .. })) => {
                        behaviour::apply_inbound_failure(peer, error);
                    }
                    SwarmEvent::Behaviour(MyBehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
                        behaviour::mdns_discovered(list, &mut swarm, &mut handshake_states, &peer_tx);
                    }
                    SwarmEvent::Behaviour(MyBehaviourEvent::Mdns(mdns::Event::Expired(list))) => {
                        behaviour::mdns_expired(list, &mut swarm, &mut handshake_states, &peer_tx);
                    }
                    SwarmEvent::Behaviour(MyBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                        propagation_source: peer_id,
                        message_id: _id,
                        message,
                    })) => {
                        behaviour::gossipsub_message(peer_id, message, topic.hash().clone(), &mut swarm, &mut pending_queries);
                    }
                    SwarmEvent::Behaviour(MyBehaviourEvent::Gossipsub(gossipsub::Event::Subscribed { peer_id, topic })) => {
                        behaviour::gossipsub_subscribed(peer_id, topic);
                    }
                    SwarmEvent::Behaviour(MyBehaviourEvent::Gossipsub(gossipsub::Event::Unsubscribed { peer_id, topic })) => {
                        behaviour::gossipsub_unsubscribed(peer_id, topic);
                    }
                    SwarmEvent::Behaviour(MyBehaviourEvent::Kademlia(event)) => {
                        behaviour::kademlia_event(event, None);
                    }
                    SwarmEvent::Behaviour(MyBehaviourEvent::KeyshareRr(request_response::Event::Message { message, peer, connection_id: _ })) => {
                        let local_peer = *swarm.local_peer_id();
                        behaviour::keyshare_message(message, peer, &mut swarm, local_peer);
                    }
                    SwarmEvent::NewListenAddr { address, .. } => {
                        info!("Local node is listening on {address}");
                    }
                    _ => {}
                }
            }
            _ = handshake_interval.tick() => {
                let mut to_remove = Vec::new();
                for (peer_id, state) in handshake_states.iter_mut() {
                    if state.confirmed {
                        continue;
                    }
                    if state.attempts >= 3 {
                        warn!("Removing non-responsive peer: {peer_id}");
                        swarm
                            .behaviour_mut()
                            .gossipsub
                            .remove_explicit_peer(peer_id);
                        to_remove.push(peer_id.clone());
                        continue;
                    }
                    if state.last_attempt.elapsed() >= Duration::from_secs(2) {
                        // Send handshake request using request-response protocol with FlatBuffer
                        let local_peer_id = swarm.local_peer_id().to_string();
                        let handshake_signature = format!("{}-{}", local_peer_id, state.attempts + 1);
                        let handshake_request = protocol::machine::build_handshake(0, 0, "TODO", &handshake_signature);

                        let request_id = swarm.behaviour_mut().handshake_rr.send_request(peer_id, handshake_request);
            debug!("libp2p: sent handshake request to peer={} request_id={:?} attempt={}",
                peer_id, request_id, state.attempts + 1);

                        state.attempts += 1;
                        state.last_attempt = Instant::now();
                    }
                }
                for peer_id in to_remove {
                    handshake_states.remove(&peer_id);
                }
                // Update peer list in channel after handshake changes
                let all_peers: Vec<String> = swarm.behaviour().gossipsub.all_peers().map(|(p, _topics)| p.to_string()).collect();
                let _ = peer_tx.send(all_peers);
            }
            /*_ = mesh_alive_interval.tick() => {
                // Periodically publish a 'mesh-alive' message to the topic
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis();
                let mesh_alive_msg = format!("mesh-alive-{}-{}-{}", swarm.local_peer_id(), now);
                let res = swarm.behaviour_mut().gossipsub.publish(topic.clone(), mesh_alive_msg.as_bytes());
            }*/
        }
    }

    Ok(())
}