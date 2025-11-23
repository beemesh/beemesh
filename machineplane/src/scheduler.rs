//! Scheduler module for Decentralized Bidding
//!
//! This module implements the "Pull" model where nodes listen for Tenders,
//! evaluate their fit, submit Bids, and if they win, acquire a Lease.

use crate::messages::constants::{
    DEFAULT_SELECTION_WINDOW_MS, SCHEDULER_EVENTS, SCHEDULER_PROPOSALS, SCHEDULER_TENDERS,
};
use crate::messages::types::Bid;
use crate::messages::{machine, signatures, types::LeaseHint};
use crate::network::utils::peer_id_to_public_key;
use libp2p::gossipsub;
use libp2p::identity::{Keypair, PublicKey};
use log::{error, info};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio::time::sleep;

/// Scheduler manages the bidding lifecycle
pub struct Scheduler {
    /// Local node ID (PeerId string)
    local_node_id: String,
    /// Node keypair for signing LeaseHints
    keypair: Keypair,
    /// Active bids we are tracking: TenderID -> BidContext
    active_bids: Arc<Mutex<HashMap<String, BidContext>>>,
    /// Channel to send messages back to the network loop
    outbound_tx: mpsc::UnboundedSender<SchedulerCommand>,
}

struct BidContext {
    tender_id: String,
    manifest_id: String,
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
    Publish {
        topic: String,
        payload: Vec<u8>,
    },
    PutDHT {
        key: Vec<u8>,
        value: Vec<u8>,
    },
    DeployWorkload {
        manifest_id: String,
        manifest_json: String,
        replicas: u32,
    },
}

impl Scheduler {
    pub fn new(
        local_node_id: String,
        keypair: Keypair,
        outbound_tx: mpsc::UnboundedSender<SchedulerCommand>,
    ) -> Self {
        Self {
            local_node_id,
            keypair,
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
            self.handle_tender(message).await;
        } else if *topic_hash == proposals {
            self.handle_bid(message).await;
        } else if *topic_hash == events {
            self.handle_event(message).await;
        }
    }

    /// Process a Tender message: Evaluate -> Bid
    async fn handle_tender(&self, message: &gossipsub::Message) {
        match machine::decode_tender(&message.data) {
            Ok(tender) => {
                let tender_id = tender.id.clone();
                info!("Received Tender: {}", tender_id);

                let manifest_id = tender.id.clone();

                // 1. Evaluate Fit
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
                        tender_id.to_string(),
                        BidContext {
                            tender_id: tender_id.to_string(),
                            manifest_id: manifest_id.clone(),
                            bids: vec![BidEntry {
                                bidder_id: self.local_node_id.clone(),
                                score: my_score,
                            }],
                        },
                    );
                }

                // 4. Publish Bid
                let mut bid = Bid {
                    tender_id: tender_id.clone(),
                    node_id: self.local_node_id.clone(),
                    score: my_score,
                    resource_fit_score: capacity_score,
                    network_locality_score: 0.5,
                    timestamp: now,
                    signature: Vec::new(),
                };

                let bid_bytes = match signatures::sign_bid(&mut bid, &self.keypair) {
                    Ok(_) => machine::build_bid(
                        &bid.tender_id,
                        &bid.node_id,
                        bid.score,
                        bid.resource_fit_score,
                        bid.network_locality_score,
                        bid.timestamp,
                        &bid.signature,
                    ),
                    Err(e) => {
                        error!("Failed to sign bid for tender {}: {}", tender_id, e);
                        return;
                    }
                };

                if let Err(e) = self.outbound_tx.send(SchedulerCommand::Publish {
                    topic: SCHEDULER_PROPOSALS.to_string(),
                    payload: bid_bytes,
                }) {
                    error!("Failed to queue bid for tender {}: {}", tender_id, e);
                } else {
                    info!(
                        "Queued Bid for tender {} with score {:.2}",
                        tender_id, my_score
                    );
                }

                // 5. Spawn Selection Window Waiter
                let active_bids = self.active_bids.clone();
                let tender_id_clone = tender_id.to_string();
                let manifest_json = tender.manifest_json.clone();
                let local_id = self.local_node_id.clone();
                let keypair = self.keypair.clone();
                let outbound_tx = self.outbound_tx.clone();

                // We need a way to trigger deployment, for now just log
                tokio::spawn(async move {
                    sleep(Duration::from_millis(DEFAULT_SELECTION_WINDOW_MS)).await;

                    let (winners, manifest_id_opt) = {
                        let mut bids = active_bids.lock().unwrap();
                        if let Some(ctx) = bids.remove(&tender_id_clone) {
                            let winners = select_winners(&ctx);
                            if !winners.is_empty() {
                                let summary = winners
                                    .iter()
                                    .map(|w| format!("{} ({:.2})", w.bidder_id, w.score))
                                    .collect::<Vec<_>>()
                                    .join(", ");
                                info!(
                                    "Selected winners for tender {} (manifest {}): {}",
                                    ctx.tender_id, ctx.manifest_id, summary
                                );
                            } else {
                                info!(
                                    "No qualifying bids for tender {} (manifest {})",
                                    ctx.tender_id, ctx.manifest_id
                                );
                            }
                            (winners, Some(ctx.manifest_id.clone()))
                        } else {
                            (Vec::new(), None)
                        }
                    };

                    if winners.iter().any(|bid| bid.bidder_id == local_id) {
                        info!(
                            "WON BID for tender {}! Proceeding to deployment...",
                            tender_id_clone
                        );
                        info!("Deploying workload for tender {}", tender_id_clone);

                        // 1. Publish LeaseHint to DHT
                        let lease_hint = LeaseHint {
                            tender_id: tender_id_clone.clone(),
                            node_id: local_id.clone(),
                            score: 1.0,
                            ttl_ms: 30000,
                            renew_nonce: 0,
                            timestamp: SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap()
                                .as_millis() as u64,
                            signature: Vec::new(),
                        };

                        let lease_hint_bytes = match {
                            let mut lease_hint = lease_hint.clone();
                            signatures::sign_lease_hint(&mut lease_hint, &keypair)
                                .map(|_| lease_hint)
                        } {
                            Ok(signed) => Some(machine::build_lease_hint(
                                &signed.tender_id,
                                &signed.node_id,
                                signed.score,
                                signed.ttl_ms,
                                signed.renew_nonce,
                                signed.timestamp,
                                &signed.signature,
                            )),
                            Err(e) => {
                                error!(
                                    "Failed to sign LeaseHint for tender {}: {}",
                                    tender_id_clone, e
                                );
                                None
                            }
                        };

                        if let Some(lease_hint_bytes) = lease_hint_bytes {
                            if let Err(e) = outbound_tx.send(SchedulerCommand::PutDHT {
                                key: format!("lease:{}", tender_id_clone).into_bytes(),
                                value: lease_hint_bytes,
                            }) {
                                error!(
                                    "Failed to publish LeaseHint for tender {}: {}",
                                    tender_id_clone, e
                                );
                            } else {
                                info!("Published LeaseHint for tender {}", tender_id_clone);
                            }
                        }

                        // 2. Trigger Workload Deployment
                        let manifest_id =
                            manifest_id_opt.unwrap_or_else(|| tender_id_clone.clone());

                        if let Err(e) = outbound_tx.send(SchedulerCommand::DeployWorkload {
                            manifest_id,
                            manifest_json: manifest_json.clone(),
                            replicas: 1,
                        }) {
                            error!(
                                "Failed to trigger deployment for tender {}: {}",
                                tender_id_clone, e
                            );
                        } else {
                            info!("Triggered deployment for tender {}", tender_id_clone);
                        }
                    } else {
                        info!("Lost bid for tender {}.", tender_id_clone);
                    }
                });
            }
            Err(e) => error!("Failed to parse Tender message: {}", e),
        }
    }

    /// Process a Bid message: Update highest bid
    async fn handle_bid(&self, message: &gossipsub::Message) {
        match machine::decode_bid(&message.data) {
            Ok(bid) => {
                let public_key = match message
                    .source
                    .as_ref()
                    .and_then(|peer_id| peer_id_to_public_key(peer_id))
                {
                    Some(key) => key,
                    None => {
                        error!(
                            "Discarding bid for tender {}: unable to derive public key from source",
                            bid.tender_id
                        );
                        return;
                    }
                };

                if !signatures::verify_sign_bid(&bid, &public_key) {
                    error!(
                        "Discarding bid for tender {} from {}: invalid signature",
                        bid.tender_id, bid.node_id
                    );
                    return;
                }

                let tender_id = bid.tender_id.clone();
                let bidder_id = bid.node_id.clone();
                let score = bid.score;

                // Ignore our own bids (handled locally)
                if bidder_id == self.local_node_id {
                    return;
                }

                let mut bids = self.active_bids.lock().unwrap();
                if let Some(ctx) = bids.get_mut(&tender_id) {
                    info!(
                        "Recorded bid for tender {}: {:.2} from {}",
                        tender_id, score, bidder_id
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
        match machine::decode_scheduler_event(&message.data) {
            Ok(event) => {
                let tender_id = event.tender_id.clone();
                info!(
                    "Received Scheduler Event for tender {}: {:?}",
                    tender_id, event.event_type
                );

                if event.node_id == self.local_node_id {
                    use crate::messages::types::EventType;

                    if matches!(
                        event.event_type,
                        EventType::Cancelled | EventType::Preempted | EventType::Failed
                    ) {
                        info!(
                            "Received termination event for locally deployed tender {}",
                            tender_id
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
}

fn select_winners(context: &BidContext) -> Vec<BidOutcome> {
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

    let mut outcomes = Vec::new();

    if let Some(top_bid) = bids.into_iter().next() {
        outcomes.push(BidOutcome {
            bidder_id: top_bid.bidder_id,
            score: top_bid.score,
        });
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

    use crate::messages::machine;
    use crate::messages::types::ApplyRequest;
    use crate::network::behaviour::MyBehaviour;
    use crate::runtimes::{DeploymentConfig, RuntimeRegistry, create_default_registry};
    use base64::Engine;
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

    /// Node-local cache mapping manifest IDs to owner public keys.
    static MANIFEST_OWNER_MAP: Lazy<RwLock<HashMap<String, Vec<u8>>>> =
        Lazy::new(|| RwLock::new(HashMap::new()));

    /// Initialize the runtime registry and provider manager
    pub async fn initialize_podman_manager() -> Result<(), Box<dyn std::error::Error>> {
        info!("Initializing runtime registry for manifest deployment");

        let registry = create_default_registry().await;
        let available_engines = registry.check_available_engines().await;

        info!("Available runtime engines: {:?}", available_engines);

        // Store the registry globally
        {
            let mut global_registry = RUNTIME_REGISTRY.write().await;
            *global_registry = Some(registry);
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
                match machine::decode_apply_request(&request) {
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
    pub async fn process_manifest_deployment(
        swarm: &mut Swarm<MyBehaviour>,
        apply_req: &ApplyRequest,
        manifest_json: &str,
        owner_pubkey: &[u8],
    ) -> Result<String, Box<dyn std::error::Error>> {
        info!("Processing manifest deployment");

        // Parse the manifest content; decode from base64 if needed
        let manifest_content = decode_manifest_content(manifest_json);

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
            debug!(
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
        let workload_info = engine
            .deploy_workload(&manifest_id, &modified_manifest_content, &deployment_config)
            .await?;

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

    /// Decode manifest content that may be base64-encoded.
    /// Falls back to treating the string as plain text if decoding fails or produces non-UTF8.
    pub(super) fn decode_manifest_content(manifest_json: &str) -> Vec<u8> {
        match base64::engine::general_purpose::STANDARD.decode(manifest_json) {
            Ok(decoded) => {
                if String::from_utf8(decoded.clone()).is_ok() {
                    debug!("Decoded base64 manifest content ({} bytes)", decoded.len());
                    decoded
                } else {
                    debug!(
                        "Base64 manifest content was not valid UTF-8; using raw manifest string"
                    );
                    manifest_json.as_bytes().to_vec()
                }
            }
            Err(_) => manifest_json.as_bytes().to_vec(),
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

            if *available.get("podman").unwrap_or(&false) {
                return Ok("podman".to_string());
            }

            if let Some(default_engine) = registry.get_default_engine() {
                warn!(
                    "Preferred Podman runtime unavailable; falling back to default engine '{}'",
                    default_engine.name()
                );
                return Ok(default_engine.name().to_string());
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

        match machine::decode_apply_request(manifest) {
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
            let result = initialize_podman_manager().await;
            assert!(result.is_ok());

            let stats = get_runtime_registry_stats().await;
            assert!(!stats.is_empty());

            let engines = list_available_engines().await;
            assert!(!engines.is_empty());
            assert!(engines.contains(&"podman".to_string()));
        }

        #[tokio::test]
        async fn test_runtime_engine_selection() {
            // Initialize registry for testing
            let _ = initialize_podman_manager().await;

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
            let engine_name = engine.unwrap();
            assert_eq!(engine_name, "podman");

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
    use base64::Engine;

    #[test]
    fn decode_manifest_content_handles_base64_and_plain() {
        let manifest = "apiVersion: v1\nkind: Pod";
        let encoded = base64::engine::general_purpose::STANDARD.encode(manifest);

        let decoded = decode_manifest_content(&encoded);
        assert_eq!(decoded, manifest.as_bytes());

        let plaintext = decode_manifest_content(manifest);
        assert_eq!(plaintext, manifest.as_bytes());
    }

    #[test]
    fn selects_top_unique_winner() {
        let context = BidContext {
            tender_id: "tender-a".to_string(),
            manifest_id: "tender-a".to_string(),
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

        let winners = select_winners(&context);
        assert_eq!(winners.len(), 1);
        assert_eq!(winners[0].bidder_id, "node2");
    }

    #[test]
    fn selects_single_local_winner_when_highest() {
        let context = BidContext {
            tender_id: "tender-b".to_string(),
            manifest_id: "tender-b".to_string(),
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

        let winners = select_winners(&context);
        assert_eq!(winners.len(), 1);
        assert_eq!(winners[0].bidder_id, "local");
    }
}
