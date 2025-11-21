//! Scheduler module for Decentralized Bidding
//!
//! This module implements the "Pull" model where nodes listen for Tasks,
//! evaluate their fit, submit Bids, and if they win, acquire a Lease.

use crate::messages::constants::{
    DEFAULT_SELECTION_WINDOW_MS, SCHEDULER_EVENTS, SCHEDULER_PROPOSALS, SCHEDULER_TENDERS,
};
use crate::messages::machine;
use crate::network::behaviour::MyBehaviour;
use libp2p::Swarm;
use libp2p::gossipsub;
use log::{debug, error, info, warn};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio::time::sleep;

/// Scheduler manages the bidding lifecycle
pub struct Scheduler {
    /// Local node ID (PeerId string)
    local_node_id: String,
    /// Active bids we are tracking: TaskID -> BidContext
    active_bids: Arc<Mutex<HashMap<String, BidContext>>>,
    /// Channel to send messages back to the network loop
    outbound_tx: mpsc::UnboundedSender<SchedulerCommand>,
}

struct BidContext {
    task_id: String,
    manifest_id: String,
    placement_metadata: HashMap<String, String>,
    replicas: u32,
    bids: Vec<BidEntry>,
}

#[derive(Clone)]
struct BidEntry {
    bidder_id: String,
    score: f64,
}

/// Commands emitted by the scheduler for the libp2p network loop to process
#[derive(Debug, Clone)]
pub enum SchedulerCommand {
    Publish { topic: String, payload: Vec<u8> },
}

impl Scheduler {
    pub fn new(
        local_node_id: String,
        outbound_tx: mpsc::UnboundedSender<SchedulerCommand>,
    ) -> Self {
        Self {
            local_node_id,
            active_bids: Arc::new(Mutex::new(HashMap::new())),
            outbound_tx,
        }
    }

    /// Handle incoming Gossipsub message
    pub async fn handle_message(
        &self,
        topic_hash: &gossipsub::TopicHash,
        message: &gossipsub::Message,
    ) {
        let tenders = gossipsub::IdentTopic::new(SCHEDULER_TENDERS).hash();
        let proposals = gossipsub::IdentTopic::new(SCHEDULER_PROPOSALS).hash();
        let events = gossipsub::IdentTopic::new(SCHEDULER_EVENTS).hash();

        if *topic_hash == tenders {
            self.handle_task(message).await;
        } else if *topic_hash == proposals {
            self.handle_bid(message).await;
        } else if *topic_hash == events {
            self.handle_event(message).await;
        }
    }

    /// Process a Task message: Evaluate -> Bid
    async fn handle_task(&self, message: &gossipsub::Message) {
        match machine::root_as_task(&message.data) {
            Ok(task) => {
                let task_id = task.id.clone();
                info!("Received Task: {}", task_id);

                let manifest_id = task.id.clone();

                let replicas = std::cmp::max(1, task.max_parallel_duplicates);

                let mut placement_metadata = HashMap::new();
                placement_metadata.insert("task_id".to_string(), task_id.clone());
                placement_metadata.insert("manifest_ref".to_string(), task.manifest_ref.clone());
                placement_metadata
                    .insert("placement_token".to_string(), task.placement_token.clone());

                // 1. Evaluate Fit (Capacity Check)
                // TODO: Connect to CapacityVerifier
                let capacity_score = 1.0; // Placeholder: assume perfect fit for now

                // 2. Calculate Bid Score
                // Score = (ResourceFit * 0.4) + (NetworkLocality * 0.3) + (Reputation * 0.3)
                // For now, just use random or static for testing
                let my_score = capacity_score * 0.8 + 0.2; // Simple formula

                // 3. Create BidContext
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64;

                {
                    let mut bids = self.active_bids.lock().unwrap();
                    bids.insert(
                        task_id.to_string(),
                        BidContext {
                            task_id: task_id.to_string(),
                            manifest_id: manifest_id.clone(),
                            placement_metadata: placement_metadata.clone(),
                            replicas,
                            bids: vec![BidEntry {
                                bidder_id: self.local_node_id.clone(),
                                score: my_score,
                            }],
                        },
                    );
                }

                // 4. Publish Bid
                let bid_bytes = machine::build_bid(
                    &task_id,
                    &self.local_node_id,
                    my_score,
                    capacity_score,
                    0.5, // Network locality placeholder
                    now,
                    &[], // Signature placeholder
                );

                if let Err(e) = self.outbound_tx.send(SchedulerCommand::Publish {
                    topic: SCHEDULER_PROPOSALS.to_string(),
                    payload: bid_bytes,
                }) {
                    error!("Failed to queue bid for task {}: {}", task_id, e);
                } else {
                    info!("Queued Bid for task {} with score {:.2}", task_id, my_score);
                }

                // 5. Spawn Selection Window Waiter
                let active_bids = self.active_bids.clone();
                let task_id_clone = task_id.to_string();
                let local_id = self.local_node_id.clone();
                let placement_sender = self.outbound_tx.clone();
                // We need a way to trigger deployment, for now just log
                tokio::spawn(async move {
                    sleep(Duration::from_millis(DEFAULT_SELECTION_WINDOW_MS)).await;

                    let (winners, manifest_id) = {
                        let mut bids = active_bids.lock().unwrap();
                        if let Some(ctx) = bids.remove(&task_id_clone) {
                            let winners = select_winners(&ctx, &local_id);
                            if !winners.is_empty() {
                                let summary = winners
                                    .iter()
                                    .map(|w| format!("{} ({:.2})", w.bidder_id, w.score))
                                    .collect::<Vec<_>>()
                                    .join(", ");
                                info!(
                                    "Selected winners for task {} (manifest {}): {}",
                                    ctx.task_id, ctx.manifest_id, summary
                                );
                            } else {
                                info!(
                                    "No qualifying bids for task {} (manifest {})",
                                    ctx.task_id, ctx.manifest_id
                                );
                            }
                            (winners, Some(ctx.manifest_id))
                        } else {
                            (Vec::new(), None)
                        }
                    };

                    if winners.iter().any(|bid| bid.bidder_id == local_id) {
                        info!(
                            "WON BID for task {}! Proceeding to deployment...",
                            task_id_clone
                        );
                        info!("Deploying workload for task {}", task_id_clone);

                        // TODO: Trigger workload deployment via runtime integration
                    } else {
                        info!("Lost bid for task {}.", task_id_clone);
                    }
                });
            }
            Err(e) => error!("Failed to parse Task message: {}", e),
        }
    }

    /// Process a Bid message: Update highest bid
    async fn handle_bid(&self, message: &gossipsub::Message) {
        match machine::root_as_bid(&message.data) {
            Ok(bid) => {
                let task_id = bid.task_id.clone();
                let bidder_id = bid.node_id.clone();
                let score = bid.score;

                // Ignore our own bids (handled locally)
                if bidder_id == self.local_node_id {
                    return;
                }

                let mut bids = self.active_bids.lock().unwrap();
                if let Some(ctx) = bids.get_mut(&task_id) {
                    info!(
                        "Recorded bid for task {}: {:.2} from {}",
                        task_id, score, bidder_id
                    );
                    ctx.bids.push(BidEntry {
                        bidder_id: bidder_id.to_string(),
                        score,
                    });
                }
            }
            Err(e) => error!("Failed to parse Bid message: {}", e),
        }
    }

    /// Process Scheduler Events (e.g. Cancelled, Preempted)
    async fn handle_event(&self, message: &gossipsub::Message) {
        match machine::root_as_scheduler_event(&message.data) {
            Ok(event) => {
                let task_id = event.task_id.clone();
                info!(
                    "Received Scheduler Event for task {}: {:?}",
                    task_id, event.event_type
                );

                if event.node_id == self.local_node_id {
                    use crate::messages::types::EventType;

                    if matches!(
                        event.event_type,
                        EventType::Cancelled | EventType::Preempted | EventType::Failed
                    ) {
                        info!(
                            "Received termination event for locally deployed task {}",
                            task_id
                        );
                    }
                }
            }
            Err(e) => error!("Failed to parse SchedulerEvent message: {}", e),
        }
    }
}

#[derive(Clone)]
struct BidOutcome {
    bidder_id: String,
    score: f64,
    placement_metadata: HashMap<String, String>,
}

fn select_winners(context: &BidContext, local_node_id: &str) -> Vec<BidOutcome> {
    let mut best_by_node: HashMap<String, BidEntry> = HashMap::new();

    for bid in &context.bids {
        let entry = best_by_node
            .entry(bid.bidder_id.clone())
            .or_insert_with(|| bid.clone());

        if bid.score > entry.score {
            *entry = bid.clone();
        }
    }

    let mut bids: Vec<BidEntry> = best_by_node.into_values().collect();
    bids.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut selected_nodes = HashSet::new();
    let mut outcomes = Vec::new();

    for bid in bids.into_iter() {
        if outcomes.len() as u32 >= context.replicas {
            break;
        }

        // Enforce that the local node can never be selected for more than one replica
        if bid.bidder_id == local_node_id && selected_nodes.contains(local_node_id) {
            continue;
        }

        if selected_nodes.insert(bid.bidder_id.clone()) {
            outcomes.push(BidOutcome {
                bidder_id: bid.bidder_id,
                score: bid.score,
                placement_metadata: context.placement_metadata.clone(),
            });
        }
    }

    outcomes
}

// ---------------------------------------------------------------------------
pub use runtime_integration::*;

// ---------------------------------------------------------------------------
// Runtime integration and apply handling (merged from run.rs)
// ---------------------------------------------------------------------------

mod runtime_integration {
    //! Workload Integration Module
    //!
    //! This module provides integration between the existing libp2p apply message handling
    //! and the new workload manager system. It updates the apply message handler to use
    //! the runtime engines and provider announcement system.

    use crate::capacity::CapacityVerifier;
    use crate::messages::machine;
    use crate::messages::types::ApplyRequest;
    use crate::network::behaviour::MyBehaviour;
    use crate::runtimes::{DeploymentConfig, RuntimeRegistry, create_default_registry};
    use libp2p::Swarm;
    use libp2p::kad;
    use libp2p::request_response;
    use log::{debug, error, info, warn};
    use once_cell::sync::Lazy;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    /// Global runtime registry for all available engines
    static RUNTIME_REGISTRY: Lazy<Arc<RwLock<Option<RuntimeRegistry>>>> =
        Lazy::new(|| Arc::new(RwLock::new(None)));

    /// Global capacity verifier for resource checks
    static CAPACITY_VERIFIER: Lazy<Arc<CapacityVerifier>> =
        Lazy::new(|| Arc::new(CapacityVerifier::new()));

    /// Node-local cache mapping manifest IDs to owner public keys.
    static MANIFEST_OWNER_MAP: Lazy<RwLock<HashMap<String, Vec<u8>>>> =
        Lazy::new(|| RwLock::new(HashMap::new()));

    /// Initialize the runtime registry and provider manager
    pub async fn initialize_podman_manager(
        force_mock_runtime: bool,
        mock_only_runtime: bool,
        scheduling_enabled: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if !scheduling_enabled {
            info!(
                "Scheduling disabled; skipping runtime registry and provider manager initialization"
            );
            return Ok(());
        }

        info!("Initializing runtime registry for manifest deployment");

        // Create runtime registry - use mock-only for tests if environment variable is set
        let use_mock_registry = force_mock_runtime || mock_only_runtime;

        #[cfg(debug_assertions)]
        let registry = if use_mock_registry {
            info!("Using mock-only runtime registry for testing");
            crate::runtimes::create_mock_only_registry().await
        } else {
            create_default_registry().await
        };

        #[cfg(not(debug_assertions))]
        let registry = {
            if use_mock_registry {
                warn!(
                    "mock runtime requested but not compiled in release build; falling back to default registry"
                );
            }
            create_default_registry().await
        };
        let available_engines = registry.check_available_engines().await;

        info!("Available runtime engines: {:?}", available_engines);

        // Store the registry globally
        {
            let mut global_registry = RUNTIME_REGISTRY.write().await;
            *global_registry = Some(registry);
        }

        // Initialize capacity verifier with system resources
        info!("Initializing capacity verifier");
        let verifier = get_global_capacity_verifier();
        if let Err(e) = verifier.update_system_resources().await {
            warn!("Failed to update system resources: {}", e);
        }

        info!("Runtime registry initialized successfully");
        Ok(())
    }

    /// Record the owner public key for a manifest on this node.
    pub async fn record_manifest_owner(manifest_id: &str, owner_pubkey: &[u8]) {
        let mut map = MANIFEST_OWNER_MAP.write().await;
        map.insert(manifest_id.to_string(), owner_pubkey.to_vec());
        info!(
            "record_manifest_owner: stored owner_pubkey len={} for manifest_id={}",
            owner_pubkey.len(),
            manifest_id
        );
    }

    /// Retrieve the owner public key for a manifest if known.
    pub async fn get_manifest_owner(manifest_id: &str) -> Option<Vec<u8>> {
        let map = MANIFEST_OWNER_MAP.read().await;
        map.get(manifest_id).cloned()
    }

    /// Remove the owner mapping for a manifest.
    pub async fn remove_manifest_owner(manifest_id: &str) -> Option<Vec<u8>> {
        let mut map = MANIFEST_OWNER_MAP.write().await;
        map.remove(manifest_id)
    }

    /// Get access to the global runtime registry (for testing and debug endpoints)
    pub async fn get_global_runtime_registry()
    -> Option<tokio::sync::RwLockReadGuard<'static, Option<RuntimeRegistry>>> {
        let registry_guard = RUNTIME_REGISTRY.read().await;
        if registry_guard.is_some() {
            Some(registry_guard)
        } else {
            None
        }
    }

    /// Get access to the global capacity verifier
    pub fn get_global_capacity_verifier() -> Arc<CapacityVerifier> {
        Arc::clone(&CAPACITY_VERIFIER)
    }

    /// Enhanced apply message handler that uses the workload manager
    pub async fn handle_apply_message_with_podman_manager(
        message: request_response::Message<Vec<u8>, Vec<u8>>,
        peer: libp2p::PeerId,
        swarm: &mut Swarm<MyBehaviour>,
        _local_peer: libp2p::PeerId,
    ) {
        match message {
            request_response::Message::Request {
                request, channel, ..
            } => {
                info!("Received apply request from peer={}", peer);

                // Parse the apply request
                match machine::root_as_apply_request(&request) {
                    Ok(apply_req) => {
                        info!(
                            "Apply request - operation_id={:?} replicas={}",
                            apply_req.operation_id, apply_req.replicas
                        );

                        let owner_pubkey = Vec::new();

                        // Extract and validate manifest
                        if !apply_req.manifest_json.is_empty() {
                            let manifest_id = apply_req.manifest_id.as_str();
                            if manifest_id.is_empty() {
                                warn!(
                                    "Apply request missing manifest_id; rejecting from peer={}",
                                    peer
                                );
                                let error_response = machine::build_apply_response(
                                    false,
                                    "unknown",
                                    "missing manifest id",
                                );
                                let _ = swarm
                                    .behaviour_mut()
                                    .apply_rr
                                    .send_response(channel, error_response);
                                return;
                            }

                            let reservation_ok = get_global_capacity_verifier()
                                .has_active_reservation_for_manifest(manifest_id)
                                .await;
                            if !reservation_ok {
                                warn!(
                                    "Apply request for manifest_id={} from peer={} without prior reservation",
                                    manifest_id, peer
                                );
                                let error_response = machine::build_apply_response(
                                    false,
                                    manifest_id,
                                    "no active capacity reservation",
                                );
                                let _ = swarm
                                    .behaviour_mut()
                                    .apply_rr
                                    .send_response(channel, error_response);
                                return;
                            }

                            match process_manifest_deployment(
                                swarm,
                                &apply_req,
                                &apply_req.manifest_json,
                                &owner_pubkey,
                            )
                            .await
                            {
                                Ok(workload_id) => {
                                    info!(
                                        "Successfully deployed workload {} for apply request",
                                        workload_id
                                    );

                                    // Send success response
                                    let success_response = machine::build_apply_response(
                                        true,
                                        &workload_id,
                                        "workload deployed successfully",
                                    );
                                    let _ = swarm
                                        .behaviour_mut()
                                        .apply_rr
                                        .send_response(channel, success_response);
                                }
                                Err(e) => {
                                    error!("Failed to deploy workload for apply request: {}", e);

                                    // Send error response
                                    let error_message = format!("deployment failed: {}", e);
                                    let error_response = machine::build_apply_response(
                                        false,
                                        "unknown",
                                        &error_message,
                                    );
                                    let _ = swarm
                                        .behaviour_mut()
                                        .apply_rr
                                        .send_response(channel, error_response);
                                }
                            }
                        } else {
                            warn!("Apply request missing manifest JSON");
                            let error_response = machine::build_apply_response(
                                false,
                                "unknown",
                                "missing manifest JSON",
                            );
                            let _ = swarm
                                .behaviour_mut()
                                .apply_rr
                                .send_response(channel, error_response);
                        }
                    }
                    Err(e) => {
                        error!("Failed to parse apply request: {}", e);
                        let error_response = machine::build_apply_response(
                            false,
                            "unknown",
                            "invalid apply request format",
                        );
                        let _ = swarm
                            .behaviour_mut()
                            .apply_rr
                            .send_response(channel, error_response);
                    }
                }
            }
            request_response::Message::Response { .. } => {
                debug!("Received apply response from peer={}", peer);
                // Handle response if needed
            }
        }
    }

    /// Process manifest deployment using the workload manager
    async fn process_manifest_deployment(
        swarm: &mut Swarm<MyBehaviour>,
        apply_req: &ApplyRequest,
        manifest_json: &str,
        owner_pubkey: &[u8],
    ) -> Result<String, Box<dyn std::error::Error>> {
        info!("Processing manifest deployment");

        // Parse the manifest content (always cleartext)
        let manifest_content = manifest_json.as_bytes().to_vec();

        // Use the manifest_id from the apply request for placement announcements
        // This ensures consistency between apply and delete operations
        let manifest_id = if apply_req.manifest_id.is_empty() {
            "unknown".to_string()
        } else {
            apply_req.manifest_id.clone()
        };

        info!(
            "Processing manifest deployment for manifest_id: {}",
            manifest_id
        );

        if owner_pubkey.is_empty() {
            warn!(
                "process_manifest_deployment: missing owner pubkey for manifest_id={}",
                manifest_id
            );
        } else {
            record_manifest_owner(&manifest_id, owner_pubkey).await;
        }

        // Modify manifest to set replicas=1 for this node deployment
        // The original manifest is stored in DHT, but each node deploys with replicas=1
        let modified_manifest_content = modify_manifest_replicas(&manifest_content)?;

        // Create deployment configuration
        let deployment_config = create_deployment_config(apply_req);

        // Select appropriate runtime engine based on manifest type
        let engine_name = select_runtime_engine(&modified_manifest_content).await?;
        info!(
            "Selected runtime engine '{}' for manifest_id: {}",
            engine_name, manifest_id
        );

        // Get runtime registry
        let registry_guard = RUNTIME_REGISTRY.read().await;
        let registry = registry_guard
            .as_ref()
            .ok_or("Runtime registry not initialized")?;

        // Get the selected engine
        let engine = registry
            .get_engine(&engine_name)
            .ok_or(format!("Runtime engine '{}' not available", engine_name))?;

        // Deploy the workload with modified manifest (replicas=1)
        // use peer-aware deployment for mock engine when compiled in debug builds
        #[cfg(debug_assertions)]
        let workload_info = {
            if engine_name == "mock" {
                if let Some(mock_engine) = engine
                    .as_any()
                    .downcast_ref::<crate::runtimes::mock::MockEngine>()
                {
                    debug!("Using peer-aware deployment for mock engine");
                    mock_engine
                        .deploy_workload_with_peer(
                            &manifest_id,
                            &modified_manifest_content,
                            &deployment_config,
                            *swarm.local_peer_id(),
                        )
                        .await?
                } else {
                    engine
                        .deploy_workload(
                            &manifest_id,
                            &modified_manifest_content,
                            &deployment_config,
                        )
                        .await?
                }
            } else {
                engine
                    .deploy_workload(&manifest_id, &modified_manifest_content, &deployment_config)
                    .await?
            }
        };

        #[cfg(not(debug_assertions))]
        let workload_info = {
            if engine_name == "mock" {
                warn!(
                    "mock runtime selected but not included in release build; proceeding with default deployment path"
                );
            }
            engine
                .deploy_workload(&manifest_id, &modified_manifest_content, &deployment_config)
                .await?
        };

        info!(
            "Workload deployed successfully: {} using engine '{}', status: {:?}",
            workload_info.id, engine_name, workload_info.status
        );

        publish_workload_to_dht(swarm, &workload_info.id);

        Ok(workload_info.id)
    }

    /// Modify the manifest to set replicas=1 for single-node deployment
    /// Each node in the fabric will deploy one replica of the workload
    fn modify_manifest_replicas(
        manifest_content: &[u8],
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let manifest_str = String::from_utf8_lossy(manifest_content);

        // Try to parse as YAML/JSON and modify replicas field
        if let Ok(mut doc) = serde_yaml::from_str::<serde_yaml::Value>(&manifest_str) {
            // Check for spec.replicas field (Kubernetes-style)
            if let Some(spec) = doc.get_mut("spec") {
                if let Some(spec_map) = spec.as_mapping_mut() {
                    spec_map.insert(
                        serde_yaml::Value::String("replicas".to_string()),
                        serde_yaml::Value::Number(serde_yaml::Number::from(1)),
                    );
                }
            }
            // Check for top-level replicas field
            else if doc.get("replicas").is_some() {
                if let Some(doc_map) = doc.as_mapping_mut() {
                    doc_map.insert(
                        serde_yaml::Value::String("replicas".to_string()),
                        serde_yaml::Value::Number(serde_yaml::Number::from(1)),
                    );
                }
            }

            // Convert back to YAML bytes
            let modified_yaml = serde_yaml::to_string(&doc)?;
            info!("Modified manifest to set replicas=1 for single-node deployment");
            Ok(modified_yaml.into_bytes())
        } else {
            // If parsing fails, return original content unchanged
            warn!("Failed to parse manifest for replica modification, using original");
            Ok(manifest_content.to_vec())
        }
    }

    /// Select the appropriate runtime engine based on manifest content and annotations
    async fn select_runtime_engine(
        manifest_content: &[u8],
    ) -> Result<String, Box<dyn std::error::Error>> {
        let manifest_str = String::from_utf8_lossy(manifest_content);

        // Try to parse as YAML and look for annotations
        if let Ok(doc) = serde_yaml::from_str::<serde_yaml::Value>(&manifest_str) {
            // Check for runtime engine annotation
            if let Some(metadata) = doc.get("metadata") {
                if let Some(annotations) = metadata.get("annotations") {
                    if let Some(engine) = annotations
                        .get("beemesh.io/runtime-engine")
                        .and_then(|v| v.as_str())
                    {
                        info!("Found runtime engine annotation: {}", engine);
                        return Ok(engine.to_string());
                    }
                }
            }

            // Check manifest type - note preferences, but don't hardcode
            if let Some(kind) = doc.get("kind").and_then(|k| k.as_str()) {
                match kind {
                    "Pod" | "Deployment" | "Service" | "ConfigMap" | "Secret" => {
                        // Kubernetes resources - will prefer Podman below if available
                        debug!("Detected Kubernetes manifest kind: {}", kind);
                    }
                    _ => {}
                }
            }

            // Check for Docker Compose format - note preference, but don't hardcode
            if doc.get("services").is_some() && doc.get("version").is_some() {
                debug!("Detected Docker Compose manifest");
                // Will prefer Docker below if available
            }
        }

        // Get available engines and select the best one
        if let Some(registry) = RUNTIME_REGISTRY.read().await.as_ref() {
            let available = registry.check_available_engines().await;

            // Prefer Podman, then Docker, then mock for testing
            if *available.get("podman").unwrap_or(&false) {
                return Ok("podman".to_string());
            } else if *available.get("docker").unwrap_or(&false) {
                return Ok("docker".to_string());
            } else if *available.get("mock").unwrap_or(&false) {
                return Ok("mock".to_string());
            }
        }

        Err("No suitable runtime engine available".into())
    }

    /// Create deployment configuration from apply request
    fn create_deployment_config(apply_req: &ApplyRequest) -> DeploymentConfig {
        let mut config = DeploymentConfig::default();

        // Set replicas
        config.replicas = apply_req.replicas;

        // Metadata from apply request can be added here if needed
        if !apply_req.operation_id.is_empty() {
            config.env.insert(
                "BEEMESH_OPERATION_ID".to_string(),
                apply_req.operation_id.clone(),
            );
        }

        config
    }

    /// Store the workload id in the machine DHT using a simple key/value record
    fn publish_workload_to_dht(swarm: &mut Swarm<MyBehaviour>, workload_id: &str) {
        let record_key = kad::RecordKey::new(&format!("workload:{}", workload_id));
        let record = kad::Record {
            key: record_key,
            value: workload_id.as_bytes().to_vec(),
            publisher: None,
            expires: None,
        };

        match swarm
            .behaviour_mut()
            .kademlia
            .put_record(record, kad::Quorum::One)
        {
            Ok(query_id) => {
                info!(
                    "DHT: stored workload {} with query_id {:?} as single source of truth",
                    workload_id, query_id
                );
            }
            Err(e) => {
                warn!(
                    "Failed to store workload {} in machine DHT: {:?}",
                    workload_id, e
                );
            }
        }
    }

    /// Enhanced self-apply processing with workload manager
    pub async fn process_enhanced_self_apply_request(
        manifest: &[u8],
        swarm: &mut Swarm<MyBehaviour>,
    ) {
        debug!(
            "Processing enhanced self-apply request (manifest len={})",
            manifest.len()
        );

        match machine::root_as_apply_request(manifest) {
            Ok(apply_req) => {
                debug!(
                    "Enhanced self-apply request - operation_id={:?} replicas={}",
                    apply_req.operation_id, apply_req.replicas
                );

                if !apply_req.manifest_json.is_empty() {
                    let manifest_json = apply_req.manifest_json.clone();
                    let owner_pubkey = Vec::new();

                    match process_manifest_deployment(
                        swarm,
                        &apply_req,
                        &manifest_json,
                        &owner_pubkey,
                    )
                    .await
                    {
                        Ok(workload_id) => {
                            info!(
                                "Successfully deployed self-applied workload: {}",
                                workload_id
                            );
                        }
                        Err(e) => {
                            error!("Failed to deploy self-applied workload: {}", e);
                        }
                    }
                } else {
                    warn!("Self-apply request missing manifest JSON");
                }
            }
            Err(e) => {
                error!("Failed to parse self-apply request: {}", e);
            }
        }
    }

    /// Get runtime registry statistics
    pub async fn get_runtime_registry_stats() -> HashMap<String, bool> {
        if let Some(registry) = RUNTIME_REGISTRY.read().await.as_ref() {
            registry.check_available_engines().await
        } else {
            HashMap::new()
        }
    }

    /// List all available runtime engines
    pub async fn list_available_engines() -> Vec<String> {
        if let Some(registry) = RUNTIME_REGISTRY.read().await.as_ref() {
            registry
                .list_engines()
                .iter()
                .map(|s| s.to_string())
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Remove a workload by ID (requires engine name)
    pub async fn remove_workload_by_id(
        workload_id: &str,
        engine_name: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let registry_guard = RUNTIME_REGISTRY.read().await;
        let registry = registry_guard
            .as_ref()
            .ok_or("Runtime registry not initialized")?;

        let engine = registry
            .get_engine(engine_name)
            .ok_or(format!("Runtime engine '{}' not available", engine_name))?;

        engine.remove_workload(workload_id).await?;
        info!(
            "Successfully removed workload: {} from engine: {}",
            workload_id, engine_name
        );
        Ok(())
    }

    /// Remove workloads by manifest ID - searches through all engines and removes matching workloads
    pub async fn remove_workloads_by_manifest_id(
        manifest_id: &str,
    ) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        info!(
            "remove_workloads_by_manifest_id: manifest_id={}",
            manifest_id
        );

        let registry_guard = RUNTIME_REGISTRY.read().await;
        let registry = registry_guard
            .as_ref()
            .ok_or("Runtime registry not initialized")?;

        let mut removed_workloads = Vec::new();
        let mut errors = Vec::new();
        let engine_names = registry.list_engines();

        for engine_name in &engine_names {
            if let Some(engine) = registry.get_engine(engine_name) {
                info!(
                    "Checking engine '{}' for workloads with manifest_id '{}'",
                    engine_name, manifest_id
                );

                // List workloads from this engine
                match engine.list_workloads().await {
                    Ok(workloads) => {
                        // Find workloads that match the manifest_id
                        for workload in workloads {
                            if workload.manifest_id == manifest_id {
                                info!(
                                    "Found matching workload: {} in engine '{}'",
                                    workload.id, engine_name
                                );

                                // Remove the workload
                                match engine.remove_workload(&workload.id).await {
                                    Ok(()) => {
                                        info!(
                                            "Successfully removed workload: {} from engine '{}'",
                                            workload.id, engine_name
                                        );
                                        removed_workloads.push(workload.id);
                                    }
                                    Err(e) => {
                                        error!(
                                            "Failed to remove workload {} from engine '{}': {}",
                                            workload.id, engine_name, e
                                        );
                                        errors.push(format!("{}:{}", engine_name, workload.id));
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!(
                            "Failed to list workloads from engine '{}': {}",
                            engine_name, e
                        );
                        errors.push(format!("list:{}", engine_name));
                    }
                }
            }
        }

        if errors.is_empty() {
            // Drop the cached owner once the manifest workloads are removed successfully.
            if remove_manifest_owner(manifest_id).await.is_some() {
                info!(
                    "remove_workloads_by_manifest_id: cleared owner mapping for manifest_id={}",
                    manifest_id
                );
            }
        } else {
            warn!(
                "remove_workloads_by_manifest_id: retaining owner mapping for manifest_id={} due to errors {:?}",
                manifest_id, errors
            );
        }

        info!(
            "remove_workloads_by_manifest_id completed: removed {} workloads for manifest_id '{}'",
            removed_workloads.len(),
            manifest_id
        );
        Ok(removed_workloads)
    }

    /// Get logs from a workload (requires engine name)
    pub async fn get_workload_logs_by_id(
        workload_id: &str,
        engine_name: &str,
        tail: Option<usize>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let registry_guard = RUNTIME_REGISTRY.read().await;
        let registry = registry_guard
            .as_ref()
            .ok_or("Runtime registry not initialized")?;

        let engine = registry
            .get_engine(engine_name)
            .ok_or(format!("Runtime engine '{}' not available", engine_name))?;

        let logs = engine.get_workload_logs(workload_id, tail).await?;
        Ok(logs)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[tokio::test]
        async fn test_runtime_registry_initialization() {
            let result = initialize_podman_manager(false, false, true).await;
            assert!(result.is_ok());

            let stats = get_runtime_registry_stats().await;
            assert!(!stats.is_empty());

            let engines = list_available_engines().await;
            assert!(!engines.is_empty());
            assert!(engines.contains(&"mock".to_string()));
        }

        #[tokio::test]
        async fn test_runtime_engine_selection() {
            // Initialize registry for testing
            let _ = initialize_podman_manager(false, false, true).await;

            // Test Kubernetes manifest
            let k8s_manifest = r#"
    apiVersion: v1
    kind: Pod
    metadata:
      name: test-pod
    spec:
      containers:
      - name: nginx
        image: nginx:latest
    "#;
            let engine = select_runtime_engine(k8s_manifest.as_bytes()).await;
            assert!(engine.is_ok());
            // Should select mock engine in test environment
            let engine_name = engine.unwrap();
            assert!(engine_name == "mock" || engine_name == "podman");

            // Test Docker Compose manifest
            let docker_manifest = r#"
    version: '3.8'
    services:
      web:
        image: nginx:latest
        ports:
          - "80:80"
    "#;
            let engine = select_runtime_engine(docker_manifest.as_bytes()).await;
            assert!(engine.is_ok());
            // Should prefer docker for compose files, but fall back to available engines
        }

        #[test]
        fn test_deployment_config_creation() {
            // This would require creating a mock ApplyRequest
            let config = DeploymentConfig::default();
            assert_eq!(config.replicas, 1);
            assert!(config.env.is_empty());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libp2p::PeerId;
    use std::time::Duration;

    #[test]
    fn selects_unique_winners_for_multiple_replicas() {
        let context = BidContext {
            task_id: "task-a".to_string(),
            manifest_id: "task-a".to_string(),
            placement_metadata: HashMap::new(),
            replicas: 2,
            bids: vec![
                BidEntry {
                    bidder_id: "node1".to_string(),
                    score: 0.9,
                },
                BidEntry {
                    bidder_id: "node2".to_string(),
                    score: 0.8,
                },
                BidEntry {
                    bidder_id: "node2".to_string(),
                    score: 0.95,
                },
            ],
        };

        let winners = select_winners(&context, "node1");
        assert_eq!(winners.len(), 2);
        assert!(winners.iter().any(|w| w.bidder_id == "node1"));
        assert!(winners.iter().any(|w| w.bidder_id == "node2"));
    }

    #[test]
    fn local_node_only_selected_once_even_with_multiple_replicas() {
        let context = BidContext {
            task_id: "task-b".to_string(),
            manifest_id: "task-b".to_string(),
            placement_metadata: HashMap::new(),
            replicas: 3,
            bids: vec![
                BidEntry {
                    bidder_id: "local".to_string(),
                    score: 0.99,
                },
                BidEntry {
                    bidder_id: "local".to_string(),
                    score: 0.98,
                },
                BidEntry {
                    bidder_id: "remote".to_string(),
                    score: 0.5,
                },
            ],
        };

        let winners = select_winners(&context, "local");
        assert_eq!(winners.len(), 2);
        assert_eq!(winners.iter().filter(|w| w.bidder_id == "local").count(), 1);
    }
}
