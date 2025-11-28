//! # Network Module
//!
//! This module implements the libp2p networking layer for the machineplane.
//! It provides peer-to-peer communication using:
//!
//! - **Gossipsub**: For pub/sub messaging of scheduler messages (Tender, Bid, Award, Event)
//! - **Kademlia DHT**: For peer discovery (MDHT) and distributed record storage
//! - **Request-Response**: For direct peer communication (manifest transfer)
//! - **QUIC Transport**: For encrypted, multiplexed connections
//!
//! ## Architecture
//!
//! The networking layer follows a message-driven architecture:
//!
//! 1. `start_libp2p_node()` initializes the swarm and begins the event loop
//! 2. Incoming gossipsub messages are decoded and dispatched to the `Scheduler`
//! 3. Outbound messages are sent via `SchedulerCommand::Publish`
//! 4. DHT operations handle peer discovery and workload record storage
//!
//! ## Spec Reference
//!
//! - §3: MDHT Discovery - Kademlia-based peer discovery with bootstrap and random walks
//! - §4: Gossipsub messaging for Tender/Bid/Award/Event distribution
//! - §5.3: RequestResponse for manifest transfer to award winners

use anyhow::Result;
use async_trait::async_trait;
use futures::{AsyncReadExt, AsyncWriteExt, stream::StreamExt};
use libp2p::{
    Swarm, gossipsub, identity::Keypair, kad, multiaddr::Multiaddr, multiaddr::Protocol,
    request_response, swarm::SwarmEvent,
};
use log::{debug, error, info, warn};
use std::net::IpAddr;
use std::sync::{LazyLock, Mutex, OnceLock};
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, watch};

use crate::messages::libp2p_constants::BEEMESH_FABRIC;
use crate::messages::types::EventType;
use crate::scheduler::SchedulerCommand;

// Submodules
pub mod behaviour;
pub mod control;
pub mod utils;

use behaviour::{MyBehaviour, MyBehaviourEvent};
use control::Libp2pControl;

// ============================================================================
// Global State
// ============================================================================

/// Global control sender for distributed operations.
///
/// Allows external components to send control messages to the libp2p event loop
/// (e.g., to trigger DHT lookups or publish messages).
static CONTROL_SENDER: OnceLock<mpsc::UnboundedSender<control::Libp2pControl>> = OnceLock::new();

/// Per-node keypair storage for multi-node testing.
///
/// Maps peer_id bytes to (public_key_bytes, private_key_bytes).
/// This allows multiple nodes in the same process to have distinct keypairs.
static NODE_KEYPAIRS: LazyLock<Mutex<std::collections::HashMap<Vec<u8>, (Vec<u8>, Vec<u8>)>>> =
    LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

// ============================================================================
// Request-Response Protocol Types
// ============================================================================

/// Protocol identifier for the request-response codec.
///
/// Used to negotiate the protocol during libp2p stream opening.
#[derive(Clone, Copy)]
pub struct ByteProtocol(&'static str);

impl AsRef<str> for ByteProtocol {
    fn as_ref(&self) -> &str {
        self.0
    }
}

/// Simple byte-based codec for request-response messages.
///
/// Reads and writes raw bytes without framing - the entire stream is
/// consumed as a single message. Used for manifest transfer.
#[derive(Clone, Default)]
pub struct ByteCodec;

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

/// Type alias for the manifest transfer codec.
pub type ManifestTransferCodec = ByteCodec;

// ============================================================================
// Keypair Management
// ============================================================================

/// Retrieves the keypair for a specific peer ID.
///
/// If `peer_id` is `None`, returns the first available keypair for backward
/// compatibility with single-node scenarios.
///
/// # Returns
///
/// A tuple of (public_key_bytes, private_key_bytes) if found.
pub fn get_node_keypair_for_peer(peer_id: Option<&[u8]>) -> Option<(Vec<u8>, Vec<u8>)> {
    let keypairs = NODE_KEYPAIRS.lock().unwrap();
    if let Some(pid) = peer_id {
        keypairs.get(pid).cloned()
    } else {
        // Backward compatibility: return first available keypair
        keypairs.values().next().cloned()
    }
}

/// Retrieves the first available keypair (backward compatibility).
pub fn get_node_keypair() -> Option<(Vec<u8>, Vec<u8>)> {
    get_node_keypair_for_peer(None)
}

/// Stores a keypair for a specific peer ID.
///
/// # Arguments
///
/// * `pair` - Tuple of (peer_id_bytes, private_key_encoding)
pub fn set_node_keypair(pair: Option<(Vec<u8>, Vec<u8>)>) {
    let mut keypairs = NODE_KEYPAIRS.lock().unwrap();
    if let Some((peer_id_bytes, keypair_bytes)) = pair {
        keypairs.insert(peer_id_bytes.clone(), (peer_id_bytes, keypair_bytes));
    }
}

// ============================================================================
// Control Channel Management
// ============================================================================

/// Sets the global control sender for distributed operations.
///
/// This sender allows external components to send control messages to the
/// libp2p event loop (e.g., DHT operations, gossipsub publishes).
pub fn set_control_sender(sender: mpsc::UnboundedSender<control::Libp2pControl>) {
    let _ = CONTROL_SENDER.set(sender);
}

/// Retrieves the global control sender for distributed operations.
pub fn get_control_sender() -> Option<&'static mpsc::UnboundedSender<control::Libp2pControl>> {
    CONTROL_SENDER.get()
}

// ============================================================================
// Swarm Setup
// ============================================================================

/// Sets up a new libp2p swarm with gossipsub, Kademlia, and request-response.
///
/// This function:
/// 1. Loads the keypair for the given peer ID
/// 2. Configures QUIC transport with DNS resolution
/// 3. Sets up gossipsub with strict validation and message deduplication
/// 4. Initializes Kademlia DHT in server mode
/// 5. Configures request-response for manifest transfer
/// 6. Subscribes to the BEEMESH_FABRIC topic
///
/// # Arguments
///
/// * `quic_port` - UDP port for QUIC transport
/// * `host` - Host address to bind to (e.g., "0.0.0.0")
/// * `local_peer_id_bytes` - Peer ID bytes to look up the keypair
///
/// # Returns
///
/// A tuple of (Swarm, gossipsub topic, peer watch receiver, peer watch sender).
pub fn setup_libp2p_node(
    quic_port: u16,
    host: &str,
    local_peer_id_bytes: &[u8],
) -> Result<(
    Swarm<MyBehaviour>,
    gossipsub::IdentTopic,
    watch::Receiver<Vec<String>>,
    watch::Sender<Vec<String>>,
)> {
    // Load the node keypair for this specific peer_id (set by lib.rs startup)
    let keypair = {
        let keypairs = NODE_KEYPAIRS.lock().unwrap();
        let (_, sk) = keypairs.get(local_peer_id_bytes).ok_or_else(|| {
            anyhow::anyhow!("machine peer identity not initialized for this peer_id")
        })?;
        libp2p::identity::Keypair::from_protobuf_encoding(sk)?
    };

    // Build the swarm with QUIC transport and DNS support
    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_quic()
        .with_dns()?
        .with_behaviour(|key| {
            info!("Local PeerId: {}", key.public().to_peer_id());

            // Configure gossipsub with content-addressed message IDs
            // This prevents duplicate messages based on content hash
            let message_id_fn = |message: &gossipsub::Message| {
                let mut s = DefaultHasher::new();
                message.data.hash(&mut s);
                gossipsub::MessageId::from(s.finish().to_string())
            };

            // Gossipsub configuration per §4 of spec
            let gossipsub_config = gossipsub::ConfigBuilder::default()
                .heartbeat_interval(Duration::from_secs(10))
                // Enforce strict validation to ensure only authenticated messages are accepted.
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

            // Configure placement record settings (via Kademlia providers) for low-touch announcements
            kademlia_config.set_provider_record_ttl(Some(std::time::Duration::from_secs(600)));
            kademlia_config.set_provider_publication_interval(None);

            let kademlia =
                kad::Behaviour::with_config(key.public().to_peer_id(), store, kademlia_config);

            Ok(MyBehaviour {
                gossipsub,
                manifest_fetch_rr,
                kademlia,
            })
        })?
        .build();

    let topic = gossipsub::IdentTopic::new(BEEMESH_FABRIC);

    debug!("Subscribing to topics: {}", topic.hash());
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

// ============================================================================
// Event Loop
// ============================================================================

/// Starts the libp2p node event loop.
///
/// This is the main async function that drives the networking layer. It:
///
/// 1. Initializes the scheduler and spawns its message handler
/// 2. Processes incoming swarm events (connections, messages, DHT)
/// 3. Handles control commands from external components
/// 4. Performs periodic DHT refresh via random walks
/// 5. Updates the connected peers watch channel
///
/// # Arguments
///
/// * `swarm` - The configured libp2p swarm
/// * `topic` - The gossipsub topic to publish scheduler messages on
/// * `peer_tx` - Watch channel sender for connected peers list
/// * `control_rx` - Receiver for control commands
/// * `dht_refresh_interval_secs` - Interval in seconds for DHT random walks
///
/// # Spec Reference
///
/// - §3: MDHT Discovery - bootstrap on first connection, periodic random walks
/// - §4: Gossipsub message handling
pub async fn start_libp2p_node(
    mut swarm: Swarm<MyBehaviour>,
    topic: gossipsub::IdentTopic, // Main fabric topic
    peer_tx: watch::Sender<Vec<String>>,
    mut control_rx: mpsc::UnboundedReceiver<Libp2pControl>,
    dht_refresh_interval_secs: u64,
) -> Result<()> {
    // Periodic cleanup of stale connections
    let mut cleanup_interval = tokio::time::interval(Duration::from_secs(2));

    // Periodic DHT refresh via random walks per §3 of spec
    let mut dht_refresh_interval = tokio::time::interval(Duration::from_secs(dht_refresh_interval_secs));
    // Don't tick immediately on startup; wait for connections first
    dht_refresh_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Track whether we've done initial DHT bootstrap
    let mut dht_bootstrapped = false;

    // --- Scheduler Setup ---
    // Create channels for scheduler input/output
    let (sched_input_tx, mut sched_input_rx) =
        mpsc::unbounded_channel::<(gossipsub::TopicHash, gossipsub::Message)>();
    let (sched_output_tx, mut sched_output_rx) = mpsc::unbounded_channel::<SchedulerCommand>();

    // Initialize scheduler input sender for this specific peer
    // This allows multiple nodes in the same process to have separate schedulers
    let local_peer_id = *swarm.local_peer_id();
    behaviour::set_scheduler_input_for_peer(local_peer_id, sched_input_tx.clone());

    // Also set legacy global for backward compatibility (first node wins)
    behaviour::SCHEDULER_INPUT_TX.set(sched_input_tx).ok();

    // Spawn Scheduler with keypair from this node
    let local_node_id = swarm.local_peer_id().to_string();
    let local_peer_id_bytes = swarm.local_peer_id().to_bytes();
    let scheduler_keypair = get_node_keypair_for_peer(Some(&local_peer_id_bytes))
        .and_then(|(_, sk)| Keypair::from_protobuf_encoding(&sk).ok())
        .ok_or_else(|| anyhow::anyhow!("machine peer identity not initialized"))?;

    let scheduler =
        crate::scheduler::Scheduler::new(local_node_id.clone(), scheduler_keypair, sched_output_tx);
    let scheduler = std::sync::Arc::new(scheduler);
    let scheduler_for_messages = scheduler.clone();

    // Spawn async task to handle incoming scheduler messages
    tokio::spawn(async move {
        while let Some((topic, msg)) = sched_input_rx.recv().await {
            scheduler_for_messages.handle_message(&topic, &msg).await;
        }
    });
    // -----------------------

    // Main event loop
    loop {
        tokio::select! {
            // Handle scheduler output (bids/tenders/awards to publish, workloads to deploy)
            Some(command) = sched_output_rx.recv() => {
                     match command {
                         SchedulerCommand::Publish { topic, payload } => {
                             // Publish scheduler message to gossipsub
                             let topic = gossipsub::IdentTopic::new(topic);
                             if let Err(e) = swarm.behaviour_mut().gossipsub.publish(topic, payload) {
                                 log::error!("Failed to publish scheduler message: {}", e);
                             }
                         }
                         SchedulerCommand::DeployWorkload { tender_id, manifest_id, manifest_json, replicas } => {
                            // Deploy workload locally after winning tender
                            let apply_req = crate::messages::types::ApplyRequest {
                                replicas,
                                operation_id: format!("sched-deploy-{}", uuid::Uuid::new_v4()),
                                manifest_json: manifest_json.clone(),
                                origin_peer: local_node_id.clone(),
                                manifest_id: manifest_id.clone(),
                                signature: Vec::new(),
                            };
                             // Empty owner_pubkey - we trust the scheduler's decision
                             let owner_pubkey = Vec::new();

                             match crate::scheduler::process_manifest_deployment(
                                 &mut swarm,
                                 &apply_req,
                                 &manifest_json,
                                 &owner_pubkey,
                             ).await {
                                 Ok(workload_id) => {
                                     // Emit Deployed event per §9.1
                                     scheduler.publish_event(
                                         &tender_id,
                                         EventType::Deployed,
                                         &format!("workload {} deployed", workload_id),
                                     );
                                 }
                                 Err(e) => {
                                     // Emit Failed event per §9.1
                                     log::error!("Failed to deploy workload from scheduler: {}", e);
                                     scheduler.publish_event(
                                         &tender_id,
                                         EventType::Failed,
                                         &format!("deployment failed: {}", e),
                                     );
                                 }
                             }
                         }
                         SchedulerCommand::SendManifest { peer_id, payload } => {
                            match peer_id.parse() {
                                Ok(peer) => {
                                    let request_id = swarm
                                        .behaviour_mut()
                                        .manifest_fetch_rr
                                        .send_request(&peer, payload);
                                    let mut pending_requests = crate::network::control::get_pending_manifest_requests()
                                        .lock()
                                        .unwrap();
                                    pending_requests.insert(
                                        request_id,
                                        (mpsc::unbounded_channel::<Result<(), String>>().0, Instant::now()),
                                    );
                                    log::info!(
                                        "Dispatched manifest transfer request {:?} to peer {}",
                                        request_id, peer
                                    );
                                }
                                Err(e) => {
                                    log::error!("Invalid peer id {} for manifest transfer: {:?}", peer_id, e);
                                }
                            }
                         }
                     }
            }
            // control messages from other parts of the host (REST handlers)
            maybe_msg = control_rx.recv() => {
                if let Some(msg) = maybe_msg {
                    control::handle_control_message(msg, &mut swarm).await;
                } else {
                    // sender was dropped, exit loop
                    info!("control channel closed; shutting down libp2p loop");
                    break;
                }
            }
            event = swarm.select_next_some() => {
                let local_peer_id = *swarm.local_peer_id();
                match event {
                    SwarmEvent::Behaviour(MyBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                        propagation_source: peer_id,
                        message_id: _id,
                        message,
                    })) => {
                        behaviour::gossipsub_message(peer_id, local_peer_id, message, topic.hash().clone());
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
                                info!(
                                    "Received manifest transfer from peer={} ({} bytes)",
                                    peer,
                                    request.len()
                                );

                                let response_bytes = match crate::messages::machine::decode_manifest_transfer(&request) {
                                    Ok(transfer) => {
                                        let computed_digest = crate::messages::machine::compute_manifest_id_from_content(
                                            transfer.manifest_json.as_bytes(),
                                        );

                                        if computed_digest != transfer.manifest_digest {
                                            warn!(
                                                "Rejecting manifest for tender {} from {}: digest mismatch (expected {}, got {})",
                                                transfer.tender_id,
                                                peer,
                                                transfer.manifest_digest,
                                                computed_digest
                                            );
                                            b"digest mismatch".to_vec()
                                        } else {
                                            crate::scheduler::register_local_manifest(
                                                &transfer.tender_id,
                                                &transfer.manifest_json,
                                            );
                                            crate::scheduler::record_tender_manifest_id(
                                                &transfer.tender_id,
                                                &transfer.manifest_id,
                                            );

                                            if !transfer.owner_pubkey.is_empty() {
                                                crate::scheduler::record_manifest_owner(
                                                    &transfer.manifest_id,
                                                    &transfer.owner_pubkey,
                                                )
                                                .await;
                                            }

                                            let apply_req = crate::messages::types::ApplyRequest {
                                                replicas: transfer.replicas.max(1),
                                                operation_id: format!(
                                                    "award-{}-{}",
                                                    transfer.tender_id,
                                                    uuid::Uuid::new_v4()
                                                ),
                                                manifest_json: transfer.manifest_json.clone(),
                                                origin_peer: transfer.owner_peer_id.clone(),
                                                manifest_id: transfer.manifest_id.clone(),
                                                signature: Vec::new(),
                                            };

                                            let owner_pubkey = transfer.owner_pubkey.clone();
                                            let manifest_json = transfer.manifest_json.clone();

                                            match crate::scheduler::process_manifest_deployment(
                                                &mut swarm,
                                                &apply_req,
                                                &manifest_json,
                                                &owner_pubkey,
                                            )
                                            .await
                                            {
                                                Ok(workload_id) => {
                                                    info!(
                                                        "Successfully deployed manifest {} for tender {} as workload {}",
                                                        transfer.manifest_id, transfer.tender_id, workload_id
                                                    );
                                                    scheduler.publish_event(
                                                        &transfer.tender_id,
                                                        EventType::Deployed,
                                                        &format!(
                                                            "workload {} deployed",
                                                            workload_id
                                                        ),
                                                    );
                                                    b"ok".to_vec()
                                                }
                                                Err(e) => {
                                                    error!(
                                                        "Deployment failed for tender {} manifest {}: {}",
                                                        transfer.tender_id, transfer.manifest_id, e
                                                    );
                                                    scheduler.publish_event(
                                                        &transfer.tender_id,
                                                        EventType::Failed,
                                                        &format!("deployment failed: {}", e),
                                                    );
                                                    format!("deploy error: {}", e).into_bytes()
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        warn!("Failed to parse manifest transfer from {}: {}", peer, e);
                                        b"invalid payload".to_vec()
                                    }
                                };

                                let _ = swarm
                                    .behaviour_mut()
                                    .manifest_fetch_rr
                                    .send_response(channel, response_bytes);
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

                        swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);

                        let all_peers: Vec<String> = swarm
                            .behaviour()
                            .gossipsub
                            .all_peers()
                            .map(|(p, _topics)| p.to_string())
                            .collect();
                        let _ = peer_tx.send(all_peers);

                        // Bootstrap DHT on first connection to start peer discovery
                        if !dht_bootstrapped {
                            let connected_peers: Vec<_> = swarm.connected_peers().cloned().collect();
                            info!("MDHT: Bootstrapping DHT with {} connected peer(s)", connected_peers.len());
                            if swarm.behaviour_mut().kademlia.bootstrap().is_ok() {
                                dht_bootstrapped = true;
                            }
                        }
                    }
                    SwarmEvent::ConnectionClosed { peer_id, connection_id: _, endpoint: _, num_established, cause: _ } => {
                        if num_established == 0 {
                            info!("DHT: All connections to peer {} closed, removing from Kademlia", peer_id);
                            swarm.behaviour_mut().kademlia.remove_peer(&peer_id);
                        }

                        let all_peers: Vec<String> = swarm
                            .behaviour()
                            .gossipsub
                            .all_peers()
                            .map(|(p, _topics)| p.to_string())
                            .collect();
                        let _ = peer_tx.send(all_peers);
                    }
                    SwarmEvent::NewListenAddr { address, .. } => {
                        let peer_id = swarm.local_peer_id();
                        let mut with_peer_id = address.clone();
                        with_peer_id.push(Protocol::P2p((*peer_id).into()));

                        info!(
                            "libp2p: listening on {address} (bootstrap with {with_peer_id})"
                        );
                    }
                    _ => {}
                }
            }
            _ = cleanup_interval.tick() => {
                // Clean up timed-out manifest distribution requests
                control::cleanup_timed_out_manifest_requests();
            }
            _ = dht_refresh_interval.tick() => {
                let connected_peers: Vec<_> = swarm.connected_peers().cloned().collect();
                
                // Only perform DHT operations if we have at least one connected peer
                if connected_peers.is_empty() {
                    continue;
                }

                // Initial bootstrap if not done yet
                if !dht_bootstrapped {
                    info!("MDHT: Initiating DHT bootstrap with {} connected peers", connected_peers.len());
                    if let Err(e) = swarm.behaviour_mut().kademlia.bootstrap() {
                        warn!("MDHT: Bootstrap failed: {:?}", e);
                    } else {
                        dht_bootstrapped = true;
                    }
                }

                // Perform random walk for peer discovery
                // Generate a random PeerId to search for, which triggers discovery of nearby peers
                let random_peer_id = libp2p::PeerId::random();
                debug!("MDHT: Starting random walk discovery (target: {})", random_peer_id);
                swarm.behaviour_mut().kademlia.get_closest_peers(random_peer_id);

                // Log current DHT state
                let routing_table_size = swarm.behaviour_mut().kademlia.kbuckets()
                    .fold(0usize, |acc, bucket| acc + bucket.num_entries());
                debug!(
                    "MDHT: Refresh complete - {} connected peers, {} DHT routing entries",
                    connected_peers.len(),
                    routing_table_size
                );
            }
        }
    }

    Ok(())
}
