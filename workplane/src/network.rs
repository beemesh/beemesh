//! # Network Layer – libp2p DHT and RPC Transport
//!
//! This module implements the networking layer for workplane agents using libp2p.
//! It provides two main capabilities:
//!
//! 1. **Kademlia DHT** – Distributed storage for service records (WDHT)
//! 2. **Request/Response RPC** – Point-to-point communication between replicas
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                           Network                                       │
//! │  ┌───────────────────────────────────────────────────────────────────┐  │
//! │  │                    WorkplaneBehaviour                             │  │
//! │  │  ┌─────────────────────┐   ┌──────────────────────────────────┐  │  │
//! │  │  │   Kademlia DHT      │   │   Request/Response RPC           │  │  │
//! │  │  │   (ServiceRecords)  │   │   (RPCRequest/RPCResponse)       │  │  │
//! │  │  └─────────────────────┘   └──────────────────────────────────┘  │  │
//! │  └───────────────────────────────────────────────────────────────────┘  │
//! │                                    │                                     │
//! │                          ┌─────────▼─────────┐                           │
//! │                          │   QUIC Transport  │                           │
//! │                          └───────────────────┘                           │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Wire Protocol
//!
//! - **Transport**: QUIC (UDP-based, encrypted, multiplexed)
//! - **DHT Protocol**: `/ipfs/kad/1.0.0` (standard Kademlia)
//! - **RPC Protocol**: `/workplane/rpc/1.0.0` (JSON-encoded request/response)
//!
//! ## Event Loop
//!
//! The network runs an async event loop (`network_event_loop`) that:
//! - Handles incoming swarm events (DHT queries, RPC requests)
//! - Processes commands from the application (publish, send request, dial)
//! - Maintains the pending request map for RPC responses
//!
//! ## Service Record Publication
//!
//! Service records are published to the DHT with periodic heartbeats:
//! 1. `Network::start` begins the heartbeat loop
//! 2. `Network::refresh_heartbeat` increments version and republishes
//! 3. DHT records propagate to peers via Kademlia replication
//!
//! ## Policy Engine
//!
//! The `PolicyEngine` filters service records based on namespace and workload
//! allowlists/denylists (see workplane-spec.md §9).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use libp2p::futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, StreamExt};
use libp2p::identity::Keypair;
use libp2p::kad;
use libp2p::request_response;
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::{Multiaddr, PeerId, Swarm, SwarmBuilder};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::interval;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::config::Config;
use crate::discovery::{self, ServiceRecord};
use crate::rpc::{RPCRequest, RPCResponse};

// ============================================================================
// Network Behaviour
// ============================================================================

/// Combined libp2p behaviour for workplane networking.
///
/// Composes:
/// - **Kademlia**: Distributed hash table for service record storage
/// - **Request/Response**: Point-to-point RPC protocol
#[derive(NetworkBehaviour)]
struct WorkplaneBehaviour {
    /// Kademlia DHT for service record storage and peer discovery.
    kademlia: kad::Behaviour<kad::store::MemoryStore>,

    /// Request/response protocol for inter-replica RPC.
    request_response: request_response::Behaviour<RPCCodec>,
}

// ============================================================================
// RPC Codec
// ============================================================================

/// JSON codec for RPC request/response serialization.
///
/// Implements libp2p's `Codec` trait to serialize [`RPCRequest`] and
/// [`RPCResponse`] as JSON over the wire.
#[derive(Clone, Default)]
struct RPCCodec;

/// Protocol identifier for workplane RPC.
#[derive(Debug, Clone, Copy)]
struct RPCProtocol(&'static str);

impl AsRef<str> for RPCProtocol {
    fn as_ref(&self) -> &str {
        self.0
    }
}

#[async_trait]
impl request_response::Codec for RPCCodec {
    type Protocol = RPCProtocol;
    type Request = RPCRequest;
    type Response = RPCResponse;

    /// Read an RPC request from the stream.
    async fn read_request<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
    ) -> std::io::Result<Self::Request>
    where
        T: AsyncRead + Unpin + Send,
    {
        let mut vec = Vec::new();
        AsyncReadExt::read_to_end(io, &mut vec).await?;
        serde_json::from_slice(&vec)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// Read an RPC response from the stream.
    async fn read_response<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
    ) -> std::io::Result<Self::Response>
    where
        T: AsyncRead + Unpin + Send,
    {
        let mut vec = Vec::new();
        AsyncReadExt::read_to_end(io, &mut vec).await?;
        serde_json::from_slice(&vec)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// Write an RPC request to the stream.
    async fn write_request<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
        req: Self::Request,
    ) -> std::io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        let data = serde_json::to_vec(&req)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        AsyncWriteExt::write_all(io, &data).await
    }

    /// Write an RPC response to the stream.
    async fn write_response<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
        res: Self::Response,
    ) -> std::io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        let data = serde_json::to_vec(&res)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        AsyncWriteExt::write_all(io, &data).await
    }
}

// ============================================================================
// Network Implementation
// ============================================================================

/// Main network handle for workplane operations.
///
/// This is a cheap `Clone` handle that can be passed to multiple tasks.
/// All operations are non-blocking and communicate with the event loop
/// via an MPSC channel.
#[derive(Clone)]
pub struct Network {
    /// Shared state protected by Arc.
    inner: Arc<NetworkInner>,
    /// Command channel to the event loop.
    cmd_tx: mpsc::Sender<NetworkCommand>,
}

/// Internal state for the Network.
struct NetworkInner {
    /// Agent configuration.
    cfg: Config,
    /// Local service record (updated on health/leader changes).
    local_record: RwLock<ServiceRecord>,
    /// Heartbeat task handle for cleanup.
    heartbeat: Mutex<Option<HeartbeatHandle>>,
    /// Policy engine for filtering discovered peers.
    policy: PolicyEngine,
    /// Local libp2p peer ID.
    peer_id: PeerId,
}

/// Handle to the background heartbeat task.
struct HeartbeatHandle {
    /// Channel to signal shutdown.
    stop_tx: watch::Sender<bool>,
    /// Task handle for awaiting completion.
    join_handle: JoinHandle<()>,
}

/// Policy engine for filtering service discovery results.
///
/// Implements the namespace and workload filtering rules from spec §9:
/// - Denylist takes precedence over allowlist
/// - Cross-namespace discovery requires explicit opt-in
/// - Empty allowlist = allow all (minus denylist)
#[derive(Clone)]
struct PolicyEngine {
    /// Whether to allow discovering workloads from other namespaces.
    allow_cross_namespace: bool,
    /// Workload IDs (`namespace/name`) that are explicitly allowed.
    allowed: HashSet<String>,
    /// Workload IDs (`namespace/name`) that are explicitly denied.
    denied: HashSet<String>,
}

/// Commands sent from application code to the network event loop.
enum NetworkCommand {
    /// Publish a service record to the DHT.
    PublishRecord {
        record: ServiceRecord,
        ttl: Duration,
    },
    /// Send an RPC request to a specific peer.
    SendRequest {
        target: PeerId,
        request: RPCRequest,
        reply_tx: oneshot::Sender<Result<RPCResponse>>,
    },
    /// Start listening on a multiaddr.
    StartListening {
        addr: Multiaddr,
        reply_tx: oneshot::Sender<Result<()>>,
    },
    /// Dial a remote peer.
    Dial {
        addr: Multiaddr,
        reply_tx: oneshot::Sender<Result<()>>,
    },
}

impl Network {
    /// Create a new network instance.
    ///
    /// Initializes the libp2p swarm with Kademlia and request/response behaviours,
    /// then spawns the event loop. Does **not** start listening or connecting—
    /// call [`Network::start`] for that.
    ///
    /// # Arguments
    ///
    /// * `cfg` - Agent configuration (must have non-empty private_key, workload_name, pod_name)
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - `private_key` is empty
    /// - `workload_name` or `pod_name` is empty
    /// - Private key decoding fails
    pub fn new(cfg: &Config) -> Result<Self> {
        if cfg.private_key.is_empty() {
            return Err(anyhow!("config: private key is required"));
        }
        if cfg.workload_name.is_empty() || cfg.pod_name.is_empty() {
            return Err(anyhow!("config: workload name and pod name are required"));
        }

        // Decode the libp2p keypair from protobuf-encoded private key
        let keypair = Keypair::from_protobuf_encoding(&cfg.private_key)
            .context("decode libp2p private key")?;
        let peer_id = keypair.public().to_peer_id();
        let peer_id_str = peer_id.to_string();

        // Build initial capabilities map
        let mut caps = serde_json::Map::new();
        caps.insert(
            "namespace".into(),
            serde_json::Value::String(cfg.namespace.clone()),
        );
        caps.insert(
            "workload".into(),
            serde_json::Value::String(cfg.workload_name.clone()),
        );
        caps.insert(
            "pod".into(),
            serde_json::Value::String(cfg.pod_name.clone()),
        );
        if let Some(ord) = cfg.ordinal() {
            caps.insert("ordinal".into(), serde_json::json!(ord));
        }

        // Create initial service record
        let record = ServiceRecord {
            workload_id: cfg.workload_id(),
            namespace: cfg.namespace.clone(),
            workload_name: cfg.workload_name.clone(),
            peer_id: peer_id_str.clone(),
            pod_name: Some(cfg.pod_name.clone()),
            ordinal: cfg.ordinal(),
            addrs: cfg.listen_addrs.clone(),
            caps,
            version: 1,
            ts: current_millis(),
            nonce: Uuid::new_v4().to_string(),
            ready: false,
            healthy: false,
        };

        // Build the libp2p swarm with QUIC transport and DNS resolution
        let swarm = SwarmBuilder::with_existing_identity(keypair)
            .with_tokio()
            .with_quic()
            .with_dns()?
            .with_behaviour(|key| {
                let peer_id = key.public().to_peer_id();
                let store = kad::store::MemoryStore::new(peer_id);
                let kad_config = kad::Config::default();
                let kademlia = kad::Behaviour::with_config(peer_id, store, kad_config);

                // Configure request/response with our RPC protocol
                let protocols = std::iter::once((
                    RPCProtocol("/workplane/rpc/1.0.0"),
                    request_response::ProtocolSupport::Full,
                ));
                let rr_config = request_response::Config::default();
                let request_response = request_response::Behaviour::new(protocols, rr_config);

                Ok(WorkplaneBehaviour {
                    kademlia,
                    request_response,
                })
            })?
            .build();

        let (cmd_tx, cmd_rx) = mpsc::channel(32);

        // Spawn the background event loop
        tokio::spawn(network_event_loop(swarm, cmd_rx));

        Ok(Self {
            inner: Arc::new(NetworkInner {
                cfg: cfg.clone(),
                local_record: RwLock::new(record),
                heartbeat: Mutex::new(None),
                policy: PolicyEngine::from_config(cfg),
                peer_id,
            }),
            cmd_tx,
        })
    }

    /// Start network operations: listen, connect to bootstrap peers, begin heartbeats.
    ///
    /// This method:
    /// 1. Starts listening on configured addresses
    /// 2. Dials bootstrap peers for initial DHT population
    /// 3. Publishes the initial service record
    /// 4. Spawns the heartbeat task for periodic record refresh
    pub fn start(&self) {
        // Start listening on configured addresses
        for addr_str in &self.inner.cfg.listen_addrs {
            if let Ok(addr) = addr_str.parse::<Multiaddr>() {
                let (tx, _rx) = oneshot::channel();
                let _ = self
                    .cmd_tx
                    .try_send(NetworkCommand::StartListening { addr, reply_tx: tx });
            }
        }

        // Connect to bootstrap peers for initial DHT population
        for peer_str in &self.inner.cfg.bootstrap_peer_strings {
            if let Ok(addr) = peer_str.parse::<Multiaddr>() {
                let (tx, _rx) = oneshot::channel();
                let _ = self
                    .cmd_tx
                    .try_send(NetworkCommand::Dial { addr, reply_tx: tx });
            }
        }

        // Publish initial service record
        if let Err(err) = self.refresh_heartbeat() {
            warn!(error = %err, "failed to publish initial service record");
        }

        // Start heartbeat task (idempotent)
        let mut guard = self.inner.heartbeat.lock().expect("heartbeat lock");
        if guard.is_some() {
            return; // Already running
        }

        let (tx, mut rx) = watch::channel(false);
        let interval_duration = self.inner.cfg.heartbeat_interval;
        let network = self.clone();
        let join_handle = tokio::spawn(async move {
            let mut ticker = interval(interval_duration);
            ticker.tick().await; // Skip immediate first tick
            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        if let Err(err) = network.refresh_heartbeat() {
                            warn!(error = %err, "failed to refresh WDHT record");
                        }
                    }
                    changed = rx.changed() => {
                        if changed.is_ok() && *rx.borrow() {
                            break;
                        }
                    }
                }
            }
            debug!("network heartbeat stopped");
        });

        *guard = Some(HeartbeatHandle {
            stop_tx: tx,
            join_handle,
        });
    }

    /// Gracefully shutdown the network: stop heartbeats and allow TTL expiration.
    pub async fn shutdown(&self) {
        if let Some(handle) = self.take_heartbeat() {
            let _ = handle.stop_tx.send(true);
            let _ = handle.join_handle.await;
        }
        // Records will expire via TTL; no explicit tombstone needed
    }

    /// Update the local health status and republish if changed.
    ///
    /// Called by the self-healer after each health probe cycle.
    ///
    /// # Arguments
    ///
    /// * `healthy` - Liveness probe result
    /// * `ready` - Readiness probe result
    pub fn update_local_status(&self, healthy: bool, ready: bool) -> Result<()> {
        let mut changed = false;
        {
            let mut record = self.inner.local_record.write().expect("record write lock");
            if record.healthy != healthy {
                record.healthy = healthy;
                changed = true;
            }
            if record.ready != ready {
                record.ready = ready;
                changed = true;
            }
            if changed {
                record.version = record.version.saturating_add(1);
                record.ts = current_millis();
                record.nonce = Uuid::new_v4().to_string();
            }
        }

        if changed {
            self.publish_current_record()?;
        }
        Ok(())
    }

    /// Update leader endpoint advertisement for stateful workloads.
    ///
    /// When becoming leader, sets `caps.leader = true` and `caps.leader_epoch`.
    /// When stepping down, sets `caps.leader = false`.
    ///
    /// # Arguments
    ///
    /// * `active` - Whether this replica is the current leader
    /// * `epoch` - Raft term/epoch for consistency
    pub fn update_leader_endpoint(&self, active: bool, epoch: u64) -> Result<()> {
        let mut should_publish = false;
        {
            let mut record = self.inner.local_record.write().expect("record write lock");
            let mut changed = false;

            // Update readiness based on leader status (for routing)
            if record.ready != active {
                record.ready = active;
                changed = true;
            }

            // Update leader capability flag
            let leader_value = serde_json::Value::Bool(active);
            if record.caps.get("leader") != Some(&leader_value) {
                record.caps.insert("leader".into(), leader_value);
                changed = true;
            }

            // Update leader epoch for consistency
            let epoch_value = serde_json::Value::Number(serde_json::Number::from(epoch));
            if record.caps.get("leader_epoch") != Some(&epoch_value) {
                record.caps.insert("leader_epoch".into(), epoch_value);
                changed = true;
            }

            if changed {
                record.version = record.version.saturating_add(1);
                record.ts = current_millis();
                record.nonce = Uuid::new_v4().to_string();
                should_publish = true;
            }
        }

        if should_publish {
            if active {
                info!(epoch, "advertising leader endpoint");
                crate::increment_counter!("workplane.network.leader_advertisements");
            } else {
                info!(epoch, "withdrawing leader endpoint");
                crate::increment_counter!("workplane.network.leader_withdrawals");
            }
            self.publish_current_record()?;
        }
        Ok(())
    }

    /// Find all service peers for the current workload.
    ///
    /// Returns service records from the local WDHT cache, filtered by the
    /// policy engine (namespace, allowlist, denylist).
    ///
    /// # Returns
    ///
    /// Vector of service records for reachable peers.
    pub fn find_service_peers(&self) -> Vec<ServiceRecord> {
        let namespace = self.inner.cfg.namespace.clone();
        let kind = self.inner.cfg.workload_kind.clone();
        let workload = self.inner.cfg.workload_name.clone();
        let local_id = self.inner.cfg.workload_id();

        // Query the discovery cache and filter by policy
        discovery::list(&namespace, &kind, &workload)
            .into_iter()
            .filter(|record| {
                record.workload_id == local_id || self.inner.policy.allows(&namespace, record)
            })
            .collect()
    }

    /// Get the local peer ID as a string.
    pub fn peer_id(&self) -> String {
        self.inner.peer_id.to_string()
    }

    /// Refresh the heartbeat: increment version and republish.
    fn refresh_heartbeat(&self) -> Result<()> {
        {
            let mut record = self.inner.local_record.write().expect("record write lock");
            record.version = record.version.saturating_add(1);
            record.ts = current_millis();
            record.nonce = Uuid::new_v4().to_string();
        }
        self.publish_current_record()
    }

    /// Publish the current service record to the DHT.
    fn publish_current_record(&self) -> Result<()> {
        let record = self.local_record();
        let ttl = self.inner.cfg.dht_ttl;

        let cmd = NetworkCommand::PublishRecord { record, ttl };
        let _ = self.cmd_tx.try_send(cmd);

        Ok(())
    }

    /// Get a clone of the local service record.
    fn local_record(&self) -> ServiceRecord {
        self.inner
            .local_record
            .read()
            .expect("record read lock")
            .clone()
    }

    /// Take ownership of the heartbeat handle (for shutdown).
    fn take_heartbeat(&self) -> Option<HeartbeatHandle> {
        self.inner.heartbeat.lock().expect("heartbeat lock").take()
    }

    /// Send an RPC request to a specific peer and await the response.
    ///
    /// # Arguments
    ///
    /// * `target_peer_id` - libp2p peer ID string of the target
    /// * `req` - The RPC request to send
    ///
    /// # Returns
    ///
    /// The response from the target peer, or an error if the request failed.
    pub async fn send_request(&self, target_peer_id: &str, req: RPCRequest) -> Result<RPCResponse> {
        let peer_id = target_peer_id.parse::<PeerId>()?;
        let (tx, rx) = oneshot::channel();

        self.cmd_tx
            .send(NetworkCommand::SendRequest {
                target: peer_id,
                request: req,
                reply_tx: tx,
            })
            .await?;

        rx.await?
    }
}

// ============================================================================
// Event Loop
// ============================================================================

/// Main network event loop handling swarm events and application commands.
///
/// This function runs indefinitely, processing:
/// - **Kademlia events**: DHT query results, record storage
/// - **Request/Response events**: Incoming RPC requests, outbound responses
/// - **Application commands**: Publish, send request, listen, dial
///
/// # Arguments
///
/// * `swarm` - The libp2p swarm to drive
/// * `cmd_rx` - Channel for receiving commands from application code
async fn network_event_loop(
    mut swarm: Swarm<WorkplaneBehaviour>,
    mut cmd_rx: mpsc::Receiver<NetworkCommand>,
) {
    // Map of pending outbound RPC requests awaiting responses
    let mut pending_requests: HashMap<
        request_response::OutboundRequestId,
        oneshot::Sender<Result<RPCResponse>>,
    > = HashMap::new();

    loop {
        tokio::select! {
            // Handle swarm events
            event = swarm.select_next_some() => {
                match event {
                    // DHT query completed - update local cache
                    SwarmEvent::Behaviour(WorkplaneBehaviourEvent::Kademlia(kad::Event::OutboundQueryProgressed { result, .. })) => {
                        if let kad::QueryResult::GetRecord(Ok(kad::GetRecordOk::FoundRecord(peer_record))) = result
                            && let Ok(record) = serde_json::from_slice::<ServiceRecord>(&peer_record.record.value)
                        {
                            // Update the in-memory discovery cache
                            discovery::put(record, Duration::from_secs(60));
                        }
                    }
                    // RPC request/response events
                    SwarmEvent::Behaviour(WorkplaneBehaviourEvent::RequestResponse(request_response::Event::Message { peer, message, .. })) => {
                        match message {
                            // Incoming RPC request - invoke registered handler
                            request_response::Message::Request { request, channel, .. } => {
                                let response = crate::rpc::handle_request(&peer.to_string(), request).await;
                                let _ = swarm.behaviour_mut().request_response.send_response(channel, response);
                            }
                            // Outbound RPC response received
                            request_response::Message::Response { request_id, response } => {
                                if let Some(tx) = pending_requests.remove(&request_id) {
                                    let _ = tx.send(Ok(response));
                                }
                            }
                        }
                    }
                    // Outbound RPC failed
                    SwarmEvent::Behaviour(WorkplaneBehaviourEvent::RequestResponse(request_response::Event::OutboundFailure { request_id, error, .. })) => {
                        if let Some(tx) = pending_requests.remove(&request_id) {
                            let _ = tx.send(Err(anyhow!("RPC failed: {:?}", error)));
                        }
                    }
                    _ => {}
                }
            }
            // Handle application commands
            Some(cmd) = cmd_rx.recv() => {
                match cmd {
                    // Publish service record to DHT
                    NetworkCommand::PublishRecord { record, ttl } => {
                        let key = kad::RecordKey::new(&record.workload_id);
                        if let Ok(data) = serde_json::to_vec(&record) {
                            let kad_record = kad::Record {
                                key,
                                value: data,
                                publisher: None,
                                expires: None, // TTL handled by local cache
                            };
                            let _ = swarm.behaviour_mut().kademlia.put_record(kad_record, kad::Quorum::One);

                            // Also update local cache
                            discovery::put(record.clone(), ttl);
                        }
                    }
                    // Send RPC request to peer
                    NetworkCommand::SendRequest { target, request, reply_tx } => {
                        let request_id = swarm.behaviour_mut().request_response.send_request(&target, request);
                        pending_requests.insert(request_id, reply_tx);
                    }
                    // Start listening on address
                    NetworkCommand::StartListening { addr, reply_tx } => {
                        let res = swarm.listen_on(addr).map(|_| ()).map_err(|e| anyhow!(e));
                        let _ = reply_tx.send(res);
                    }
                    // Dial remote peer
                    NetworkCommand::Dial { addr, reply_tx } => {
                         let res = swarm.dial(addr).map(|_| ()).map_err(|e| anyhow!(e));
                         let _ = reply_tx.send(res);
                    }
                }
            }
        }
    }
}

// ============================================================================
// Policy Engine
// ============================================================================

impl PolicyEngine {
    /// Create a policy engine from configuration.
    fn from_config(cfg: &Config) -> Self {
        Self {
            allow_cross_namespace: cfg.allow_cross_namespace,
            allowed: cfg.allowed_workloads.iter().cloned().collect(),
            denied: cfg.denied_workloads.iter().cloned().collect(),
        }
    }

    /// Check if a service record should be visible to this agent.
    ///
    /// Rules (per spec §9):
    /// 1. Denylist always blocks
    /// 2. Cross-namespace requires `allow_cross_namespace` OR explicit allowlist entry
    /// 3. Empty allowlist = allow all (minus denylist)
    fn allows(&self, local_namespace: &str, record: &ServiceRecord) -> bool {
        let workload_id = format!("{}/{}", record.namespace, record.workload_name);

        // Denylist takes precedence
        if self.denied.contains(&workload_id) {
            return false;
        }

        // Cross-namespace check
        if !self.allow_cross_namespace && record.namespace != local_namespace {
            return self.allowed.contains(&workload_id);
        }

        // Empty allowlist = allow all
        if self.allowed.is_empty() {
            return true;
        }

        // Check allowlist
        self.allowed.contains(&workload_id)
    }
}

// ============================================================================
// Utilities
// ============================================================================

/// Get current Unix time in milliseconds.
fn current_millis() -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    now.as_millis() as i64
}
