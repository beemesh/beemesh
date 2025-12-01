use crate::signatures;
use crate::messages::CandidateNode;
use crate::network::BEEMESH_FABRIC;
use crate::network::control::Libp2pControl;
use crate::scheduler::{register_local_manifest, DEFAULT_SELECTION_WINDOW_MS};
use axum::{
    Router,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, post},
};
use base64::Engine;
use libp2p::identity::Keypair;
use log::{debug, error, info};
use rand::RngCore;
use serde_json::Value;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tokio::sync::mpsc;
use tokio::{
    sync::watch,
    time::{Duration, timeout},
};
use uuid::Uuid;

async fn create_response_with_fallback(
    body: &[u8],
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    axum::response::Response::builder()
        .status(axum::http::StatusCode::OK)
        .header("Content-Type", "application/octet-stream")
        .body(axum::body::Body::from(body.to_vec()))
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)
}

// Stateless K8s API - all state derived from Podman runtime
pub mod kube;

async fn get_nodes(
    State(state): State<RestState>,
    _headers: HeaderMap,
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    let peers = state.peer_rx.borrow().clone();
    let response = crate::messages::NodesResponse { peers };
    let response_data = bincode::serialize(&response).expect("serialize NodesResponse");

    // Return plain response
    Ok(axum::response::Response::builder()
        .status(200)
        .header("Content-Type", "application/octet-stream")
        .body(axum::body::Body::from(response_data))
        .unwrap())
}

async fn get_public_key(State(_state): State<RestState>) -> String {
    // Get the machine's libp2p public key
    if let Some((pk, _)) = crate::network::get_node_keypair() {
        return base64::engine::general_purpose::STANDARD.encode(pk);
    }
    "ERROR: No keypair available".to_string()
}

/// REST API state - stateless by design
/// 
/// The machineplane is ephemeral and does not persist any K8s resource state.
/// All K8s API responses are derived from Podman runtime state.
/// The tender_tracker is for transient operation tracking only (not fabric-wide state).
#[derive(Clone)]
pub struct RestState {
    pub peer_rx: watch::Receiver<Vec<String>>,
    pub control_tx: mpsc::UnboundedSender<crate::network::control::Libp2pControl>,
    /// Transient local tender tracking for ongoing operations (not persisted)
    pub tender_tracker: Arc<RwLock<HashMap<String, TenderRecord>>>,
    /// Local peer ID for this node (used for keypair lookup in multi-node scenarios)
    pub local_peer_id_bytes: Vec<u8>,
}

pub fn build_router(
    peer_rx: watch::Receiver<Vec<String>>,
    control_tx: mpsc::UnboundedSender<crate::network::control::Libp2pControl>,
    local_peer_id_bytes: Vec<u8>,
) -> Router {
    let state = RestState {
        peer_rx,
        control_tx,
        tender_tracker: Arc::new(RwLock::new(HashMap::new())),
        local_peer_id_bytes,
    };
    let kube_routes = Router::new()
        .route("/version", get(kube::version))
        .route("/apis", get(kube::api_group_list))
        .nest("/api", kube::core_router())
        .nest("/apis/apps/v1", kube::apps_v1_router());

    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/api/v1/pubkey", get(get_public_key))
        .route("/api/v1/kem_pubkey", get(get_public_key))
        .route("/api/v1/signing_pubkey", get(get_public_key))
        .route("/debug/dht/active_announces", get(debug_active_announces))
        .route("/debug/dht/peers", get(debug_dht_peers))
        .route("/debug/peers", get(debug_peers))
        .route("/debug/pods", get(debug_pods))
        .route("/debug/tenders", get(debug_all_tenders))
        .route("/debug/local_peer_id", get(debug_local_peer_id))
        .route("/tenders", post(create_tender))
        .route("/tenders/{tender_id}", get(get_tender_status))
        .route("/disposal/{namespace}/{kind}/{name}", delete(delete_tender))
        .route("/tenders/{tender_id}/candidates", post(get_candidates))
        .route("/disposal/{namespace}/{kind}/{name}", get(get_disposal_status))
        .route("/nodes", get(get_nodes))
        .merge(kube_routes)
        .with_state(state)
}

pub async fn get_candidates(
    Path(tender_id): Path<String>,
    State(state): State<RestState>,
    _headers: HeaderMap,
    _body: Bytes,
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    log::info!(
        "get_candidates: called for tender_id={} (direct delivery mode)",
        tender_id
    );

    let candidates = collect_candidate_pubkeys(&state, &tender_id, 5).await?;
    let response = crate::messages::CandidatesResponse {
        ok: true,
        candidates,
    };
    let response_data = bincode::serialize(&response).expect("serialize CandidatesResponse");

    // Return plain response
    Ok(axum::response::Response::builder()
        .status(200)
        .header("Content-Type", "application/octet-stream")
        .body(axum::body::Body::from(response_data))
        .unwrap())
}

pub(crate) async fn collect_candidate_pubkeys(
    state: &RestState,
    tender_id: &str,
    max_candidates: usize,
) -> Result<Vec<CandidateNode>, axum::http::StatusCode> {
    const PLACEHOLDER_KEM_PUBLIC_KEY: &str = "mock-kem-public-key";

    log::info!(
        "collect_candidate_pubkeys: using peer inventory for tender {} (limit {})",
        tender_id,
        max_candidates
    );

    let peers = state.peer_rx.borrow().clone();
    let mut candidates: Vec<CandidateNode> = Vec::new();

    for peer_id in peers {
        if candidates.len() >= max_candidates {
            break;
        }

        candidates.push(CandidateNode {
            peer_id: peer_id.clone(),
            public_key: PLACEHOLDER_KEM_PUBLIC_KEY.to_string(),
        });
    }

    if candidates.is_empty() {
        log::warn!(
            "collect_candidate_pubkeys: no peers available on topic {}; returning empty candidate list",
            BEEMESH_FABRIC
        );
    }

    Ok(candidates)
}

#[derive(Debug, Clone)]
pub struct KubeResourceRecord {
    pub api_version: String,
    pub kind: String,
    pub namespace: String,
    pub name: String,
    pub uid: String,
    pub resource_version: u64,
    pub manifest: Value,
    pub creation_timestamp: std::time::SystemTime,
}

#[derive(Debug, Clone)]
pub struct TenderRecord {
    pub manifest_bytes: Vec<u8>,
    pub created_at: std::time::SystemTime,
    // map of peer_id -> manifest payload for manifest distribution
    pub manifests_distributed: HashMap<String, String>,
    pub assigned_peers: Option<Vec<String>>,
    pub owner_pubkey: Vec<u8>,
    pub kube: Option<KubeResourceRecord>,
}

pub async fn create_tender(
    State(state): State<RestState>,
    _headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    debug!("create_tender: parsing payload");

    // Payload is received directly over the mutually authenticated transport.
    let payload_bytes_for_parsing = body.to_vec();

    log::info!(
        "create_tender: received payload len={}, first_20_bytes={:02x?}",
        payload_bytes_for_parsing.len(),
        &payload_bytes_for_parsing[..std::cmp::min(20, payload_bytes_for_parsing.len())]
    );

    // Generate a unique tender_id using UUID v4
    let tender_id = Uuid::new_v4().to_string();
    log::info!("create_tender: generated tender_id='{}'", tender_id);

    let owner_pubkey = Vec::new();

    // Store manifest bytes as-is
    let manifest_bytes_to_store = payload_bytes_for_parsing;
    let manifest_str = String::from_utf8_lossy(&manifest_bytes_to_store).to_string();
    
    // Register manifest locally for cache
    register_local_manifest(&tender_id, &manifest_str);

    let rec = TenderRecord {
        manifest_bytes: manifest_bytes_to_store,
        created_at: std::time::SystemTime::now(),
        manifests_distributed: HashMap::new(),
        assigned_peers: None,
        owner_pubkey: owner_pubkey.clone(),
        kube: None,
    };
    {
        let mut tracker = state.tender_tracker.write().await;
        log::info!(
            "create_tender: tracking tender with tender_id='{}'",
            tender_id
        );
        log::info!(
            "create_tender: tender_tracker had {} tenders before insert",
            tracker.len()
        );
        tracker.insert(tender_id.clone(), rec);
        log::info!(
            "create_tender: tender_tracker now has {} tenders after insert",
            tracker.len()
        );
        log::info!(
            "create_tender: verifying tender_id '{}' exists in tracker: {}",
            tender_id,
            tracker.contains_key(&tender_id)
        );
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .as_millis() as u64;

    // Use the state's local_peer_id to get the correct keypair for this node
    let keypair = crate::network::get_node_keypair_for_peer(Some(&state.local_peer_id_bytes))
        .and_then(|(_, sk)| Keypair::from_protobuf_encoding(&sk).ok())
        .ok_or_else(|| {
            error!(
                "create_tender: failed to publish tender {} due to missing node keypair",
                tender_id
            );
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let mut tender = crate::messages::Tender {
        id: tender_id.clone(),
        manifest_digest: tender_id.clone(),
        qos_preemptible: false,
        timestamp,
        nonce: rand::thread_rng().next_u64(),
        signature: Vec::new(),
    };

    signatures::sign_tender(&mut tender, &keypair).map_err(|e| {
        error!("create_tender: failed to sign tender {}: {}", tender_id, e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let tender_bytes = bincode::serialize(&tender).expect("tender serialization");

    let (reply_tx, mut reply_rx) = mpsc::unbounded_channel::<Result<(), String>>();
    state
        .control_tx
        .send(Libp2pControl::PublishTender {
            payload: tender_bytes,
            reply_tx,
        })
        .map_err(|e| {
            error!(
                "create_tender: failed to dispatch tender publication for tender_id={}: {}",
                tender_id, e
            );
            StatusCode::BAD_GATEWAY
        })?;

    match timeout(Duration::from_secs(10), reply_rx.recv()).await {
        Ok(Some(Ok(_))) => {}
        Ok(Some(Err(e))) => {
            log::warn!(
                "create_tender: tender publication failed for tender_id={}: {}",
                tender_id,
                e
            );
            return Err(StatusCode::BAD_GATEWAY);
        }
        Ok(None) => {
            log::warn!(
                "create_tender: tender publication channel closed for tender_id={}",
                tender_id
            );
            return Err(StatusCode::BAD_GATEWAY);
        }
        Err(_) => {
            log::warn!(
                "create_tender: timed out publishing tender_id={}",
                tender_id
            );
            return Err(StatusCode::GATEWAY_TIMEOUT);
        }
    }

    let response = crate::messages::TenderCreateResponse {
        ok: true,
        tender_id: tender_id.clone(),
        manifest_ref: tender_id.clone(),
        selection_window_ms: DEFAULT_SELECTION_WINDOW_MS,
        message: String::new(),
    };
    let response_data = bincode::serialize(&response).expect("serialize TenderCreateResponse");

    // Return plain response
    Ok(axum::response::Response::builder()
        .status(200)
        .header("Content-Type", "application/octet-stream")
        .body(axum::body::Body::from(response_data))
        .unwrap())
}

// Debug: return the active announces (provider CIDs) tracked by the control module
async fn debug_active_announces(State(_state): State<RestState>) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "ok": true,
        "cids": [],
        "message": "provider announcements are low-touch and expire naturally"
    }))
}

/// Debug endpoint to get local peer ID
async fn debug_local_peer_id(State(state): State<RestState>) -> axum::Json<serde_json::Value> {
    // Get the local peer ID from the control channel
    use tokio::sync::mpsc;
    let (reply_tx, mut reply_rx) = mpsc::unbounded_channel();

    let control_msg = crate::network::control::Libp2pControl::GetLocalPeerId { reply_tx };

    if state.control_tx.send(control_msg).is_err() {
        return axum::Json(serde_json::json!({
            "ok": false,
            "error": "Failed to send control message"
        }));
    }

    // Wait for response with timeout
    match tokio::time::timeout(std::time::Duration::from_secs(2), reply_rx.recv()).await {
        Ok(Some(peer_id)) => axum::Json(serde_json::json!({
            "ok": true,
            "local_peer_id": peer_id.to_string()
        })),
        Ok(None) => axum::Json(serde_json::json!({
            "ok": false,
            "error": "Control channel closed"
        })),
        Err(_) => axum::Json(serde_json::json!({
            "ok": false,
            "error": "Timeout waiting for local peer ID"
        })),
    }
}

// Debug: get DHT peer information
async fn debug_dht_peers(State(state): State<RestState>) -> axum::Json<serde_json::Value> {
    use tokio::sync::mpsc;

    let (reply_tx, mut reply_rx) = mpsc::unbounded_channel();

    // Send control message to get DHT peer info
    let control_msg = crate::network::control::Libp2pControl::GetDhtPeers { reply_tx };

    if let Err(e) = state.control_tx.send(control_msg) {
        return axum::Json(serde_json::json!({
            "ok": false,
            "error": format!("Failed to send DHT peers request: {}", e)
        }));
    }

    // Wait for response with timeout
    match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx.recv()).await {
        Ok(Some(Ok(peer_info))) => axum::Json(serde_json::json!({
            "ok": true,
            "dht_peers": peer_info
        })),
        Ok(Some(Err(e))) => axum::Json(serde_json::json!({
            "ok": false,
            "error": format!("DHT peers query failed: {}", e)
        })),
        Ok(None) => axum::Json(serde_json::json!({
            "ok": false,
            "error": "DHT peers channel closed"
        })),
        Err(_) => axum::Json(serde_json::json!({
            "ok": false,
            "error": "DHT peers request timed out"
        })),
    }
}

async fn debug_peers(State(state): State<RestState>) -> axum::Json<serde_json::Value> {
    let peers: Vec<String> = state.peer_rx.borrow().clone();

    axum::Json(serde_json::json!({
        "ok": true,
        "peers": peers,
        "count": peers.len()
    }))
}

/// Debug endpoint to list local pods from Podman
///
/// Queries the local Podman runtime directly (stateless - no fabric-wide state).
/// Returns all pods with their status and metadata.
/// Used by tests to verify pod deployment without cross-node state tracking.
async fn debug_pods(State(state): State<RestState>) -> axum::Json<serde_json::Value> {
    // Get the local peer ID first
    let local_peer_id = {
        use tokio::sync::mpsc;
        let (reply_tx, mut reply_rx) = mpsc::unbounded_channel();
        let control_msg = crate::network::control::Libp2pControl::GetLocalPeerId { reply_tx };
        
        if state.control_tx.send(control_msg).is_ok() {
            match tokio::time::timeout(std::time::Duration::from_secs(1), reply_rx.recv()).await {
                Ok(Some(peer_id)) => peer_id.to_string(),
                _ => "unknown".to_string(),
            }
        } else {
            "unknown".to_string()
        }
    };

    // Query Podman for local pods
    let socket = match crate::runtimes::podman::PodmanEngine::get_socket() {
        Some(s) => s,
        None => {
            return axum::Json(serde_json::json!({
                "ok": false,
                "error": "Podman socket not configured",
                "local_peer_id": local_peer_id,
                "instances": {},
                "instance_count": 0
            }));
        }
    };

    let client = crate::runtimes::podman_api::PodmanApiClient::new(&socket);
    
    match client.list_pods().await {
        Ok(pods) => {
            let mut instances = serde_json::Map::new();
            
            for pod in &pods {
                let pod_id = pod.id.clone().unwrap_or_default();
                let pod_name = pod.name.clone().unwrap_or_default();
                let pod_status = pod.status.clone().unwrap_or_default();
                let pod_created = pod.created.clone();
                let labels = pod.labels.clone().unwrap_or_default();
                
                // Skip pods without ID
                if pod_id.is_empty() {
                    continue;
                }
                
                // Extract workload identity from labels
                let workload_namespace = labels.get("beemesh.namespace").cloned().unwrap_or_default();
                let workload_kind = labels.get("beemesh.kind").cloned().unwrap_or_default();
                let workload_name = labels.get("beemesh.name").cloned().unwrap_or_default();
                
                // Extract app name for filtering
                let app_name = labels.get("app.kubernetes.io/name")
                    .or_else(|| labels.get("app"))
                    .cloned()
                    .unwrap_or_default();
                
                // Build metadata map
                let mut metadata = serde_json::Map::new();
                metadata.insert("name".to_string(), serde_json::Value::String(app_name.clone()));
                for (k, v) in &labels {
                    metadata.insert(k.clone(), serde_json::Value::String(v.clone()));
                }
                
                // Export manifest if possible (for verification)
                let exported_manifest = if !pod_name.is_empty() {
                    match client.generate_kube(&pod_name).await {
                        Ok(yaml) => Some(yaml),
                        Err(_) => None,
                    }
                } else {
                    None
                };
                
                instances.insert(pod_id.clone(), serde_json::json!({
                    "id": pod_id,
                    "name": pod_name,
                    "status": pod_status,
                    "workload": {
                        "namespace": workload_namespace,
                        "kind": workload_kind,
                        "name": workload_name
                    },
                    "metadata": metadata,
                    "created": pod_created,
                    "exported_manifest": exported_manifest
                }));
            }
            
            axum::Json(serde_json::json!({
                "ok": true,
                "local_peer_id": local_peer_id,
                "instances": instances,
                "instance_count": instances.len()
            }))
        }
        Err(e) => {
            axum::Json(serde_json::json!({
                "ok": false,
                "error": format!("Failed to list pods: {}", e),
                "local_peer_id": local_peer_id,
                "instances": {},
                "instance_count": 0
            }))
        }
    }
}

// Debug: list all tracked tenders
async fn debug_all_tenders(State(state): State<RestState>) -> axum::Json<serde_json::Value> {
    let tracker = state.tender_tracker.read().await;
    let mut tenders = serde_json::Map::new();

    for (tender_id, record) in tracker.iter() {
        tenders.insert(tender_id.clone(), serde_json::json!({
            "created_at": record.created_at.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs(),
            "assigned_peers": record.assigned_peers,
            "manifest_bytes_len": record.manifest_bytes.len()
        }));
    }

    axum::Json(serde_json::json!({
        "ok": true,
        "tenders": serde_json::Value::Object(tenders)
    }))
}

pub async fn get_tender_status(
    Path(tender_id): Path<String>,
    State(state): State<RestState>,
    _headers: HeaderMap,
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    let maybe = { state.tender_tracker.read().await.get(&tender_id).cloned() };
    if let Some(r) = maybe {
        let assigned = r.assigned_peers.unwrap_or_default();

        let response = crate::messages::TenderStatusResponse {
            tender_id: tender_id.clone(),
            state: "Pending".to_string(),
            assigned_peers: assigned,
        };
        let response_data = bincode::serialize(&response).expect("serialize TenderStatusResponse");
        return create_response_with_fallback(&response_data).await;
    }
    let error_response = bincode::serialize(&crate::messages::TenderStatusResponse {
        tender_id: String::new(),
        state: "Error".to_string(),
        assigned_peers: vec![],
    }).expect("serialize TenderStatusResponse");
    create_response_with_fallback(&error_response).await
}

/// Delete a workload by resource coordinates - publishes DISPOSAL to gossipsub (fire-and-forget)
///
/// This endpoint broadcasts a signed DISPOSAL message to all nodes in the fabric.
/// Nodes that have pods matching the resource coordinates (namespace/kind/name) will:
/// 1. Mark the resource for disposal (prevents self-healer recovery)
/// 2. Delete local pods
///
/// The response returns immediately after publishing (fire-and-forget).
pub async fn delete_tender(
    Path((namespace, kind, name)): Path<(String, String, String)>,
    State(state): State<RestState>,
    _headers: HeaderMap,
    _body: Bytes,
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    let resource_coord = format!("{}/{}/{}", namespace, kind, name);
    info!("delete_tender: resource={}", resource_coord);

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .as_millis() as u64;

    // Get keypair for signing
    let keypair = crate::network::get_node_keypair_for_peer(Some(&state.local_peer_id_bytes))
        .and_then(|(_, sk)| Keypair::from_protobuf_encoding(&sk).ok())
        .ok_or_else(|| {
            error!("delete_tender: missing node keypair");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Create and sign Disposal message with resource coordinates
    let mut disposal = crate::messages::Disposal {
        namespace: namespace.clone(),
        kind: kind.clone(),
        name: name.clone(),
        timestamp,
        nonce: rand::thread_rng().next_u64(),
        signature: Vec::new(),
    };

    crate::signatures::sign_disposal(&mut disposal, &keypair).map_err(|e| {
        error!("delete_tender: failed to sign disposal: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let disposal_bytes = bincode::serialize(&disposal).expect("disposal serialization");

    // Publish to gossipsub (fire-and-forget)
    let (reply_tx, mut reply_rx) = mpsc::unbounded_channel();
    state
        .control_tx
        .send(Libp2pControl::PublishDisposal {
            payload: disposal_bytes,
            reply_tx,
        })
        .map_err(|e| {
            error!("delete_tender: failed to send disposal: {}", e);
            StatusCode::BAD_GATEWAY
        })?;

    // Wait briefly for publish confirmation
    match timeout(Duration::from_secs(5), reply_rx.recv()).await {
        Ok(Some(Ok(_))) => {
            info!("delete_tender: disposal published for resource={}", resource_coord);
        }
        Ok(Some(Err(e))) => {
            log::warn!("delete_tender: disposal publish failed: {}", e);
            return Err(StatusCode::BAD_GATEWAY);
        }
        Ok(None) | Err(_) => {
            log::warn!("delete_tender: disposal publish timeout");
            return Err(StatusCode::GATEWAY_TIMEOUT);
        }
    }

    let response = crate::messages::DeleteResponse {
        ok: true,
        operation_id: format!("disposal-{}", timestamp),
        message: "Disposal published to fabric".to_string(),
        resource: resource_coord,
        removed_pods: vec![], // Fire-and-forget - no immediate confirmation
    };
    let response_data = bincode::serialize(&response).expect("serialize DeleteResponse");

    Ok(axum::response::Response::builder()
        .status(200)
        .header("Content-Type", "application/octet-stream")
        .body(axum::body::Body::from(response_data))
        .unwrap())
}

/// Check if a resource is marked for disposal
///
/// Used by the Workplane self-healer before initiating pod recovery.
/// Returns { resource, disposing: bool }
async fn get_disposal_status(
    Path((namespace, kind, name)): Path<(String, String, String)>,
) -> axum::Json<serde_json::Value> {
    let resource_key = format!("{}/{}/{}", namespace, kind, name);
    let disposing = crate::scheduler::is_disposing(&resource_key).await;
    axum::Json(serde_json::json!({
        "resource": resource_key,
        "disposing": disposing
    }))
}
