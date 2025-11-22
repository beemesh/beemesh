use anyhow::Result;
use async_trait::async_trait;
use futures::{AsyncReadExt, AsyncWriteExt, stream::StreamExt};
use libp2p::{
    PeerId, Swarm, autonat, gossipsub, identify, identity::Keypair, kad, multiaddr::Multiaddr,
    multiaddr::Protocol, relay, request_response, swarm::SwarmEvent,
};
use log::{debug, error, info, warn};
use once_cell::sync::{Lazy, OnceCell};
use std::net::IpAddr;
use std::sync::Mutex;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, watch};

use crate::messages::libp2p_constants::{
    BEEMESH_FABRIC, SCHEDULER_EVENTS, SCHEDULER_PROPOSALS, SCHEDULER_TENDERS,
};
use crate::scheduler::SchedulerCommand;

// Flattened modules
pub mod behaviour;
pub mod control;
pub mod dht_manager;
pub mod utils;

use behaviour::{MyBehaviour, MyBehaviourEvent};
use control::Libp2pControl;

// Global control sender for distributed operations
static CONTROL_SENDER: OnceCell<mpsc::UnboundedSender<control::Libp2pControl>> = OnceCell::new();
static NODE_KEYPAIR: Lazy<Mutex<Option<(Vec<u8>, Vec<u8>)>>> = Lazy::new(|| Mutex::new(None));

#[derive(Clone, Copy)]
pub struct ByteProtocol(&'static str);

impl AsRef<str> for ByteProtocol {
    fn as_ref(&self) -> &str {
        self.0
    }
}

#[derive(Clone, Default)]
pub struct ByteCodec;

fn ensure_handshake_state<'a>(
    peer: &PeerId,
    handshake_states: &'a mut std::collections::HashMap<PeerId, HandshakeState>,
) -> &'a mut HandshakeState {
    handshake_states.entry(*peer).or_insert(HandshakeState {
        attempts: 0,
        last_attempt: Instant::now() - Duration::from_secs(3),
        confirmed: false,
    })
}

#[async_trait]
impl request_response::Codec for ByteCodec {
    type Protocol = ByteProtocol;
    type Request = Vec<u8>;
    type Response = Vec<u8>;

    async fn read_request<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
    ) -> std::io::Result<Self::Request>
    where
        T: futures::AsyncRead + Unpin + Send,
    {
        let mut buf = Vec::new();
        io.read_to_end(&mut buf).await?;
        Ok(buf)
    }

    async fn read_response<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
    ) -> std::io::Result<Self::Response>
    where
        T: futures::AsyncRead + Unpin + Send,
    {
        let mut buf = Vec::new();
        io.read_to_end(&mut buf).await?;
        Ok(buf)
    }

    async fn write_request<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
        req: Self::Request,
    ) -> std::io::Result<()>
    where
        T: futures::AsyncWrite + Unpin + Send,
    {
        io.write_all(&req).await?;
        io.close().await
    }

    async fn write_response<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
        res: Self::Response,
    ) -> std::io::Result<()>
    where
        T: futures::AsyncWrite + Unpin + Send,
    {
        io.write_all(&res).await?;
        io.close().await
    }
}

// Protocol definitions
pub type ApplyCodec = ByteCodec;
pub type HandshakeCodec = ByteCodec;
pub type DeleteCodec = ByteCodec;

#[derive(Debug, Clone)]
struct HandshakeState {
    attempts: u32,
    last_attempt: std::time::Instant,
    confirmed: bool,
}

pub fn get_node_keypair() -> Option<(Vec<u8>, Vec<u8>)> {
    NODE_KEYPAIR.lock().unwrap().clone()
}

pub fn set_node_keypair(pair: Option<(Vec<u8>, Vec<u8>)>) {
    let mut slot = NODE_KEYPAIR.lock().unwrap();
    *slot = pair;
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

/// Set the global control sender for distributed operations
pub fn set_control_sender(sender: mpsc::UnboundedSender<control::Libp2pControl>) {
    let _ = CONTROL_SENDER.set(sender);
}

/// Get the global control sender for distributed operations
pub fn get_control_sender() -> Option<&'static mpsc::UnboundedSender<control::Libp2pControl>> {
    CONTROL_SENDER.get()
}

pub fn setup_libp2p_node(
    quic_port: u16,
    host: &str,
) -> Result<(
    Swarm<MyBehaviour>,
    gossipsub::IdentTopic,
    watch::Receiver<Vec<String>>,
    watch::Sender<Vec<String>>,
)> {
    // Load the node keypair (set by lib.rs startup) or generate a new one
    let keypair = {
        let slot = NODE_KEYPAIR.lock().unwrap();
        if let Some((_pk, sk)) = &*slot {
            libp2p::identity::Keypair::from_protobuf_encoding(sk)?
        } else {
            libp2p::identity::Keypair::generate_ed25519()
        }
    };

    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(keypair)
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
                // Use permissive validation so locally-published scheduler tenders are always
                // delivered even when a validator is not registered (e.g. in tests).
                .validation_mode(gossipsub::ValidationMode::Permissive)
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
                    ByteProtocol("/beemesh/apply/1.0.0"),
                    request_response::ProtocolSupport::Full,
                )),
                request_response::Config::default(),
            );

            // Create the request-response behavior for handshake protocol
            let handshake_rr = request_response::Behaviour::new(
                std::iter::once((
                    ByteProtocol("/beemesh/handshake/1.0.0"),
                    request_response::ProtocolSupport::Full,
                )),
                request_response::Config::default(),
            );

            // Create the request-response behavior for delete protocol
            let delete_rr = request_response::Behaviour::new(
                std::iter::once((
                    ByteProtocol("/beemesh/delete/1.0.0"),
                    request_response::ProtocolSupport::Full,
                )),
                request_response::Config::default(),
            );

            // Create the request-response behavior for manifest fetch protocol
            let manifest_fetch_rr = request_response::Behaviour::new(
                std::iter::once((
                    ByteProtocol("/beemesh/manifest-fetch/1.0.0"),
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

            // Configure placement record settings (via Kademlia providers) for better local discovery
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
                delete_rr,
                manifest_fetch_rr,
                kademlia,
                relay,
                autonat,
                identify,
            })
        })?
        .build();

    let topic = gossipsub::IdentTopic::new(BEEMESH_FABRIC);
    let tenders_topic = gossipsub::IdentTopic::new(SCHEDULER_TENDERS);
    let proposals_topic = gossipsub::IdentTopic::new(SCHEDULER_PROPOSALS);
    let events_topic = gossipsub::IdentTopic::new(SCHEDULER_EVENTS);

    debug!(
        "Subscribing to topics: {}, {}, {}, {}",
        topic.hash(),
        tenders_topic.hash(),
        proposals_topic.hash(),
        events_topic.hash()
    );
    swarm.behaviour_mut().gossipsub.subscribe(&topic)?;
    swarm.behaviour_mut().gossipsub.subscribe(&tenders_topic)?;
    swarm
        .behaviour_mut()
        .gossipsub
        .subscribe(&proposals_topic)?;
    swarm.behaviour_mut().gossipsub.subscribe(&events_topic)?;
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

/// Start the libp2p node event loop and process control messages and swarm events.
pub async fn start_libp2p_node(
    mut swarm: Swarm<MyBehaviour>,
    topic: gossipsub::IdentTopic, // Main fabric topic
    peer_tx: watch::Sender<Vec<String>>,
    mut control_rx: mpsc::UnboundedReceiver<Libp2pControl>,
) -> Result<()> {
    use std::collections::HashMap;

    let mut handshake_states: HashMap<PeerId, HandshakeState> = HashMap::new();
    let mut handshake_interval = tokio::time::interval(Duration::from_secs(1));
    let mut renew_interval = tokio::time::interval(Duration::from_millis(500));
    let mut cleanup_interval = tokio::time::interval(Duration::from_secs(2));
    //let mut mesh_alive_interval = tokio::time::interval(Duration::from_secs(1));

    // --- Scheduler Setup ---
    let (sched_input_tx, mut sched_input_rx) =
        mpsc::unbounded_channel::<(gossipsub::TopicHash, gossipsub::Message)>();
    let (sched_output_tx, mut sched_output_rx) = mpsc::unbounded_channel::<SchedulerCommand>();

    // Initialize global input sender for gossipsub_message.rs
    behaviour::SCHEDULER_INPUT_TX.set(sched_input_tx).ok();

    // Spawn Scheduler
    let local_node_id = swarm.local_peer_id().to_string();
    let scheduler_keypair = get_node_keypair()
        .and_then(|(_, sk)| Keypair::from_protobuf_encoding(&sk).ok())
        .unwrap_or_else(|| {
            warn!("Node keypair unavailable; generating ephemeral scheduler keypair");
            Keypair::generate_ed25519()
        });

    let scheduler =
        crate::scheduler::Scheduler::new(local_node_id.clone(), scheduler_keypair, sched_output_tx);
    let scheduler = std::sync::Arc::new(scheduler);

    tokio::spawn(async move {
        while let Some((topic, msg)) = sched_input_rx.recv().await {
            scheduler.handle_message(&topic, &msg).await;
        }
    });
    // -----------------------

    loop {
        tokio::select! {
            // Handle scheduler output (bids/leases to publish)
            Some(command) = sched_output_rx.recv() => {
                     match command {
                         SchedulerCommand::Publish { topic, payload } => {
                             let topic = gossipsub::IdentTopic::new(topic);
                             if let Err(e) = swarm.behaviour_mut().gossipsub.publish(topic, payload) {
                                 log::error!("Failed to publish scheduler message: {}", e);
                             }
                         }
                         SchedulerCommand::PutDHT { key, value } => {
                             let record = kad::Record {
                                 key: kad::RecordKey::new(&key),
                                 value,
                                 publisher: None,
                                 expires: None,
                             };
                             if let Err(e) = swarm.behaviour_mut().kademlia.put_record(record, kad::Quorum::One) {
                                 log::error!("Failed to put DHT record: {:?}", e);
                             }
                         }
                         SchedulerCommand::DeployWorkload { manifest_id, manifest_json, replicas } => {
                            let apply_req = crate::messages::types::ApplyRequest {
                                replicas,
                                operation_id: format!("sched-deploy-{}", uuid::Uuid::new_v4()),
                                manifest_json: manifest_json.clone(),
                                origin_peer: local_node_id.clone(),
                                manifest_id: manifest_id.clone(),
                                signature: Vec::new(),
                            };
                             // We use an empty owner_pubkey for now as we trust the scheduler's decision
                             // In a real scenario, we should pass the owner_pubkey from the Tender/Manifest
                             let owner_pubkey = Vec::new();

                             if let Err(e) = crate::scheduler::process_manifest_deployment(
                                 &mut swarm,
                                 &apply_req,
                                 &manifest_json,
                                 &owner_pubkey,
                             ).await {
                                 log::error!("Failed to deploy workload from scheduler: {}", e);
                             }
                         }
                     }
            }
            // control messages from other parts of the host (REST handlers)
            maybe_msg = control_rx.recv() => {
                if let Some(msg) = maybe_msg {
                    control::handle_control_message(msg, &mut swarm).await;
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
                        match message {
                            request_response::Message::Request { request, channel, .. } => {
                                let response = match crate::messages::machine::decode_handshake(&request) {
                                    Ok(_) => {
                                        ensure_handshake_state(&peer, &mut handshake_states).confirmed = true;
                                        let timestamp = std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_millis() as u64;
                                        crate::messages::machine::build_handshake(
                                            rand::random::<u32>(),
                                            timestamp,
                                            "beemesh/1.0",
                                            &peer.to_string(),
                                        )
                                    }
                                    Err(e) => {
                                        log::error!("libp2p: failed to parse handshake request: {:?}", e);
                                        crate::messages::machine::build_handshake(0, 0, "", "")
                                    }
                                };

                                let _ = swarm
                                    .behaviour_mut()
                                    .handshake_rr
                                    .send_response(channel, response);
                            }
                            request_response::Message::Response { response, .. } => {
                                match crate::messages::machine::decode_handshake(&response) {
                                    Ok(_) => {
                                        ensure_handshake_state(&peer, &mut handshake_states).confirmed = true;
                                    }
                                    Err(e) => {
                                        log::error!("libp2p: failed to parse handshake response: {:?}", e);
                                    }
                                }
                            }
                        }
                    }
                    SwarmEvent::Behaviour(MyBehaviourEvent::HandshakeRr(request_response::Event::OutboundFailure { peer, error, .. })) => {
                        warn!("handshake outbound failure with peer {}: {:?}", peer, error);
                    }
                    SwarmEvent::Behaviour(MyBehaviourEvent::HandshakeRr(request_response::Event::InboundFailure { peer, error, .. })) => {
                        warn!("handshake inbound failure with peer {}: {:?}", peer, error);
                    }
                    SwarmEvent::Behaviour(MyBehaviourEvent::ApplyRr(request_response::Event::Message { message, peer, connection_id: _ })) => {
                        let local_peer = *swarm.local_peer_id();
                        // Create a future for the workload manager handler and drive it immediately
                        let future = crate::scheduler::handle_apply_message_with_podman_manager(
                            message, peer, &mut swarm, local_peer
                        );
                        // Since we're in an async context, we can await this directly
                        future.await;
                    }
                    SwarmEvent::Behaviour(MyBehaviourEvent::ApplyRr(request_response::Event::OutboundFailure { peer, error, .. })) => {
                        warn!("apply outbound failure with peer {}: {:?}", peer, error);
                    }
                    SwarmEvent::Behaviour(MyBehaviourEvent::ApplyRr(request_response::Event::InboundFailure { peer, error, .. })) => {
                        warn!("apply inbound failure with peer {}: {:?}", peer, error);
                    }

                    SwarmEvent::Behaviour(MyBehaviourEvent::DeleteRr(request_response::Event::Message { message, peer, connection_id: _ })) => {
                        match message {
                            request_response::Message::Request { request, channel, .. } => {
                                info!("Received delete request from peer={}", peer);
                                match crate::messages::machine::decode_delete_request(&request) {
                                    Ok(delete_req) => {
                                        info!(
                                            "Delete request - manifest_id={:?} operation_id={:?} force={}",
                                            &delete_req.manifest_id, &delete_req.operation_id, delete_req.force
                                        );

                                        let manifest_id = delete_req.manifest_id.clone();
                                        let operation_id = delete_req.operation_id.clone();
                                        let force = delete_req.force;
                                        let requesting_peer = peer.to_string();

                                        tokio::spawn(async move {
                                            let (success, message, removed_workloads) =
                                                process_delete_request(&manifest_id, force, &requesting_peer).await;

                                            let _response = crate::messages::machine::build_delete_response(
                                                success,
                                                &operation_id,
                                                &message,
                                                &manifest_id,
                                                &removed_workloads,
                                            );

                                            info!(
                                                "Delete request processed: success={} message={} removed_workloads={:?}",
                                                success, message, removed_workloads
                                            );
                                        });

                                        let ack_response = crate::messages::machine::build_delete_response(
                                            true,
                                            delete_req.operation_id.as_str(),
                                            "delete request received and processing",
                                            delete_req.manifest_id.as_str(),
                                            &[],
                                        );
                                        let _ = swarm
                                            .behaviour_mut()
                                            .delete_rr
                                            .send_response(channel, ack_response);
                                    }
                                    Err(e) => {
                                        error!("Failed to parse delete request: {}", e);
                                        let error_response = crate::messages::machine::build_delete_response(
                                            false,
                                            "unknown",
                                            "invalid delete request format",
                                            "unknown",
                                            &[],
                                        );
                                        let _ = swarm
                                            .behaviour_mut()
                                            .delete_rr
                                            .send_response(channel, error_response);
                                    }
                                }
                            }
                            request_response::Message::Response { response, .. } => {
                                info!("Received delete response from peer={}", peer);
                                match crate::messages::machine::decode_delete_response(&response) {
                                    Ok(delete_resp) => {
                                        info!(
                                            "Delete response - ok={} operation_id={:?} message={:?} removed_workloads={:?}",
                                            delete_resp.ok,
                                            &delete_resp.operation_id,
                                            &delete_resp.message,
                                            Some(delete_resp.removed_workloads.clone())
                                        );
                                    }
                                    Err(e) => {
                                        warn!("Failed to parse delete response: {:?}", e);
                                    }
                                }
                            }
                        }
                    }
                    SwarmEvent::Behaviour(MyBehaviourEvent::DeleteRr(request_response::Event::OutboundFailure { peer, error, .. })) => {
                        warn!("delete outbound failure with peer {}: {:?}", peer, error);
                    }
                    SwarmEvent::Behaviour(MyBehaviourEvent::DeleteRr(request_response::Event::InboundFailure { peer, error, .. })) => {
                        warn!("delete inbound failure with peer {}: {:?}", peer, error);
                    }

                    SwarmEvent::Behaviour(MyBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                        propagation_source: peer_id,
                        message_id: _id,
                        message,
                    })) => {
                        behaviour::gossipsub_message(peer_id, message, topic.hash().clone());
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
                    SwarmEvent::Behaviour(MyBehaviourEvent::ManifestFetchRr(request_response::Event::Message { message, peer, connection_id: _ })) => {
                        match message {
                            request_response::Message::Request { request, channel, .. } => {
                                warn!(
                                    "Received manifest fetch request from peer={} ({} bytes)",
                                    peer,
                                    request.len()
                                );
                                let response = b"manifest fetch not implemented".to_vec();
                                let _ = swarm
                                    .behaviour_mut()
                                    .manifest_fetch_rr
                                    .send_response(channel, response);
                            }
                            request_response::Message::Response { request_id, response } => {
                                let reply_sender = {
                                    let mut pending_requests = crate::network::control::get_pending_manifest_requests()
                                        .lock()
                                        .unwrap();
                                    pending_requests
                                        .remove(&request_id)
                                        .map(|(sender, _)| sender)
                                };

                                if let Some(reply_tx) = reply_sender {
                                    if response.is_empty() {
                                        let _ = reply_tx.send(Err("Empty manifest response".to_string()));
                                    } else {
                                        let _ = reply_tx.send(Ok(()));
                                    }
                                } else {
                                    debug!(
                                        "Received manifest fetch response without pending request: {:?} from {}",
                                        request_id,
                                        peer
                                    );
                                }
                            }
                        }
                    }

                    SwarmEvent::Behaviour(MyBehaviourEvent::ManifestFetchRr(request_response::Event::OutboundFailure { peer, request_id, error, connection_id: _ })) => {
                        warn!("libp2p: manifest fetch outbound failure for peer {}: {:?}", peer, error);
                        // Handle failed manifest distribution requests
                        let reply_sender = {
                            let mut pending_requests = crate::network::control::get_pending_manifest_requests()
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
                        // Add the connected peer to Kademlia DHT for placement announcements (via providers)
                        // Use the connection endpoint address for Kademlia
                        let addr = endpoint.get_remote_address();
                        swarm.behaviour_mut().kademlia.add_address(&peer_id, addr.clone());

                        // Ensure we track handshake state and proactively include the peer in our mesh
                        handshake_states
                            .entry(peer_id)
                            .or_insert(HandshakeState {
                                attempts: 0,
                                // Make the peer immediately eligible for the next handshake tick
                                last_attempt: Instant::now() - Duration::from_secs(3),
                                confirmed: false,
                            });
                        swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);

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
                        // Send handshake request using request-response protocol
                        let timestamp = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64;
                        let nonce = rand::random::<u32>();

                        let protocol_version = "beemesh/1.0";
                        let local_peer_id = swarm.local_peer_id().to_string();

                        // Build simple handshake request
                        let handshake_request = crate::messages::machine::build_handshake(
                            nonce,
                            timestamp,
                            protocol_version,
                            &local_peer_id,
                        );

                        let request_id = swarm
                            .behaviour_mut()
                            .handshake_rr
                            .send_request(peer_id, handshake_request);
                        debug!(
                            "libp2p: sent handshake request to peer={} request_id={:?} attempt={}",
                            peer_id,
                            request_id,
                            state.attempts + 1
                        );

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
            _ = renew_interval.tick() => {
                // Drain any enqueued control messages produced by behaviours (e.g. AnnounceProvider)
                control::drain_enqueued_controls(&mut swarm).await;
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

async fn process_delete_request(
    manifest_id: &str,
    force: bool,
    _requesting_peer: &str,
) -> (bool, String, Vec<String>) {
    if !force {
        warn!(
            "Processing delete request for manifest_id={} without explicit ownership validation",
            manifest_id
        );
    }

    match crate::scheduler::remove_workloads_by_manifest_id(manifest_id).await {
        Ok(removed_workloads) => {
            if removed_workloads.is_empty() {
                info!("No workloads found for manifest_id={}", manifest_id);
                (
                    true,
                    "no workloads found for manifest".to_string(),
                    removed_workloads,
                )
            } else {
                info!(
                    "Successfully removed {} workloads for manifest_id={}",
                    removed_workloads.len(),
                    manifest_id
                );
                (
                    true,
                    format!("removed {} workloads", removed_workloads.len()),
                    removed_workloads,
                )
            }
        }
        Err(e) => {
            error!(
                "Failed to remove workloads for manifest_id={}: {}",
                manifest_id, e
            );
            (false, format!("failed to remove workloads: {}", e), vec![])
        }
    }
}
