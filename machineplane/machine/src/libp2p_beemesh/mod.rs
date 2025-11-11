use anyhow::Result;

use futures::stream::StreamExt;
use libp2p::{
    PeerId, Swarm, gossipsub, kad, multiaddr::Multiaddr, multiaddr::Protocol, request_response,
    swarm::SwarmEvent,
};
use libp2p::{autonat, identify, relay};
use log::{debug, info, warn};
use once_cell::sync::OnceCell;
use std::collections::HashMap as StdHashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    time::Duration,
};
use tokio::sync::{mpsc, watch};

use protocol::libp2p_constants::SCHEDULER_PROPOSALS_TOPIC;

// Global control sender for distributed operations
static CONTROL_SENDER: OnceCell<mpsc::UnboundedSender<control::Libp2pControl>> = OnceCell::new();
static DISABLED_SCHEDULING: OnceCell<Mutex<StdHashMap<PeerId, bool>>> = OnceCell::new();

fn scheduling_map() -> &'static Mutex<StdHashMap<PeerId, bool>> {
    DISABLED_SCHEDULING.get_or_init(|| Mutex::new(StdHashMap::new()))
}

fn extract_listen_endpoint(addr: &Multiaddr) -> Option<(String, u16)> {
    let mut host: Option<String> = None;
    let mut port: Option<u16> = None;
    for proto in addr.iter() {
        match proto {
            Protocol::Ip4(ipv4) => host = Some(ipv4.to_string()),
            Protocol::Ip6(ipv6) => host = Some(ipv6.to_string()),
            Protocol::Dns(dns) => host = Some(dns.to_string()),
            Protocol::Dns4(dns) => host = Some(dns.to_string()),
            Protocol::Dns6(dns) => host = Some(dns.to_string()),
            Protocol::Tcp(value) | Protocol::Udp(value) => port = Some(value),
            _ => {}
        }
    }
    host.zip(port)
}

pub fn register_scheduling_preference(peer: PeerId, disabled: bool) {
    let mut map = scheduling_map().lock().unwrap();
    map.insert(peer, disabled);
}

pub fn is_scheduling_disabled_for(peer: &PeerId) -> bool {
    scheduling_map()
        .lock()
        .unwrap()
        .get(peer)
        .copied()
        .unwrap_or(false)
}

mod request_response_codec;
pub use request_response_codec::{ApplyCodec, DeleteCodec, HandshakeCodec};

use crate::libp2p_beemesh::{
    behaviour::{MyBehaviour, MyBehaviourEvent},
    control::Libp2pControl,
};

pub mod behaviour;
pub mod capacity;
pub mod control;
pub mod envelope;
pub mod reply;
pub mod security;
pub mod utils;

// Handshake state used by the handshake behaviour handlers
#[derive(Debug)]
pub struct HandshakeState {
    pub attempts: u8,
    pub last_attempt: tokio::time::Instant,
    pub confirmed: bool,
}

pub fn setup_libp2p_node(
    quic_port: u16,
    host: &str,
    disable_scheduling: bool,
) -> Result<(
    Swarm<MyBehaviour>,
    gossipsub::IdentTopic,
    watch::Receiver<Vec<String>>,
    watch::Sender<Vec<String>>,
)> {
    let mut swarm = libp2p::SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_quic()
        .with_dns()?
        .with_behaviour(|key| {
            debug!("Local PeerId: {}", key.public().to_peer_id());
            let message_id_fn = |message: &gossipsub::Message| {
                let mut s = DefaultHasher::new();
                message.data.hash(&mut s);
                gossipsub::MessageId::from(s.finish().to_string())
            };
            let gossipsub_config = gossipsub::ConfigBuilder::default()
                .heartbeat_interval(Duration::from_secs(10))
                // require validation so we have an opportunity to reject invalid messages early
                .validation_mode(gossipsub::ValidationMode::Strict)
                .mesh_n_low(1) // minimum peers in mesh
                .mesh_n(3) // target mesh size
                .mesh_n_high(6) // maximum peers in mesh
                .mesh_outbound_min(1) // minimum outbound connections
                .message_id_fn(message_id_fn)
                .allow_self_origin(true)
                .build()
                .map_err(|e| std::io::Error::other(e))?;
            let gossipsub = gossipsub::Behaviour::new(
                gossipsub::MessageAuthenticity::Signed(key.clone()),
                gossipsub_config,
            )?;

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

            // Create the request-response behavior for scheduler (capacity/proposals)
            let scheduler_rr = request_response::Behaviour::new(
                std::iter::once((
                    "/beemesh/scheduler-tasks/1.0.0",
                    request_response::ProtocolSupport::Full,
                )),
                request_response::Config::default(),
            );

            // Create the request-response behavior for delete protocol
            let delete_rr = request_response::Behaviour::new(
                std::iter::once((
                    "/beemesh/delete/1.0.0",
                    request_response::ProtocolSupport::Full,
                )),
                request_response::Config::default(),
            );

            // Create the request-response behavior for manifest fetch protocol
            let manifest_fetch_rr = request_response::Behaviour::new(
                std::iter::once((
                    "/beemesh/manifest-fetch/1.0.0",
                    request_response::ProtocolSupport::Full,
                )),
                request_response::Config::default(),
            );

            // Create Kademlia DHT behavior with configuration suitable for small networks
            let store = kad::store::MemoryStore::new(key.public().to_peer_id());
            let mut kademlia_config = kad::Config::default();

            kademlia_config.set_replication_factor(std::num::NonZeroUsize::new(1).unwrap()); // Minimum replication
            kademlia_config.set_max_packet_size(1024 * 1024); // Allow larger packets

            // Configure timeouts and parallelism for small networks
            kademlia_config.set_parallelism(std::num::NonZeroUsize::new(3).unwrap()); // Increase parallelism for local tests
            kademlia_config.set_query_timeout(std::time::Duration::from_secs(15)); // Longer timeout for local tests

            // Configure provider record settings for better local discovery
            kademlia_config.set_provider_record_ttl(Some(std::time::Duration::from_secs(30))); // Shorter TTL for local tests
            kademlia_config
                .set_provider_publication_interval(Some(std::time::Duration::from_secs(5))); // More frequent republishing

            let kademlia =
                kad::Behaviour::with_config(key.public().to_peer_id(), store, kademlia_config);

            let relay = relay::Behaviour::new(key.public().to_peer_id(), Default::default());
            let autonat = autonat::Behaviour::new(key.public().to_peer_id(), Default::default());
            let identify = identify::Behaviour::new(identify::Config::new(
                "/beemesh/0.1.0".into(), // protocol version
                key.public(),
            ));

            Ok(MyBehaviour {
                gossipsub,
                apply_rr,
                handshake_rr,
                scheduler_rr,
                delete_rr,
                manifest_fetch_rr,
                kademlia,
                relay,
                autonat,
                identify,
            })
        })?
        .build();

    register_scheduling_preference(swarm.local_peer_id().clone(), disable_scheduling);

    let topic = gossipsub::IdentTopic::new(SCHEDULER_PROPOSALS_TOPIC);
    debug!("Subscribing to scheduler proposals topic: {}", topic.hash());
    swarm.behaviour_mut().gossipsub.subscribe(&topic)?;
    // Ensure local host is an explicit mesh peer for the topic so publish() finds at least one subscriber
    let local_peer = swarm.local_peer_id().clone();
    swarm
        .behaviour_mut()
        .gossipsub
        .add_explicit_peer(&local_peer);

    let listen_addr: Multiaddr = match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ipv4)) => {
            let mut addr = Multiaddr::empty();
            addr.push(Protocol::Ip4(ipv4));
            addr.push(Protocol::Udp(quic_port));
            addr.push(Protocol::QuicV1);
            addr
        }
        Ok(IpAddr::V6(ipv6)) => {
            let mut addr = Multiaddr::empty();
            addr.push(Protocol::Ip6(ipv6));
            addr.push(Protocol::Udp(quic_port));
            addr.push(Protocol::QuicV1);
            addr
        }
        Err(_) => {
            debug!(
                "libp2p host '{}' is not an IP literal; falling back to IPv4 multiaddr string",
                host
            );
            format!("/ip4/{}/udp/{}/quic-v1", host, quic_port).parse()?
        }
    };

    swarm.listen_on(listen_addr)?;

    let (peer_tx, peer_rx) = watch::channel(Vec::new());
    Ok((swarm, topic, peer_rx, peer_tx))
}

// Global node keypair set at startup by machine::main
pub static NODE_KEYPAIR: OnceCell<Option<(Vec<u8>, Vec<u8>)>> = OnceCell::new();

pub fn set_node_keypair(pair: Option<(Vec<u8>, Vec<u8>)>) {
    let _ = NODE_KEYPAIR.set(pair);
}

/// Set the global control sender for distributed operations
pub fn set_control_sender(sender: mpsc::UnboundedSender<control::Libp2pControl>) {
    let _ = CONTROL_SENDER.set(sender);
}

/// Get the global control sender for distributed operations
pub fn get_control_sender() -> Option<&'static mpsc::UnboundedSender<control::Libp2pControl>> {
    CONTROL_SENDER.get()
}

/// Set whether scheduler request handling is disabled for this node
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
    let mut renew_interval = tokio::time::interval(Duration::from_millis(500));
    let mut cleanup_interval = tokio::time::interval(Duration::from_secs(2));
    //let mut mesh_alive_interval = tokio::time::interval(Duration::from_secs(1));

    loop {
        tokio::select! {
            // control messages from other parts of the host (REST handlers)
            maybe_msg = control_rx.recv() => {
                if let Some(msg) = maybe_msg {
                    control::handle_control_message(msg, &mut swarm, &topic, &mut pending_queries).await;
                } else {
                    // sender was dropped, withdraw provider announces and exit loop
                    info!("control channel closed; withdrawing provider announcements");
                    control::withdraw_all_providers(&mut swarm);
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
                        // Create a future for the workload manager handler and drive it immediately
                        let future = crate::workload_integration::handle_apply_message_with_workload_manager(
                            message, peer, &mut swarm, local_peer
                        );
                        // Since we're in an async context, we can await this directly
                        future.await;
                    }
                    SwarmEvent::Behaviour(MyBehaviourEvent::ApplyRr(request_response::Event::OutboundFailure { peer, error, .. })) => {
                        behaviour::apply_outbound_failure(peer, error);
                    }
                    SwarmEvent::Behaviour(MyBehaviourEvent::ApplyRr(request_response::Event::InboundFailure { peer, error, .. })) => {
                        behaviour::apply_inbound_failure(peer, error);
                    }

                    SwarmEvent::Behaviour(MyBehaviourEvent::DeleteRr(request_response::Event::Message { message, peer, connection_id: _ })) => {
                        let local_peer = *swarm.local_peer_id();
                        behaviour::delete_message(message, peer, &mut swarm, local_peer);
                    }
                    SwarmEvent::Behaviour(MyBehaviourEvent::DeleteRr(request_response::Event::OutboundFailure { peer, error, .. })) => {
                        behaviour::delete_outbound_failure(peer, error);
                    }
                    SwarmEvent::Behaviour(MyBehaviourEvent::DeleteRr(request_response::Event::InboundFailure { peer, error, .. })) => {
                        behaviour::delete_inbound_failure(peer, error);
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

                    SwarmEvent::Behaviour(MyBehaviourEvent::SchedulerRr(request_response::Event::Message { message, peer, connection_id: _ })) => {
                        let local_peer = *swarm.local_peer_id();
                        behaviour::scheduler_message(message, peer, &mut swarm, local_peer, &mut pending_queries);
                    }

                    SwarmEvent::Behaviour(MyBehaviourEvent::ManifestFetchRr(request_response::Event::OutboundFailure { peer, request_id, error, connection_id: _ })) => {
                        warn!("libp2p: manifest fetch outbound failure for peer {}: {:?}", peer, error);
                        // Handle failed manifest distribution requests
                        let reply_sender = {
                            let mut pending_requests = crate::libp2p_beemesh::control::get_pending_manifest_requests()
                                .lock()
                                .unwrap();
                            pending_requests
                                .remove(&request_id)
                                .map(|(sender, _)| sender)
                        };
                        if let Some(reply_tx) = reply_sender {
                            let _ = reply_tx.send(Err(format!("Outbound failure: {:?}", error)));
                        }
                    }
                    SwarmEvent::Behaviour(MyBehaviourEvent::ManifestFetchRr(request_response::Event::InboundFailure { peer, error, .. })) => {
                        warn!("libp2p: manifest fetch inbound failure for peer {}: {:?}", peer, error);
                    }
                    SwarmEvent::ConnectionEstablished { peer_id, connection_id: _, endpoint, num_established: _, concurrent_dial_errors: _, established_in: _ } => {
                        info!("DHT: Connection established with peer {}, adding to Kademlia", peer_id);
                        // Add the connected peer to Kademlia DHT for provider announcements
                        // Use the connection endpoint address for Kademlia
                        let addr = endpoint.get_remote_address();
                        swarm.behaviour_mut().kademlia.add_address(&peer_id, addr.clone());

                        // Bootstrap DHT after establishing connections to improve local test reliability
                        let connected_peers: Vec<_> = swarm.connected_peers().cloned().collect();
                        if connected_peers.len() >= 2 {
                            info!("DHT: Bootstrapping with {} connected peers", connected_peers.len());
                            let _ = swarm.behaviour_mut().kademlia.bootstrap();
                        }
                    }
                    SwarmEvent::ConnectionClosed { peer_id, connection_id: _, endpoint: _, num_established, cause: _ } => {
                        if num_established == 0 {
                            info!("DHT: All connections to peer {} closed, removing from Kademlia", peer_id);
                            swarm.behaviour_mut().kademlia.remove_peer(&peer_id);
                        }
                    }
                    SwarmEvent::NewListenAddr { address, .. } => {
                        if let Some((host, port)) = extract_listen_endpoint(&address) {
                            info!("libp2p: listening on {}:{}", host, port);
                        } else {
                            info!("libp2p: listening on {address}");
                        }
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
                        let timestamp = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64;
                        let nonce = rand::random::<u32>();

                        // Generate proper cryptographic signature for handshake request
                        match crypto::ensure_keypair_on_disk() {
                            Ok((pub_bytes, sk_bytes)) => {
                                let protocol_version = "beemesh/1.0";
                                let local_peer_id = swarm.local_peer_id().to_string();


                                // Build simple handshake request
                                let handshake_request = protocol::machine::build_handshake(
                                    nonce,
                                    timestamp,
                                    protocol_version,
                                    &local_peer_id,
                                );

                                // Create nonce for envelope
                                let envelope_nonce = format!("handshake_req_{}", nonce);

                                // Build canonical envelope bytes
                                let canonical_bytes = protocol::machine::build_envelope_canonical(
                                    &handshake_request,
                                    "handshake",
                                    &envelope_nonce,
                                    timestamp,
                                    "ml-dsa-65",
                                    None,
                                );

                                match crypto::sign_envelope(&sk_bytes, &pub_bytes, &canonical_bytes) {
                                    Ok((sig_b64, pub_b64)) => {
                                        // Create signed envelope
                                        let signed_envelope = protocol::machine::build_envelope_signed(
                                            &handshake_request,
                                            "handshake",
                                            &envelope_nonce,
                                            timestamp,
                                            "ml-dsa-65",
                                            "ml-dsa-65",
                                            &sig_b64,
                                            &pub_b64,
                                            None,
                                        );

                                        let request_id = swarm.behaviour_mut().handshake_rr.send_request(peer_id, signed_envelope);
                                        debug!("libp2p: sent handshake request to peer={} request_id={:?} attempt={}",
                                            peer_id, request_id, state.attempts + 1);

                                        state.attempts += 1;
                                        state.last_attempt = Instant::now();
                                    }
                                    Err(e) => {
                                        warn!("failed to sign handshake request for peer {}: {:?}", peer_id, e);
                                    }
                                }
                            }
                            Err(e) => {
                                warn!("failed to load keypair for handshake request to peer {}: {:?}", peer_id, e);
                            }
                        }
                    }
                }
                for peer_id in to_remove {
                    handshake_states.remove(&peer_id);
                }
                // Update peer list in channel after handshake changes
                let all_peers: Vec<String> = swarm.behaviour().gossipsub.all_peers().map(|(p, _topics)| p.to_string()).collect();
                let _ = peer_tx.send(all_peers);
            }
            _ = renew_interval.tick() => {
                // Drain any enqueued control messages produced by behaviours (e.g. AnnounceProvider)
                control::drain_enqueued_controls(&mut swarm, &topic, &mut pending_queries).await;
                // renew any due provider announcements
                control::renew_due_providers(&mut swarm);
            }
            _ = cleanup_interval.tick() => {
                // Clean up timed-out manifest distribution requests
                control::cleanup_timed_out_manifest_requests();
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
