//! # Agent – Main Orchestrator
//!
//! This module implements the core [`Agent`] struct that coordinates all workplane
//! subsystems: networking, Raft consensus, self-healing, and RPC handling.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                            Agent                                        │
//! │  ┌───────────────────────────────────────────────────────────────────┐  │
//! │  │                     RPC Handler (registered)                      │  │
//! │  │  - healthz: health check response                                 │  │
//! │  │  - leader_only: proxy to leader if not leader                     │  │
//! │  │  - default: return handled_by peer_id                             │  │
//! │  └───────────────────────────────────────────────────────────────────┘  │
//! │                                                                         │
//! │  ┌──────────────┐   ┌──────────────┐   ┌──────────────┐                 │
//! │  │   Network    │   │ RaftManager  │   │  SelfHealer  │                 │
//! │  │ (libp2p DHT) │   │ (leader)     │   │ (replicas)   │                 │
//! │  └──────┬───────┘   └──────┬───────┘   └──────┬───────┘                 │
//! │         │                  │                  │                         │
//! │         └──────────────────┼──────────────────┘                         │
//! │                            │                                            │
//! │                   ┌────────▼────────┐                                   │
//! │                   │  LeaderMonitor  │ (stateful only)                   │
//! │                   │  - 50ms tick    │                                   │
//! │                   │  - heartbeats   │                                   │
//! │                   │  - elections    │                                   │
//! │                   └─────────────────┘                                   │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Stateful vs Stateless Workloads
//!
//! | Feature              | Stateless (Deployment) | Stateful (StatefulSet) |
//! |---------------------|------------------------|------------------------|
//! | Raft consensus      | No                     | Yes                    |
//! | Leader election     | N/A (all equal)        | Yes                    |
//! | leader_only routing | All handle requests    | Leader only            |
//! | LeaderMonitor       | Not started            | Started                |
//!
//! The workload kind is determined by [`Config::is_stateful`] based on the
//! `workload_kind` field (set via `BEE_WORKLOAD_KIND` environment variable).
//!
//! ## RPC Handler
//!
//! The workload registers a global stream handler that processes incoming RPCs:
//!
//! 1. **healthz**: Returns `{ok: true, remote: peer_id, pod: pod_name}`
//! 2. **leader_only requests**: If not leader, proxy to current leader
//! 3. **default**: Returns `{ok: true, handled_by: local_peer_id}`
//!
//! ## Leader State
//!
//! The `leader_state: Arc<RwLock<Option<String>>>` tracks the current leader:
//! - For stateless workloads: Always set to local peer_id
//! - For stateful workloads: Updated by RaftManager via LeaderMonitor

use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{Result, anyhow};
use serde_json::{Map, Value};
use tokio::task::JoinHandle;
use tokio::{select, sync::watch, time::interval};
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::discovery::ServiceRecord;
use crate::network::Network;
use crate::raft::{Heartbeat, LeadershipUpdate, RaftAction, RaftManager, RaftRole, VoteRequest, VoteResponse};
use crate::selfheal::SelfHealer;
use crate::rpc::{RPCRequest, RPCResponse, register_stream_handler};

/// Main agent coordinating networking, Raft, and self-healing.
///
/// Create via [`Agent::new`], then call [`Agent::start`] to begin operations.
/// Call [`Agent::close`] for graceful shutdown.
pub struct Agent {
    /// Agent configuration.
    cfg: Config,

    /// Network layer for DHT and RPC.
    pub network: Network,

    /// Raft manager for stateful workloads (None for stateless).
    pub raft: Option<Arc<RwLock<RaftManager>>>,

    /// Self-healer for replica count management.
    selfhealer: Option<SelfHealer>,

    /// Current leader peer ID. Updated by LeaderMonitor for stateful workloads.
    leader_state: Arc<RwLock<Option<String>>>,

    /// Background task for Raft ticks and elections (stateful only).
    leader_monitor: Option<LeaderMonitor>,
}

impl Agent {
    /// Create a new agent.
    ///
    /// This initializes all subsystems but does not start background tasks.
    /// Call [`Agent::start`] to begin operations.
    ///
    /// # Arguments
    ///
    /// * `cfg` - Agent configuration (private_key, workload_name, pod_name required)
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - `private_key` is empty
    /// - `workload_name` or `pod_name` is empty
    pub fn new(cfg: Config) -> Result<Self> {
        if cfg.private_key.is_empty() {
            return Err(anyhow!("config: private key is required"));
        }
        if cfg.workload_name.is_empty() || cfg.pod_name.is_empty() {
            return Err(anyhow!("config: workload and pod name required"));
        }
        let mut cfg = cfg;
        cfg.apply_defaults();

        // Initialize network layer
        let network = Network::new(&cfg)?;
        cfg.peer_id_str = Some(network.peer_id());

        // ====================================================================
        // Register RPC Stream Handler
        // ====================================================================
        //
        // This handler processes all incoming RPC requests. Key behaviors:
        // - healthz: Simple health check response
        // - leader_only: Proxy to leader if this replica is not the leader
        // - default: Return success with local peer_id

        let leader_state: Arc<RwLock<Option<String>>> = Arc::new(RwLock::new(None));
        let handler_network = network.clone();
        let local_peer_id = handler_network.peer_id();
        let leader_state_for_handler = leader_state.clone();

        register_stream_handler(Arc::new(move |remote: &ServiceRecord, req: RPCRequest| {
            let leader_state = leader_state_for_handler.clone();
            let handler_network = handler_network.clone();
            let local_peer_id = local_peer_id.clone();
            let remote_peer_id = remote.peer_id.clone();
            let remote_pod = remote.pod_name.clone();

            Box::pin(async move {
                // ============================================================
                // Handle healthz requests (always allowed)
                // ============================================================
                if req.method.eq_ignore_ascii_case("healthz") {
                    let mut body = Map::new();
                    body.insert("remote".to_string(), Value::String(remote_peer_id));
                    if let Some(pod) = remote_pod {
                        body.insert("pod".to_string(), Value::String(pod));
                    }
                    return RPCResponse {
                        ok: true,
                        error: None,
                        body,
                    };
                }

                // ============================================================
                // Handle leader_only requests (stateful workloads)
                // ============================================================
                //
                // Per spec §8.1, if leader_only is true and we're not the leader,
                // we must proxy the request to the current leader.

                let leader = leader_state.read().expect("leader state").clone();
                let leader_matches = leader
                    .as_ref()
                    .map(|leader_id| leader_id == &local_peer_id)
                    .unwrap_or(false);

                if req.leader_only && !leader_matches {
                    if let Some(leader_id) = leader {
                        debug!(target = %leader_id, method = %req.method, "proxying follower request to leader");
                        match handler_network.send_request(&leader_id, req.clone()).await {
                            Ok(resp) => return resp,
                            Err(err) => {
                                warn!(error = %err, method = %req.method, "failed to proxy request to leader");
                                crate::increment_counter!("workplane.raft.follower_rejections");
                                return RPCResponse {
                                    ok: false,
                                    error: Some("leader unreachable".to_string()),
                                    body: Map::new(),
                                };
                            }
                        }
                    }
                    // No leader known
                    crate::increment_counter!("workplane.raft.follower_rejections");
                    return RPCResponse {
                        ok: false,
                        error: Some("not leader".to_string()),
                        body: Map::new(),
                    };
                }

                // ============================================================
                // Default handler: return success
                // ============================================================
                let mut body = Map::new();
                body.insert("handled_by".to_string(), Value::String(local_peer_id));
                RPCResponse {
                    ok: true,
                    error: None,
                    body,
                }
            })
        }));

        // ====================================================================
        // Initialize Raft (Stateful Workloads Only)
        // ====================================================================
        let mut raft = None;
        if cfg.is_stateful() {
            let peer_id = network.peer_id();
            let workload_id = cfg.workload_id();
            let mut manager = RaftManager::new(peer_id.clone(), workload_id);

            // Initialize peer set from current WDHT records
            let peer_ids: std::collections::HashSet<String> = network
                .find_service_peers()
                .into_iter()
                .map(|p| p.peer_id)
                .collect();
            manager.update_peers(peer_ids);

            // Bootstrap: if we're the only node, become leader immediately
            let actions = manager.bootstrap_if_needed();

            // Extract leader_id from leadership changed actions
            let mut new_leader = None;
            for action in &actions {
                if let crate::raft::RaftAction::LeadershipChanged(update) = action {
                    new_leader = update.leader_id.clone();
                }
            }
            *leader_state.write().expect("leader state") = new_leader;
            raft = Some(Arc::new(RwLock::new(manager)));
        } else {
            // Stateless workloads: all replicas are "leaders" (no routing constraints)
            *leader_state.write().expect("leader state") = Some(network.peer_id());
        }

        // Initialize self-healer
        let selfhealer = Some(SelfHealer::new(network.clone(), cfg.clone()));

        Ok(Self {
            cfg,
            network,
            raft,
            selfhealer,
            leader_state,
            leader_monitor: None,
        })
    }

    /// Start all background operations.
    ///
    /// This starts:
    /// - Network: listening, bootstrap connections, heartbeat publishing
    /// - SelfHealer: health probing, replica reconciliation
    /// - LeaderMonitor: Raft ticks, elections (stateful only)
    pub fn start(&mut self) {
        self.network.start();
        if let Some(sh) = self.selfhealer.as_mut() {
            sh.start();
        }
        // Only start leader monitor for stateful workloads
        if self.is_stateful() {
            self.start_leader_monitor();
        }
    }

    /// Gracefully shutdown all operations.
    ///
    /// Stops background tasks in order:
    /// 1. LeaderMonitor (stops Raft ticks)
    /// 2. SelfHealer (stops health probing)
    /// 3. Network (stops heartbeats, allows TTL expiration)
    pub async fn close(&mut self) {
        if let Some(monitor) = self.leader_monitor.take() {
            monitor.stop().await;
        }
        if let Some(sh) = self.selfhealer.as_mut() {
            sh.stop().await;
        }
        self.network.shutdown().await;
        info!("agent shut down");
    }

    /// Check if this is a stateful workload (StatefulSet).
    pub fn is_stateful(&self) -> bool {
        self.cfg.is_stateful()
    }

    /// Get the agent configuration.
    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// Start the leader monitor for stateful workloads.
    fn start_leader_monitor(&mut self) {
        if self.leader_monitor.is_some() {
            return; // Already running
        }
        let Some(raft) = self.raft.clone() else {
            return; // No Raft manager
        };

        let monitor = LeaderMonitor::new(
            self.cfg.clone(),
            self.network.clone(),
            raft,
            self.leader_state.clone(),
        );
        self.leader_monitor = Some(monitor);
    }
}

// ============================================================================
// Leader Monitor
// ============================================================================

/// Background task for Raft leader election and heartbeat management.
///
/// This monitor runs two tick loops:
/// 1. **Fast tick (50ms)**: Drives Raft state machine (heartbeats, election timeouts)
/// 2. **Slow tick (replica_check_interval)**: Updates peer set from WDHT
///
/// The fast tick interval matches the Raft heartbeat interval (50ms) to ensure
/// timely leader heartbeats and quick election timeout detection.
struct LeaderMonitor {
    /// Channel to signal shutdown.
    stop_tx: watch::Sender<bool>,
    /// Task handle for awaiting completion.
    handle: JoinHandle<()>,
}

impl LeaderMonitor {
    /// Create and start a new leader monitor.
    ///
    /// # Arguments
    ///
    /// * `cfg` - Agent configuration
    /// * `network` - Network for sending heartbeats and votes
    /// * `raft` - Shared Raft manager
    /// * `leader_state` - Shared leader state to update on leadership changes
    fn new(
        cfg: Config,
        network: Network,
        raft: Arc<RwLock<RaftManager>>,
        leader_state: Arc<RwLock<Option<String>>>,
    ) -> Self {
        let (tx, mut rx) = watch::channel(false);
        
        // Raft tick interval: 50ms (matches HEARTBEAT_INTERVAL_MS in raft.rs)
        // This ensures we send heartbeats on time and detect election timeouts quickly
        let raft_tick_interval = Duration::from_millis(50);
        let discovery_interval = cfg.replica_check_interval;
        
        let handle = tokio::spawn(async move {
            let mut raft_ticker = interval(raft_tick_interval);
            let mut discovery_ticker = interval(discovery_interval);
            raft_ticker.tick().await; // Skip first tick
            discovery_ticker.tick().await;
            
            loop {
                select! {
                    // Fast tick for Raft election timeouts and heartbeats
                    _ = raft_ticker.tick() => {
                        // Get actions from Raft state machine tick
                        let actions = {
                            let mut guard = raft.write().expect("raft lock");
                            guard.tick()
                        };
                        
                        // Process each action
                        for action in actions {
                            match action {
                                RaftAction::SendHeartbeat(hb) => {
                                    // Leader sends heartbeat to all followers
                                    LeaderMonitor::broadcast_heartbeat(&network, &cfg, &hb).await;
                                }
                                RaftAction::RequestVotes(req) => {
                                    // Candidate requests votes from all peers
                                    LeaderMonitor::request_votes(&network, &cfg, &raft, &leader_state, req).await;
                                }
                                RaftAction::LeadershipChanged(update) => {
                                    // Update shared leader state
                                    {
                                        let mut guard = leader_state.write().expect("leader state");
                                        *guard = update.leader_id.clone();
                                    }
                                    // Advertise/withdraw leader endpoint in WDHT
                                    LeaderMonitor::apply_network_update(&network, &update);
                                }
                            }
                        }
                    }
                    
                    // Slow tick for service discovery updates
                    _ = discovery_ticker.tick() => {
                        // Update Raft peer set from current WDHT records
                        let records = network.find_service_peers();
                        let relevant: std::collections::HashSet<String> = records
                            .into_iter()
                            .filter(|record| record.workload_id == cfg.workload_id())
                            .map(|r| r.peer_id)
                            .collect();
                        {
                            let mut guard = raft.write().expect("raft lock");
                            guard.update_peers(relevant);
                        }
                    }
                    
                    // Shutdown signal
                    changed = rx.changed() => {
                        if changed.is_ok() && *rx.borrow() {
                            break;
                        }
                    }
                }
            }
        });

        Self {
            stop_tx: tx,
            handle,
        }
    }
    
    /// Broadcast heartbeat to all known peers (called by leader).
    ///
    /// Sends a `raft.heartbeat` RPC to all peers in the same workload except self.
    async fn broadcast_heartbeat(network: &Network, cfg: &Config, heartbeat: &Heartbeat) {
        let records = network.find_service_peers();
        let peers: Vec<_> = records
            .into_iter()
            .filter(|r| r.workload_id == cfg.workload_id() && r.peer_id != heartbeat.leader_id)
            .collect();
        
        for peer in peers {
            // Serialize heartbeat to JSON body
            let body = match serde_json::to_value(heartbeat) {
                Ok(Value::Object(map)) => map,
                _ => Map::new(),
            };
            let req = RPCRequest {
                method: "raft.heartbeat".to_string(),
                leader_only: false,
                body,
            };
            
            if let Err(e) = network.send_request(&peer.peer_id, req).await {
                debug!(peer = %peer.peer_id, error = %e, "failed to send heartbeat");
            }
        }
    }
    
    /// Request votes from all peers during election (called by candidate).
    ///
    /// Sends a `raft.vote` RPC to all peers. Responses are processed immediately
    /// to check for majority and transition to leader if won.
    async fn request_votes(
        network: &Network,
        cfg: &Config,
        raft: &Arc<RwLock<RaftManager>>,
        leader_state: &Arc<RwLock<Option<String>>>,
        vote_request: VoteRequest,
    ) {
        let records = network.find_service_peers();
        let peers: Vec<_> = records
            .into_iter()
            .filter(|r| r.workload_id == cfg.workload_id() && r.peer_id != vote_request.candidate_id)
            .collect();
        
        for peer in peers {
            // Serialize vote request to JSON body
            let body = match serde_json::to_value(&vote_request) {
                Ok(Value::Object(map)) => map,
                _ => Map::new(),
            };
            let req = RPCRequest {
                method: "raft.vote".to_string(),
                leader_only: false,
                body,
            };
            
            match network.send_request(&peer.peer_id, req).await {
                Ok(resp) if resp.ok => {
                    // Parse vote response and handle it
                    if let Ok(vote_resp) = serde_json::from_value::<VoteResponse>(
                        serde_json::Value::Object(resp.body)
                    ) {
                        let actions = {
                            let mut guard = raft.write().expect("raft lock");
                            guard.handle_vote_response(vote_resp)
                        };
                        
                        // Process any leadership changes
                        for action in actions {
                            if let RaftAction::LeadershipChanged(update) = action {
                                {
                                    let mut guard = leader_state.write().expect("leader state");
                                    *guard = update.leader_id.clone();
                                }
                                LeaderMonitor::apply_network_update(network, &update);
                            }
                        }
                    }
                }
                Ok(_) => {
                    debug!(peer = %peer.peer_id, "vote request rejected");
                }
                Err(e) => {
                    debug!(peer = %peer.peer_id, error = %e, "failed to request vote");
                }
            }
        }
    }

    /// Stop the monitor and wait for the task to complete.
    async fn stop(self) {
        let _ = self.stop_tx.send(true);
        let _ = self.handle.await;
    }

    /// Apply a leadership change to the network (advertise/withdraw leader endpoint).
    ///
    /// When becoming leader, sets `caps.leader = true` in the WDHT record.
    /// When stepping down, sets `caps.leader = false`.
    fn apply_network_update(network: &Network, update: &LeadershipUpdate) {
        match update.role {
            RaftRole::Leader => {
                if let Err(err) = network.update_leader_endpoint(true, update.term) {
                    warn!(error = %err, "failed to advertise leader endpoint");
                }
            }
            RaftRole::Follower => {
                if let Err(err) = network.update_leader_endpoint(false, update.term) {
                    warn!(error = %err, "failed to withdraw leader endpoint");
                }
            }
            RaftRole::Detached => {
                // No network update needed for detached state
            }
        }
    }
}
