use crate::messages::constants::{BEEMESH_FABRIC, DEFAULT_SELECTION_WINDOW_MS};
use crate::messages::types::CandidateNode;
use crate::scheduler::register_local_manifest;
#[cfg(debug_assertions)]
use axum::{
    Router,
    body::Bytes,
    extract::{Path, Query, State},
    http::HeaderMap,
    routing::{delete, get, post},
};
use base64::Engine;
use log::{debug, error, info};
use once_cell::sync::Lazy;
use serde_json::Value;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tokio::sync::mpsc;
use tokio::{sync::watch, time::Duration};
use ulid::Ulid;

async fn create_response_with_fallback(
    body: &[u8],
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    axum::response::Response::builder()
        .status(axum::http::StatusCode::OK)
        .header("Content-Type", "application/octet-stream")
        .body(axum::body::Body::from(body.to_vec()))
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)
}

pub mod kube;

async fn get_nodes(
    State(state): State<RestState>,
    _headers: HeaderMap,
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    let peers = state.peer_rx.borrow().clone();
    let response_data = crate::messages::machine::build_nodes_response(&peers);

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

#[derive(Clone)]
pub struct RestState {
    pub peer_rx: watch::Receiver<Vec<String>>,
    pub control_tx: mpsc::UnboundedSender<crate::network::control::Libp2pControl>,
    tender_store: Arc<RwLock<HashMap<String, TenderRecord>>>,
}

// Global in-memory store of decrypted manifests for debugging / tests.
// Keyed by manifest_id -> decrypted manifest JSON/value.
static DECRYPTED_MANIFESTS: Lazy<tokio::sync::RwLock<HashMap<String, serde_json::Value>>> =
    Lazy::new(|| tokio::sync::RwLock::new(HashMap::new()));

// Global mapping of operation_id -> manifest_cid to ensure consistent manifest ID usage across REST API and apply processing
static OPERATION_MANIFEST_MAPPING: Lazy<tokio::sync::RwLock<HashMap<String, String>>> =
    Lazy::new(|| tokio::sync::RwLock::new(HashMap::new()));

/// Store a decrypted manifest (async). FOR DEBUG/TEST USE ONLY.
/// This function should NOT be called during normal operation as the machine plane
/// should be stateless and not persist manifest content. Only use for debugging endpoints.
pub async fn store_decrypted_manifest(manifest_id: &str, value: serde_json::Value) {
    let mut map = DECRYPTED_MANIFESTS.write().await;
    map.insert(manifest_id.to_string(), value.clone());
    log::warn!(
        "store_decrypted_manifest: stored manifest_id='{}' value_preview='{}'",
        manifest_id,
        serde_json::to_string(&value)
            .unwrap_or("(invalid)".to_string())
            .chars()
            .take(100)
            .collect::<String>()
    );
}

/// Store the mapping of operation_id -> manifest_cid for consistent manifest ID usage
pub async fn store_operation_manifest_mapping(operation_id: &str, manifest_cid: &str) {
    let mut map = OPERATION_MANIFEST_MAPPING.write().await;
    map.insert(operation_id.to_string(), manifest_cid.to_string());
    log::info!(
        "store_operation_manifest_mapping: operation_id={} -> manifest_cid={}",
        operation_id,
        manifest_cid
    );
}

/// Get the manifest_cid for a given operation_id
pub async fn get_manifest_cid_for_operation(operation_id: &str) -> Option<String> {
    let map = OPERATION_MANIFEST_MAPPING.read().await;
    map.get(operation_id).cloned()
}

/// Return all decrypted manifests as a JSON object.
pub async fn get_decrypted_manifests_map() -> serde_json::Value {
    let map = DECRYPTED_MANIFESTS.read().await;
    serde_json::to_value(map.clone()).unwrap_or(serde_json::json!({}))
}

pub fn build_router(
    peer_rx: watch::Receiver<Vec<String>>,
    control_tx: mpsc::UnboundedSender<crate::network::control::Libp2pControl>,
) -> Router {
    let state = RestState {
        peer_rx,
        control_tx,
        tender_store: Arc::new(RwLock::new(HashMap::new())),
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
        .route("/debug/decrypted_manifests", get(debug_decrypted_manifests))
        .route("/debug/dht/active_announces", get(debug_active_announces))
        .route("/debug/dht/peers", get(debug_dht_peers))
        .route("/debug/peers", get(debug_peers))
        .route("/debug/tenders", get(debug_all_tenders))
        .route(
            "/debug/workloads_by_peer/{peer_id}",
            get(debug_workloads_by_peer),
        )
        .route("/debug/local_peer_id", get(debug_local_peer_id))
        .route(
            "/tenders/{tender_id}/manifest_id",
            get(get_tender_manifest_id),
        )
        .route("/tenders", post(create_tender))
        .route("/tenders/{tender_id}", get(get_tender_status))
        .route("/tenders/{tender_id}", delete(delete_tender))
        .route("/tenders/{tender_id}/candidates", post(get_candidates))
        .route("/apply_direct/{peer_id}", post(apply_direct))
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
    let response_data =
        crate::messages::machine::build_candidates_response_with_keys(true, &candidates);

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
    pub manifest_cid: Option<String>,
    // store last generated operation id for manifest id computation
    pub last_operation_id: Option<String>,
    pub owner_pubkey: Vec<u8>,
    pub kube: Option<KubeResourceRecord>,
}

pub async fn create_tender(
    State(state): State<RestState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
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

    // Calculate manifest_id deterministically and get operation_id from query params
    let (manifest_id, operation_id) = if let Some(id) = params.get("manifest_id") {
        // If manifest_id is provided, use it directly (operation_id might be empty)
        (
            id.clone(),
            params
                .get("operation_id")
                .cloned()
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        )
    } else if let Some(operation_id) = params.get("operation_id") {
        // Calculate manifest_id using the same method as in apply processing
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();

        // Use stable manifest name for consistent hashing
        if let Some(name) =
            crate::messages::machine::extract_manifest_name(&payload_bytes_for_parsing)
        {
            name.hash(&mut hasher);
        } else {
            // Fallback to content hash if no name found
            payload_bytes_for_parsing.hash(&mut hasher);
        }
        let manifest_id = format!("{:x}", hasher.finish())[..16].to_string();
        (manifest_id, operation_id.clone())
    } else {
        // Fallback: use a UUID for tender_id but also generate manifest_id from content
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let operation_id = uuid::Uuid::new_v4().to_string();
        let mut hasher = DefaultHasher::new();

        // Use stable manifest name for consistent hashing
        if let Some(name) =
            crate::messages::machine::extract_manifest_name(&payload_bytes_for_parsing)
        {
            name.hash(&mut hasher);
        } else {
            // Fallback to content hash if no name found
            payload_bytes_for_parsing.hash(&mut hasher);
        }
        let manifest_id = format!("{:x}", hasher.finish())[..16].to_string();
        (manifest_id, operation_id)
    };

    // Generate a unique tender_id to satisfy the ULID requirement in the spec
    let tender_id = Ulid::new().to_string();
    log::info!("create_tender: generated tender_id='{}'", tender_id);

    // Store the operation_id -> manifest_id mapping for later self-apply lookups
    store_operation_manifest_mapping(&operation_id, &manifest_id).await;

    let owner_pubkey = Vec::new();

    // Store manifest bytes as-is
    let manifest_bytes_to_store = payload_bytes_for_parsing;
    let manifest_str = String::from_utf8_lossy(&manifest_bytes_to_store).to_string();
    register_local_manifest(&tender_id, &manifest_str);

    let rec = TenderRecord {
        manifest_bytes: manifest_bytes_to_store,
        created_at: std::time::SystemTime::now(),
        manifests_distributed: HashMap::new(),
        assigned_peers: None,
        manifest_cid: Some(manifest_id.clone()),
        last_operation_id: Some(operation_id),
        owner_pubkey: owner_pubkey.clone(),
        kube: None,
    };
    {
        let mut store = state.tender_store.write().await;
        log::info!(
            "create_tender: storing tender with tender_id='{}' in tender_store",
            tender_id
        );
        log::info!(
            "create_tender: tender_store had {} tenders before insert",
            store.len()
        );
        store.insert(tender_id.clone(), rec);
        log::info!(
            "create_tender: tender_store now has {} tenders after insert",
            store.len()
        );
        log::info!(
            "create_tender: verifying tender_id '{}' exists in store: {}",
            tender_id,
            store.contains_key(&tender_id)
        );
    }

    if !owner_pubkey.is_empty() {
        crate::scheduler::record_manifest_owner(&manifest_id, &owner_pubkey).await;
    } else {
        log::warn!(
            "create_tender: missing owner pubkey when recording manifest_id={}",
            manifest_id
        );
    }

    let response_data = crate::messages::machine::build_tender_create_response(
        true,
        &tender_id,
        &manifest_id,
        DEFAULT_SELECTION_WINDOW_MS,
        "",
    );

    // Return plain response
    Ok(axum::response::Response::builder()
        .status(200)
        .header("Content-Type", "application/octet-stream")
        .body(axum::body::Body::from(response_data))
        .unwrap())
}

// Debug: return the decrypted manifests collected by this node (for testing)
async fn debug_decrypted_manifests(
    State(_state): State<RestState>,
) -> axum::Json<serde_json::Value> {
    let data = get_decrypted_manifests_map().await;
    axum::Json(serde_json::json!({"ok": true, "decrypted_manifests": data}))
}

// Debug: return the active announces (provider CIDs) tracked by the control module
async fn debug_active_announces(State(_state): State<RestState>) -> axum::Json<serde_json::Value> {
    // access the static ACTIVE_ANNOUNCES in control module
    let cids = crate::network::control::list_active_announces();
    axum::Json(serde_json::json!({"ok": true, "cids": cids}))
}

/// Debug endpoint to get local peer ID
async fn debug_local_peer_id(State(state): State<RestState>) -> axum::Json<serde_json::Value> {
    // Get the local peer ID from the control channel
    use tokio::sync::mpsc;
    let (reply_tx, mut reply_rx) = mpsc::unbounded_channel();

    let control_msg = crate::network::control::Libp2pControl::GetLocalPeerId { reply_tx };

    if let Err(_) = state.control_tx.send(control_msg) {
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

/// Debug endpoint to get workloads deployed by a specific peer ID
async fn debug_workloads_by_peer(
    Path(peer_id): Path<String>,
    State(_state): State<RestState>,
) -> axum::Json<serde_json::Value> {
    log::info!(
        "Debug workload query for peer {} received, but mock runtime support has been removed",
        peer_id
    );

    axum::Json(serde_json::json!({
        "ok": false,
        "error": "Mock runtime has been removed; workload inspection is unavailable",
        "peer_id": peer_id,
        "workload_count": 0,
        "workloads": {}
    }))
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

// Debug: list all tenders with their manifest CIDs
async fn debug_all_tenders(State(state): State<RestState>) -> axum::Json<serde_json::Value> {
    let store = state.tender_store.read().await;
    let mut tenders = serde_json::Map::new();

    for (tender_id, record) in store.iter() {
        tenders.insert(tender_id.clone(), serde_json::json!({
            "manifest_cid": record.manifest_cid,
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

async fn get_tender_manifest_id(
    Path(tender_id): Path<String>,
    State(state): State<RestState>,
    _headers: HeaderMap,
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    let maybe = { state.tender_store.read().await.get(&tender_id).cloned() };
    let tender = match maybe {
        Some(t) => t,
        None => {
            let error_response =
                crate::messages::machine::build_tender_status_response("", "Error", &[], None);
            return create_response_with_fallback(&error_response).await;
        }
    };

    let operation_id = match tender.last_operation_id {
        Some(o) => o,
        None => {
            let error_response =
                crate::messages::machine::build_tender_status_response("", "Error", &[], None);
            return create_response_with_fallback(&error_response).await;
        }
    };

    // Use stored manifest_cid instead of recalculating
    let manifest_id = match get_manifest_cid_for_operation(&operation_id).await {
        Some(cid) => {
            log::info!(
                "get_tender_manifest_id: returning stored manifest_cid={} for operation_id={}",
                cid,
                operation_id
            );
            cid
        }
        None => {
            // Fallback to tender record manifest_cid if available
            if let Some(cid) = &tender.manifest_cid {
                log::warn!(
                    "get_tender_manifest_id: using tender.manifest_cid={} for operation_id={}",
                    cid,
                    operation_id
                );
                cid.clone()
            } else {
                log::error!(
                    "get_tender_manifest_id: no manifest_cid found for operation_id={}",
                    operation_id
                );
                let error_response =
                    crate::messages::machine::build_tender_status_response("", "Error", &[], None);
                return create_response_with_fallback(&error_response).await;
            }
        }
    };

    let response_data = serde_json::json!({"ok": true, "manifest_id": &manifest_id});
    let response_str = serde_json::to_string(&response_data).unwrap_or_default();
    // Response is already protected by the transport; return as plain bytes
    create_response_with_fallback(response_str.as_bytes()).await
}

pub async fn get_tender_status(
    Path(tender_id): Path<String>,
    State(state): State<RestState>,
    _headers: HeaderMap,
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    let maybe = { state.tender_store.read().await.get(&tender_id).cloned() };
    if let Some(r) = maybe {
        let assigned = r.assigned_peers.unwrap_or_default();

        let response_data = crate::messages::machine::build_tender_status_response(
            &tender_id,
            "Pending",
            &assigned,
            r.manifest_cid.as_deref(),
        );
        return create_response_with_fallback(&response_data).await;
    }
    let error_response =
        crate::messages::machine::build_tender_status_response("", "Error", &[], None);
    create_response_with_fallback(&error_response).await
}

/// Delete a tender by tender ID - discovers providers and sends delete requests
pub async fn delete_tender(
    Path(tender_id): Path<String>,
    State(_state): State<RestState>,
    _headers: HeaderMap,
    body: Bytes,
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    info!("delete_tender: tender_id={}", tender_id);

    // Generate operation ID for this delete request
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let operation_id = format!("delete-{}-{}", tender_id, timestamp);

    // Parse query parameters for force flag
    let force = String::from_utf8_lossy(&body).contains("force=true");

    // Step 1: Handle local deletion directly
    // In the resource pool model, we only manage local resources.
    // Discovery and fabric-wide choreography is handled by external components.

    let mut removed_workloads = Vec::new();
    let success;
    let message;

    match handle_local_delete(&tender_id, force).await {
        Ok(local_removed) => {
            if local_removed.is_empty() {
                success = false;
                message = "No local workloads found for this manifest".to_string();
            } else {
                success = true;
                message = format!(
                    "Successfully removed {} local workloads",
                    local_removed.len()
                );
                removed_workloads = local_removed;
            }
        }
        Err(e) => {
            success = false;
            message = format!("Local deletion failed: {}", e);
        }
    }

    let response = crate::messages::machine::build_delete_response(
        success,
        &operation_id,
        &message,
        &tender_id,
        &removed_workloads,
    );

    // Return plain response
    Ok(axum::response::Response::builder()
        .status(200)
        .header("Content-Type", "application/octet-stream")
        .body(axum::body::Body::from(response))
        .unwrap())
}

/// Handle local workload deletion when the provider is the same machine
async fn handle_local_delete(manifest_id: &str, _force: bool) -> Result<Vec<String>, String> {
    info!(
        "handle_local_delete: processing manifest_id={}",
        manifest_id
    );

    // Use the workload integration module to remove workloads by manifest ID
    match crate::scheduler::remove_workloads_by_manifest_id(manifest_id).await {
        Ok(removed_workloads) => {
            info!(
                "handle_local_delete: successfully removed {} workloads for manifest_id '{}'",
                removed_workloads.len(),
                manifest_id
            );
            Ok(removed_workloads)
        }
        Err(e) => {
            error!(
                "handle_local_delete: failed to remove workloads for manifest_id '{}': {}",
                manifest_id, e
            );
            Err(format!("local deletion failed: {}", e))
        }
    }
}

/// Forward an ApplyRequest directly to a specific peer via libp2p
/// This bypasses centralized tender storage and forwards the request directly
pub async fn apply_direct(
    Path(peer_id): Path<String>,
    State(state): State<RestState>,
    _headers: HeaderMap,
    body: Bytes,
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    debug!("apply_direct: forwarding ApplyRequest to peer {}", peer_id);

    // Use the request body as-is
    let apply_request_bytes = body.to_vec();

    // Parse the peer string into a PeerId
    let target_peer_id: libp2p::PeerId = match peer_id.parse() {
        Ok(id) => id,
        Err(e) => {
            log::warn!("apply_direct: invalid peer ID '{}': {}", peer_id, e);
            let error_response = crate::messages::machine::build_apply_response(
                false,
                "unknown",
                &format!("Invalid peer ID: {}", e),
            );
            return create_response_with_fallback(&error_response).await;
        }
    };

    // Forward the ApplyRequest directly to the peer via libp2p
    let (reply_tx, mut reply_rx) = mpsc::unbounded_channel::<Result<String, String>>();
    if let Err(e) =
        state
            .control_tx
            .send(crate::network::control::Libp2pControl::SendApplyRequest {
                peer_id: target_peer_id,
                manifest: apply_request_bytes,
                reply_tx,
            })
    {
        log::error!("apply_direct: failed to send to libp2p control: {}", e);
        let error_response = crate::messages::machine::build_apply_response(
            false,
            "unknown",
            "Failed to forward request to libp2p",
        );
        return create_response_with_fallback(&error_response).await;
    }

    // Wait for response with timeout
    let result = match tokio::time::timeout(Duration::from_secs(30), reply_rx.recv()).await {
        Ok(Some(Ok(_msg))) => {
            debug!("apply_direct: success for peer {}", peer_id);
            crate::messages::machine::build_apply_response(
                true,
                "forwarded",
                "Request forwarded successfully",
            )
        }
        Ok(Some(Err(e))) => {
            log::warn!("apply_direct: error for peer {}: {}", peer_id, e);
            crate::messages::machine::build_apply_response(
                false,
                "forwarded",
                &format!("Forward failed: {}", e),
            )
        }
        _ => {
            log::warn!("apply_direct: timeout for peer {}", peer_id);
            crate::messages::machine::build_apply_response(
                false,
                "forwarded",
                "Timeout waiting for peer response",
            )
        }
    };

    // Return response (simplified - no encryption needed for this case)
    create_response_with_fallback(&result).await
}
