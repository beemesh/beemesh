use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, StreamExt};
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
use crate::streams::{RPCRequest, RPCResponse};

// --- Network Behaviour ---

#[derive(NetworkBehaviour)]
struct WorkplaneBehaviour {
    kademlia: kad::Behaviour<kad::store::MemoryStore>,
    request_response: request_response::Behaviour<RPCCodec>,
}

// --- RPC Codec ---

#[derive(Clone, Default)]
struct RPCCodec;

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
        serde_json::from_slice(&vec).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

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
        serde_json::from_slice(&vec).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

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

// --- Network Implementation ---

#[derive(Clone)]
pub struct Network {
    inner: Arc<NetworkInner>,
    cmd_tx: mpsc::Sender<NetworkCommand>,
}

struct NetworkInner {
    cfg: Config,
    local_record: RwLock<ServiceRecord>,
    heartbeat: Mutex<Option<HeartbeatHandle>>,
    policy: PolicyEngine,
    peer_id: PeerId,
}

struct HeartbeatHandle {
    stop_tx: watch::Sender<bool>,
    join_handle: JoinHandle<()>,
}

#[derive(Clone)]
struct PolicyEngine {
    allow_cross_namespace: bool,
    allowed: HashSet<String>,
    denied: HashSet<String>,
}

enum NetworkCommand {
    PublishRecord {
        record: ServiceRecord,
        ttl: Duration,
    },
    SendRequest {
        target: PeerId,
        request: RPCRequest,
        reply_tx: oneshot::Sender<Result<RPCResponse>>,
    },
    StartListening {
        addr: Multiaddr,
        reply_tx: oneshot::Sender<Result<()>>,
    },
    Dial {
        addr: Multiaddr,
        reply_tx: oneshot::Sender<Result<()>>,
    },
}

impl Network {
    pub fn new(cfg: &Config) -> Result<Self> {
        if cfg.private_key.is_empty() {
            return Err(anyhow!("config: private key is required"));
        }
        if cfg.workload_name.is_empty() || cfg.pod_name.is_empty() {
            return Err(anyhow!("config: workload name and pod name are required"));
        }

        let keypair = Keypair::from_protobuf_encoding(&cfg.private_key)
            .context("decode libp2p private key")?;
        let peer_id = keypair.public().to_peer_id();
        let peer_id_str = peer_id.to_string();

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

        let swarm = SwarmBuilder::with_existing_identity(keypair)
            .with_tokio()
            .with_quic()
            .with_dns()? 
            .with_behaviour(|key| {
                let peer_id = key.public().to_peer_id();
                let store = kad::store::MemoryStore::new(peer_id);
                let kad_config = kad::Config::default();
                let kademlia = kad::Behaviour::with_config(peer_id, store, kad_config);

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

        // Spawn the event loop
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

    pub fn start(&self) {
        // Start listening
        for addr_str in &self.inner.cfg.listen_addrs {
            if let Ok(addr) = addr_str.parse::<Multiaddr>() {
                let (tx, _rx) = oneshot::channel();
                let _ = self.cmd_tx.try_send(NetworkCommand::StartListening { addr, reply_tx: tx });
                // We don't block on this, just fire and forget for start()
            }
        }

        // Connect to bootstrap peers
        for peer_str in &self.inner.cfg.bootstrap_peer_strings {
             if let Ok(addr) = peer_str.parse::<Multiaddr>() {
                 let (tx, _rx) = oneshot::channel();
                 let _ = self.cmd_tx.try_send(NetworkCommand::Dial { addr, reply_tx: tx });
             }
        }

        if let Err(err) = self.refresh_heartbeat() {
            warn!(error = %err, "failed to publish initial service record");
        }

        let mut guard = self.inner.heartbeat.lock().expect("heartbeat lock");
        if guard.is_some() {
            return;
        }

        let (tx, mut rx) = watch::channel(false);
        let interval_duration = self.inner.cfg.heartbeat_interval;
        let network = self.clone();
        let join_handle = tokio::spawn(async move {
            let mut ticker = interval(interval_duration);
            ticker.tick().await;
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

    pub async fn shutdown(&self) {
        if let Some(handle) = self.take_heartbeat() {
            let _ = handle.stop_tx.send(true);
            let _ = handle.join_handle.await;
        }
        // In real DHT, we might want to publish a "tombstone" or rely on TTL expiration.
        // For now, we just stop refreshing.
    }

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

    pub fn update_leader_endpoint(&self, active: bool, epoch: u64) -> Result<()> {
        let mut should_publish = false;
        {
            let mut record = self.inner.local_record.write().expect("record write lock");
            let mut changed = false;
            if record.ready != active {
                record.ready = active;
                changed = true;
            }
            let leader_value = serde_json::Value::Bool(active);
            if record.caps.get("leader") != Some(&leader_value) {
                record.caps.insert("leader".into(), leader_value);
                changed = true;
            }
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

    // This now queries the local DHT view (Kademlia store)
    // Note: In a real Kademlia, we might need to perform a network query (GetRecord).
    // For simplicity and performance, we can rely on the fact that we are constantly
    // refreshing/publishing records, so they should propagate.
    // However, Kademlia doesn't automatically sync everything to everyone.
    // We might need to periodically query for our workload ID to find peers.
    pub fn find_service_peers(&self) -> Vec<ServiceRecord> {
        // TODO: Implement actual DHT query or maintain a local cache of discovered peers.
        // For now, we'll use the discovery module's static map as a fallback/cache if we were keeping it,
        // but we are replacing it.
        //
        // To make this work properly with Kademlia, we should be periodically querying the DHT
        // for keys like "namespace/workload".
        //
        // Since we are replacing the stub, we need to bridge the gap.
        // For this iteration, let's assume we are using the discovery module as a "cache"
        // that gets populated by Kademlia events.
        
        let namespace = self.inner.cfg.namespace.clone();
        let workload = self.inner.cfg.workload_name.clone();
        let local_id = self.inner.cfg.workload_id();
        
        // This calls the discovery module which we will update to be just a cache
        discovery::list(&namespace, &workload)
            .into_iter()
            .filter(|record| {
                record.workload_id == local_id || self.inner.policy.allows(&namespace, record)
            })
            .collect()
    }

    pub fn peer_id(&self) -> String {
        self.inner.peer_id.to_string()
    }

    fn refresh_heartbeat(&self) -> Result<()> {
        {
            let mut record = self.inner.local_record.write().expect("record write lock");
            record.version = record.version.saturating_add(1);
            record.ts = current_millis();
            record.nonce = Uuid::new_v4().to_string();
        }
        self.publish_current_record()
    }

    fn publish_current_record(&self) -> Result<()> {
        let record = self.local_record();
        let ttl = self.inner.cfg.dht_ttl;
        
        let cmd = NetworkCommand::PublishRecord { record, ttl };
        let _ = self.cmd_tx.try_send(cmd);
        
        Ok(())
    }

    fn local_record(&self) -> ServiceRecord {
        self.inner
            .local_record
            .read()
            .expect("record read lock")
            .clone()
    }

    fn take_heartbeat(&self) -> Option<HeartbeatHandle> {
        self.inner.heartbeat.lock().expect("heartbeat lock").take()
    }
    
    pub async fn send_request(&self, target_peer_id: &str, req: RPCRequest) -> Result<RPCResponse> {
        let peer_id = target_peer_id.parse::<PeerId>()?;
        let (tx, rx) = oneshot::channel();
        
        self.cmd_tx.send(NetworkCommand::SendRequest {
            target: peer_id,
            request: req,
            reply_tx: tx,
        }).await?;
        
        rx.await?
    }
}

async fn network_event_loop(
    mut swarm: Swarm<WorkplaneBehaviour>,
    mut cmd_rx: mpsc::Receiver<NetworkCommand>,
) {
    let mut pending_requests: HashMap<request_response::OutboundRequestId, oneshot::Sender<Result<RPCResponse>>> = HashMap::new();

    loop {
        tokio::select! {
            event = swarm.select_next_some() => {
                match event {
                    SwarmEvent::Behaviour(WorkplaneBehaviourEvent::Kademlia(kad::Event::OutboundQueryProgressed { result, .. })) => {
                        match result {
                            kad::QueryResult::GetRecord(Ok(kad::GetRecordOk::FoundRecord(peer_record))) => {
                                if let Ok(record) = serde_json::from_slice::<ServiceRecord>(&peer_record.record.value) {
                                    // Update local cache (discovery module)
                                    // We'll keep discovery::put as a way to update the in-memory cache
                                    // even though we removed the networking logic from it.
                                    discovery::put(record, Duration::from_secs(60));
                                }
                            }
                            _ => {}
                        }
                    }
                    SwarmEvent::Behaviour(WorkplaneBehaviourEvent::RequestResponse(request_response::Event::Message { peer, message, .. })) => {
                        match message {
                            request_response::Message::Request { request, channel, .. } => {
                                // Handle incoming RPC
                                // We need to call the registered handler
                                let response = crate::streams::handle_request(&peer.to_string(), request).await;
                                let _ = swarm.behaviour_mut().request_response.send_response(channel, response);
                            }
                            request_response::Message::Response { request_id, response } => {
                                if let Some(tx) = pending_requests.remove(&request_id) {
                                    let _ = tx.send(Ok(response));
                                }
                            }
                        }
                    }
                    SwarmEvent::Behaviour(WorkplaneBehaviourEvent::RequestResponse(request_response::Event::OutboundFailure { request_id, error, .. })) => {
                        if let Some(tx) = pending_requests.remove(&request_id) {
                            let _ = tx.send(Err(anyhow!("RPC failed: {:?}", error)));
                        }
                    }
                    _ => {}
                }
            }
            Some(cmd) = cmd_rx.recv() => {
                match cmd {
                    NetworkCommand::PublishRecord { record, ttl } => {
                        let key = kad::RecordKey::new(&record.workload_id);
                        if let Ok(data) = serde_json::to_vec(&record) {
                            let kad_record = kad::Record {
                                key,
                                value: data,
                                publisher: None,
                                expires: None, // TODO: Handle TTL
                            };
                            let _ = swarm.behaviour_mut().kademlia.put_record(kad_record, kad::Quorum::One);

                            // Also update local cache
                            discovery::put(record.clone(), ttl);
                        }
                    }
                    NetworkCommand::SendRequest { target, request, reply_tx } => {
                        let request_id = swarm.behaviour_mut().request_response.send_request(&target, request);
                        pending_requests.insert(request_id, reply_tx);
                    }
                    NetworkCommand::StartListening { addr, reply_tx } => {
                        let res = swarm.listen_on(addr).map(|_| ()).map_err(|e| anyhow!(e));
                        let _ = reply_tx.send(res);
                    }
                    NetworkCommand::Dial { addr, reply_tx } => {
                         let res = swarm.dial(addr).map(|_| ()).map_err(|e| anyhow!(e));
                         let _ = reply_tx.send(res);
                    }
                }
            }
        }
    }
}

impl PolicyEngine {
    fn from_config(cfg: &Config) -> Self {
        Self {
            allow_cross_namespace: cfg.allow_cross_namespace,
            allowed: cfg.allowed_workloads.iter().cloned().collect(),
            denied: cfg.denied_workloads.iter().cloned().collect(),
        }
    }

    fn allows(&self, local_namespace: &str, record: &ServiceRecord) -> bool {
        let workload_id = format!("{}/{}", record.namespace, record.workload_name);
        if self.denied.contains(&workload_id) {
            return false;
        }
        if !self.allow_cross_namespace && record.namespace != local_namespace {
            return self.allowed.contains(&workload_id);
        }
        if self.allowed.is_empty() {
            return true;
        }
        self.allowed.contains(&workload_id)
    }
}

fn current_millis() -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    now.as_millis() as i64
}
