#[cfg(debug_assertions)]
use crate::runtime::RuntimeEngine;
use axum::{
    Router,
    body::Bytes,
    extract::{Extension, Path, Query, State},
    http::HeaderMap,
    middleware,
    routing::{delete, get, post},
};
use base64::Engine;
use log::{debug, error, info, warn};
use once_cell::sync::Lazy;
use protocol::libp2p_constants::{FREE_CAPACITY_PREFIX, FREE_CAPACITY_TIMEOUT_MS};
use serde_json::Value;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tokio::sync::mpsc;
use tokio::{sync::watch, time::Duration};

pub mod envelope_handler;
pub mod kube;
use envelope_handler::{
    EnvelopeHandler, create_encrypted_response_with_key, create_response_for_envelope_metadata,
    create_response_with_fallback,
};

async fn get_nodes(
    State(state): State<RestState>,
    _headers: HeaderMap,
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    let peers = state.peer_rx.borrow().clone();
    let response_data = protocol::machine::build_nodes_response(&peers);

    // No envelope metadata available, return unencrypted response
    create_response_with_fallback(&response_data).await
}

async fn get_kem_public_key(State(_state): State<RestState>) -> String {
    // Get the machine's KEM public key for encryption
    match crypto::ensure_kem_keypair_on_disk() {
        Ok((kem_pub_bytes, _)) => base64::engine::general_purpose::STANDARD.encode(&kem_pub_bytes),
        Err(e) => format!("ERROR: Failed to get KEM public key: {}", e),
    }
}

async fn get_signing_public_key(State(_state): State<RestState>) -> String {
    // Get the machine's signing public key for signature verification
    match crypto::ensure_keypair_on_disk() {
        Ok((signing_pub_bytes, _)) => {
            base64::engine::general_purpose::STANDARD.encode(&signing_pub_bytes)
        }
        Err(e) => format!("ERROR: Failed to get signing public key: {}", e),
    }
}

#[derive(Clone)]
pub struct RestState {
    pub peer_rx: watch::Receiver<Vec<String>>,
    pub control_tx: mpsc::UnboundedSender<crate::libp2p_beemesh::control::Libp2pControl>,
    task_store: Arc<RwLock<HashMap<String, TaskRecord>>>,
    pub envelope_handler: std::sync::Arc<EnvelopeHandler>,
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
    control_tx: mpsc::UnboundedSender<crate::libp2p_beemesh::control::Libp2pControl>,
    envelope_handler: std::sync::Arc<EnvelopeHandler>,
) -> Router {
    let state = RestState {
        peer_rx,
        control_tx,
        task_store: Arc::new(RwLock::new(HashMap::new())),
        envelope_handler,
    };
    let kube_routes = Router::new()
        .route("/version", get(kube::version))
        .route("/apis", get(kube::api_group_list))
        .nest("/api", kube::core_router())
        .nest("/apis/apps/v1", kube::apps_v1_router());

    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/api/v1/kem_pubkey", get(get_kem_public_key))
        .route("/api/v1/signing_pubkey", get(get_signing_public_key))
        .route("/debug/decrypted_manifests", get(debug_decrypted_manifests))
        .route("/debug/dht/active_announces", get(debug_active_announces))
        .route("/debug/dht/peers", get(debug_dht_peers))
        .route("/debug/peers", get(debug_peers))
        .route("/debug/tasks", get(debug_all_tasks))
        .route(
            "/debug/workloads_by_peer/{peer_id}",
            get(debug_workloads_by_peer),
        )
        .route("/debug/local_peer_id", get(debug_local_peer_id))
        .route("/tasks/{task_id}/manifest_id", get(get_task_manifest_id))
        .route("/tasks", post(create_task))
        .route("/tasks/{task_id}", get(get_task_status))
        .route("/tasks/{task_id}", delete(delete_task))
        .route("/tasks/{task_id}/candidates", post(get_candidates))
        .route("/apply_direct/{peer_id}", post(apply_direct))
        .route("/nodes", get(get_nodes))
        .merge(kube_routes)
        // Add envelope middleware to decrypt incoming requests and extract peer keys
        .layer(middleware::from_fn_with_state(
            state.envelope_handler.clone(),
            envelope_handler::envelope_middleware,
        ))
        // state
        .with_state(state)
}

pub async fn get_candidates(
    Path(task_id): Path<String>,
    State(state): State<RestState>,
    Extension(envelope_metadata): Extension<crate::restapi::envelope_handler::EnvelopeMetadata>,
    _headers: HeaderMap,
    _body: Bytes,
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    log::info!(
        "get_candidates: called for task_id={} (direct delivery mode)",
        task_id
    );

    let candidates = collect_candidate_pubkeys(&state, &task_id, 5).await?;
    let response_data = protocol::machine::build_candidates_response_with_keys(true, &candidates);

    // Use KEM key from envelope metadata for secure response encryption if available
    if !envelope_metadata.kem_pubkey.is_empty() {
        create_encrypted_response_with_key(
            &state.envelope_handler,
            &response_data,
            "candidates_response",
            envelope_metadata.peer_id.as_deref(),
            &envelope_metadata.kem_pubkey,
        )
        .await
    } else {
        // No KEM key in metadata, return unencrypted response
        create_response_with_fallback(&response_data).await
    }
}

pub(crate) async fn collect_candidate_pubkeys(
    state: &RestState,
    task_id: &str,
    max_candidates: usize,
) -> Result<Vec<(String, String)>, axum::http::StatusCode> {
    let request_id = format!(
        "{}:{}:{}",
        FREE_CAPACITY_PREFIX,
        task_id,
        uuid::Uuid::new_v4()
    );
    let capacity_fb = protocol::machine::build_capacity_request(
        500u32,
        512u64 * 1024 * 1024,
        10u64 * 1024 * 1024 * 1024,
        1u32,
    );
    let (reply_tx, mut reply_rx) = mpsc::unbounded_channel::<String>();
    let _ = state.control_tx.send(
        crate::libp2p_beemesh::control::Libp2pControl::QueryCapacityWithPayload {
            request_id: request_id.clone(),
            reply_tx: reply_tx.clone(),
            payload: capacity_fb,
        },
    );

    let mut responders: Vec<String> = Vec::new();
    let start = std::time::Instant::now();
    let timeout = Duration::from_millis(FREE_CAPACITY_TIMEOUT_MS);
    log::info!(
        "collect_candidate_pubkeys: waiting for responses, timeout={}ms",
        FREE_CAPACITY_TIMEOUT_MS
    );

    while start.elapsed() < timeout {
        let remaining = timeout.saturating_sub(start.elapsed());
        match tokio::time::timeout(remaining, reply_rx.recv()).await {
            Ok(Some(peer)) => {
                if !responders.contains(&peer) {
                    responders.push(peer);
                    if responders.len() >= max_candidates {
                        break;
                    }
                }
            }
            Ok(None) => {
                log::warn!("collect_candidate_pubkeys: channel closed");
                break;
            }
            Err(_) => {
                break;
            }
        }
    }

    let mut candidates: Vec<(String, String)> = Vec::new();
    for peer_with_key in responders {
        if let Some(colon_pos) = peer_with_key.find(':') {
            let peer_id_str = &peer_with_key[..colon_pos];
            let pubkey_b64 = &peer_with_key[colon_pos + 1..];
            if !pubkey_b64.is_empty() {
                candidates.push((peer_id_str.to_string(), pubkey_b64.to_string()));
            } else {
                log::warn!(
                    "collect_candidate_pubkeys: peer {} has no public key, skipping",
                    peer_id_str
                );
            }
        } else {
            log::warn!(
                "collect_candidate_pubkeys: invalid peer entry: {}",
                peer_with_key
            );
        }
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
pub struct TaskRecord {
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

pub async fn create_task(
    State(state): State<RestState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    _headers: HeaderMap,
    Extension(envelope_metadata): Extension<crate::restapi::envelope_handler::EnvelopeMetadata>,
    body: axum::body::Bytes,
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    debug!("create_task: parsing decrypted payload from envelope middleware");

    // The envelope middleware has already decrypted the payload for us
    // We receive the inner EncryptedManifest flatbuffer directly
    let payload_bytes_for_parsing = body.to_vec();

    log::info!(
        "create_task: received payload len={}, first_20_bytes={:02x?}",
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
        if let Some(name) = protocol::machine::extract_manifest_name(&payload_bytes_for_parsing) {
            name.hash(&mut hasher);
        } else {
            // Fallback to content hash if no name found
            payload_bytes_for_parsing.hash(&mut hasher);
        }
        let manifest_id = format!("{:x}", hasher.finish())[..16].to_string();
        (manifest_id, operation_id.clone())
    } else {
        // Fallback: use a UUID for task_id but also generate manifest_id from content
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let operation_id = uuid::Uuid::new_v4().to_string();
        let mut hasher = DefaultHasher::new();

        // Use stable manifest name for consistent hashing
        if let Some(name) = protocol::machine::extract_manifest_name(&payload_bytes_for_parsing) {
            name.hash(&mut hasher);
        } else {
            // Fallback to content hash if no name found
            payload_bytes_for_parsing.hash(&mut hasher);
        }
        let manifest_id = format!("{:x}", hasher.finish())[..16].to_string();
        (manifest_id, operation_id)
    };

    // Use manifest_id as the task_id (since manifest_id is the central identifier)
    let task_id = manifest_id.clone();
    log::info!(
        "create_task: using manifest_id='{}' as task_id='{}'",
        manifest_id,
        task_id
    );

    // Store the operation_id -> manifest_id mapping for later self-apply lookups
    store_operation_manifest_mapping(&operation_id, &manifest_id).await;

    // Extract owner public key from secure request extensions (set by envelope middleware)
    let owner_pubkey = envelope_metadata.signing_pubkey.clone();

    log::info!(
        "create_task: owner_pubkey len={} for manifest_id={}",
        owner_pubkey.len(),
        manifest_id
    );

    // Parse as EncryptedManifest flatbuffer only (no YAML support)
    log::info!(
        "create_task: attempting to parse payload_bytes len={} as EncryptedManifest",
        payload_bytes_for_parsing.len()
    );
    log::info!(
        "create_task: payload_bytes first 20 bytes={:02x?}",
        &payload_bytes_for_parsing[..std::cmp::min(20, payload_bytes_for_parsing.len())]
    );

    // Create envelope for the encrypted payload - no longer need to parse as EncryptedManifest
    let manifest_bytes_to_store =
        if !payload_bytes_for_parsing.is_empty() && payload_bytes_for_parsing[0] == 0x02 {
            log::info!("create_task: detected encrypted manifest payload (recipient-blob format)");

            // Create a proper envelope containing the encrypted payload for decryption
            // The decryption process expects an envelope with payload_type="manifest"
            let envelope_nonce: [u8; 16] = rand::random();
            let nonce_str = base64::engine::general_purpose::STANDARD.encode(&envelope_nonce);
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);

            protocol::machine::build_envelope_canonical(
                &payload_bytes_for_parsing,
                "manifest",
                &nonce_str,
                ts,
                "ml-kem-512",
                None,
            )
        } else {
            log::info!("create_task: payload appears to be plain manifest or other format");
            payload_bytes_for_parsing
        };

    let rec = TaskRecord {
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
        let mut store = state.task_store.write().await;
        log::info!(
            "create_task: storing task with task_id='{}' in task_store",
            task_id
        );
        log::info!(
            "create_task: task_store had {} tasks before insert",
            store.len()
        );
        store.insert(task_id.clone(), rec);
        log::info!(
            "create_task: task_store now has {} tasks after insert",
            store.len()
        );
        log::info!(
            "create_task: verifying task_id '{}' exists in store: {}",
            task_id,
            store.contains_key(&task_id)
        );
    }

    if !owner_pubkey.is_empty() {
        crate::workload_integration::record_manifest_owner(&manifest_id, &owner_pubkey).await;
    } else {
        log::warn!(
            "create_task: missing owner pubkey when recording manifest_id={}",
            manifest_id
        );
    }

    let response_data = protocol::machine::build_task_create_response(
        true,
        &task_id,
        &manifest_id,
        FREE_CAPACITY_TIMEOUT_MS,
    );

    // Use KEM key directly from envelope metadata for secure response encryption
    if !envelope_metadata.kem_pubkey.is_empty() {
        create_encrypted_response_with_key(
            &state.envelope_handler,
            &response_data,
            "task_create_response",
            envelope_metadata.peer_id.as_deref(),
            &envelope_metadata.kem_pubkey,
        )
        .await
    } else {
        // No KEM key in metadata, return unencrypted response
        create_response_with_fallback(&response_data).await
    }
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
    let cids = crate::libp2p_beemesh::control::list_active_announces();
    axum::Json(serde_json::json!({"ok": true, "cids": cids}))
}

/// Debug endpoint to get local peer ID
async fn debug_local_peer_id(State(state): State<RestState>) -> axum::Json<serde_json::Value> {
    // Get the local peer ID from the control channel
    use tokio::sync::mpsc;
    let (reply_tx, mut reply_rx) = mpsc::unbounded_channel();

    let control_msg = crate::libp2p_beemesh::control::Libp2pControl::GetLocalPeerId { reply_tx };

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
    // Try to access the global runtime registry to get MockEngine state
    #[cfg(debug_assertions)]
    {
        if let Some(registry_guard) =
            crate::workload_integration::get_global_runtime_registry().await
        {
            if let Some(ref registry) = *registry_guard {
                if let Some(mock_engine) = registry.get_engine("mock") {
                    if let Some(mock_engine) = mock_engine
                        .as_any()
                        .downcast_ref::<crate::runtime::mock::MockEngine>()
                    {
                        let peer_workloads = mock_engine.get_workloads_by_peer(&peer_id);
                        let mut workloads_json = serde_json::Map::new();

                        for workload in &peer_workloads {
                            let exported_manifest = match mock_engine
                                .export_manifest(&workload.info.id)
                                .await
                            {
                                Ok(manifest_bytes) => match String::from_utf8(manifest_bytes) {
                                    Ok(manifest_str) => Some(manifest_str),
                                    Err(e) => {
                                        log::warn!(
                                            "Failed to convert manifest bytes to string for workload {}: {}",
                                            workload.info.id,
                                            e
                                        );
                                        None
                                    }
                                },
                                Err(e) => {
                                    log::warn!(
                                        "Failed to export manifest for workload {}: {}",
                                        workload.info.id,
                                        e
                                    );
                                    None
                                }
                            };

                            workloads_json.insert(
                                workload.info.id.clone(),
                                serde_json::json!({
                                    "manifest_id": workload.info.manifest_id,
                                    "status": format!("{:?}", workload.info.status),
                                    "metadata": workload.info.metadata,
                                    "created_at": workload.info.created_at.duration_since(std::time::UNIX_EPOCH)
                                        .unwrap_or_default().as_secs(),
                                    "updated_at": workload.info.updated_at.duration_since(std::time::UNIX_EPOCH)
                                        .unwrap_or_default().as_secs(),
                                    "ports": workload.info.ports,
                                    "exported_manifest": exported_manifest,
                                })
                            );
                        }

                        return axum::Json(serde_json::json!({
                            "ok": true,
                            "peer_id": peer_id,
                            "workload_count": peer_workloads.len(),
                            "workloads": workloads_json
                        }));
                    }
                }
            }
        }
    }

    axum::Json(serde_json::json!({
        "ok": false,
        "error": "MockEngine not available",
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
    let control_msg = crate::libp2p_beemesh::control::Libp2pControl::GetDhtPeers { reply_tx };

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

// Debug: list all tasks with their manifest CIDs
async fn debug_all_tasks(State(state): State<RestState>) -> axum::Json<serde_json::Value> {
    let store = state.task_store.read().await;
    let mut tasks = serde_json::Map::new();

    for (task_id, record) in store.iter() {
        tasks.insert(task_id.clone(), serde_json::json!({
            "manifest_cid": record.manifest_cid,
            "created_at": record.created_at.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs(),
            "assigned_peers": record.assigned_peers,
            "manifest_bytes_len": record.manifest_bytes.len()
        }));
    }

    axum::Json(serde_json::json!({
        "ok": true,
        "tasks": serde_json::Value::Object(tasks)
    }))
}

async fn get_task_manifest_id(
    Path(task_id): Path<String>,
    State(state): State<RestState>,
    _headers: HeaderMap,
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    let maybe = { state.task_store.read().await.get(&task_id).cloned() };
    let task = match maybe {
        Some(t) => t,
        None => {
            let error_response =
                protocol::machine::build_task_status_response("", "Error", &[], None);
            return create_response_with_fallback(&error_response).await;
        }
    };

    let operation_id = match task.last_operation_id {
        Some(o) => o,
        None => {
            let error_response =
                protocol::machine::build_task_status_response("", "Error", &[], None);
            return create_response_with_fallback(&error_response).await;
        }
    };

    // Use stored manifest_cid instead of recalculating
    let manifest_id = match get_manifest_cid_for_operation(&operation_id).await {
        Some(cid) => {
            log::info!(
                "get_task_manifest_id: returning stored manifest_cid={} for operation_id={}",
                cid,
                operation_id
            );
            cid
        }
        None => {
            // Fallback to task record manifest_cid if available
            if let Some(cid) = &task.manifest_cid {
                log::warn!(
                    "get_task_manifest_id: using task.manifest_cid={} for operation_id={}",
                    cid,
                    operation_id
                );
                cid.clone()
            } else {
                log::error!(
                    "get_task_manifest_id: no manifest_cid found for operation_id={}",
                    operation_id
                );
                let error_response =
                    protocol::machine::build_task_status_response("", "Error", &[], None);
                return create_response_with_fallback(&error_response).await;
            }
        }
    };

    let response_data = serde_json::json!({"ok": true, "manifest_id": &manifest_id});
    let response_str = serde_json::to_string(&response_data).unwrap_or_default();
    // No envelope metadata available, return unencrypted response
    create_response_with_fallback(response_str.as_bytes()).await
}

pub async fn get_task_status(
    Path(task_id): Path<String>,
    State(state): State<RestState>,
    _headers: HeaderMap,
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    let maybe = { state.task_store.read().await.get(&task_id).cloned() };
    if let Some(r) = maybe {
        let assigned = r.assigned_peers.unwrap_or_default();

        let response_data = protocol::machine::build_task_status_response(
            &task_id,
            "Pending",
            &assigned,
            r.manifest_cid.as_deref(),
        );
        return create_response_with_fallback(&response_data).await;
    }
    let error_response = protocol::machine::build_task_status_response("", "Error", &[], None);
    create_response_with_fallback(&error_response).await
}

/// Delete a task by task ID - discovers providers and sends delete requests
pub async fn delete_task(
    Path(task_id): Path<String>,
    State(state): State<RestState>,
    _headers: HeaderMap,
    Extension(envelope_metadata): Extension<crate::restapi::envelope_handler::EnvelopeMetadata>,
    body: Bytes,
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    info!("delete_task: task_id={}", task_id);

    // Generate operation ID for this delete request
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let operation_id = format!("delete-{}-{}", task_id, timestamp);

    // Extract requesting peer info from envelope metadata
    let origin_peer = envelope_metadata
        .peer_id
        .clone()
        .unwrap_or_else(|| "unknown".to_string());

    // Parse query parameters for force flag
    let force = String::from_utf8_lossy(&body).contains("force=true");

    // Step 1: Discover which nodes are providing this task/manifest
    let providers = match find_manifest_providers(&task_id, &state).await {
        Ok(providers) => providers,
        Err(e) => {
            error!(
                "Failed to discover providers for task_id {}: {}",
                task_id, e
            );
            let error_response = protocol::machine::build_delete_response(
                false,
                &operation_id,
                &format!("Failed to discover providers: {}", e),
                &task_id,
                &[],
            );
            return create_response_for_envelope_metadata(
                &state.envelope_handler,
                &error_response,
                "delete_response",
                &envelope_metadata,
            )
            .await;
        }
    };

    if providers.is_empty() {
        warn!("No providers found for task_id: {}", task_id);
        let response = protocol::machine::build_delete_response(
            true,
            &operation_id,
            "No providers found for task",
            &task_id,
            &[],
        );
        return create_response_for_envelope_metadata(
            &state.envelope_handler,
            &response,
            "delete_response",
            &envelope_metadata,
        )
        .await;
    }

    info!(
        "Found {} providers for task_id {}: {:?}",
        providers.len(),
        task_id,
        providers
    );

    // Step 2: Create delete request
    let delete_request =
        protocol::machine::build_delete_request(&task_id, &operation_id, &origin_peer, force);

    // Step 3: Send delete requests to all providers
    let mut successful_deletes = Vec::new();
    let mut failed_deletes = Vec::new();
    let mut removed_workloads = Vec::new();

    // Get our local peer ID for comparison
    let (local_peer_tx, mut local_peer_rx) = mpsc::unbounded_channel();
    if let Err(e) = state.control_tx.send(
        crate::libp2p_beemesh::control::Libp2pControl::GetLocalPeerId {
            reply_tx: local_peer_tx,
        },
    ) {
        error!("Failed to get local peer ID: {}", e);
        let error_response = protocol::machine::build_delete_response(
            false,
            &operation_id,
            &format!("Failed to get local peer ID: {}", e),
            &task_id,
            &[],
        );
        return create_response_for_envelope_metadata(
            &state.envelope_handler,
            &error_response,
            "delete_response",
            &envelope_metadata,
        )
        .await;
    }

    let local_peer_id =
        match tokio::time::timeout(Duration::from_secs(2), local_peer_rx.recv()).await {
            Ok(Some(peer_id)) => peer_id,
            _ => {
                error!("Timeout getting local peer ID");
                let error_response = protocol::machine::build_delete_response(
                    false,
                    &operation_id,
                    "Timeout getting local peer ID",
                    &task_id,
                    &[],
                );
                return create_response_for_envelope_metadata(
                    &state.envelope_handler,
                    &error_response,
                    "delete_response",
                    &envelope_metadata,
                )
                .await;
            }
        };

    for provider_peer_id_str in providers {
        // Parse provider peer ID
        let provider_peer_id: libp2p::PeerId = match provider_peer_id_str.parse() {
            Ok(id) => id,
            Err(e) => {
                error!("Invalid provider peer ID '{}': {}", provider_peer_id_str, e);
                failed_deletes.push(format!("{}:invalid_peer_id", provider_peer_id_str));
                continue;
            }
        };

        // Check if this is a self-delete (local peer)
        if provider_peer_id == local_peer_id {
            info!("Performing self-delete for manifest_id: {}", task_id);

            // Handle local workload deletion directly
            match handle_local_delete(&task_id, force).await {
                Ok(local_removed_workloads) => {
                    info!(
                        "Self-delete successful for manifest_id {}: removed {} workloads",
                        task_id,
                        local_removed_workloads.len()
                    );
                    successful_deletes.push(provider_peer_id_str);
                    removed_workloads.extend(local_removed_workloads);
                }
                Err(e) => {
                    error!("Self-delete failed for manifest_id {}: {}", task_id, e);
                    failed_deletes
                        .push(format!("{}:self_delete_failed:{}", provider_peer_id_str, e));
                }
            }
        } else {
            // Send delete request to remote peer
            match send_delete_request_to_peer(&state, &provider_peer_id_str, &delete_request).await
            {
                Ok(result) => {
                    info!(
                        "Delete request sent to peer {}: {:?}",
                        provider_peer_id_str, result
                    );
                    successful_deletes.push(provider_peer_id_str);
                }
                Err(e) => {
                    error!(
                        "Failed to send delete request to peer {}: {}",
                        provider_peer_id_str, e
                    );
                    failed_deletes.push(format!("{}:{}", provider_peer_id_str, e));
                }
            }
        }
    }

    // Step 4: Return response
    let success = !successful_deletes.is_empty();
    let message = if success {
        if failed_deletes.is_empty() {
            format!(
                "Delete requests sent to {} providers",
                successful_deletes.len()
            )
        } else {
            format!(
                "Delete requests sent to {}/{} providers. Failures: {:?}",
                successful_deletes.len(),
                successful_deletes.len() + failed_deletes.len(),
                failed_deletes
            )
        }
    } else {
        format!(
            "Failed to send delete requests to any providers: {:?}",
            failed_deletes
        )
    };

    let response = protocol::machine::build_delete_response(
        success,
        &operation_id,
        &message,
        &task_id,
        &removed_workloads,
    );

    create_response_for_envelope_metadata(
        &state.envelope_handler,
        &response,
        "delete_response",
        &envelope_metadata,
    )
    .await
}

/// Handle local workload deletion when the provider is the same machine
async fn handle_local_delete(manifest_id: &str, _force: bool) -> Result<Vec<String>, String> {
    info!(
        "handle_local_delete: processing manifest_id={}",
        manifest_id
    );

    // Use the workload integration module to remove workloads by manifest ID
    match crate::workload_integration::remove_workloads_by_manifest_id(manifest_id).await {
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

/// Find providers for a task using DHT discovery
async fn find_manifest_providers(task_id: &str, state: &RestState) -> Result<Vec<String>, String> {
    info!("find_manifest_providers: searching for task_id={}", task_id);

    // Use the libp2p control system to find providers for this manifest
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    // Send request to find providers via DHT
    let control_msg = crate::libp2p_beemesh::control::Libp2pControl::FindManifestHolders {
        manifest_id: task_id.to_string(),
        reply_tx: tx,
    };

    if let Err(e) = state.control_tx.send(control_msg) {
        warn!("Failed to send find providers request: {}", e);
        return Ok(vec![]);
    }

    // Wait for response with timeout
    match tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await {
        Ok(Some(providers)) => {
            info!(
                "find_manifest_providers: found {} providers for task_id={}",
                providers.len(),
                task_id
            );
            Ok(providers
                .into_iter()
                .map(|peer_id| peer_id.to_string())
                .collect())
        }
        Ok(None) => {
            warn!(
                "find_manifest_providers: channel closed for task_id={}",
                task_id
            );
            Ok(vec![])
        }
        Err(_) => {
            warn!(
                "find_manifest_providers: timeout waiting for providers for task_id={}",
                task_id
            );
            Ok(vec![])
        }
    }
}

/// Send a delete request to a specific peer
pub(crate) async fn send_delete_request_to_peer(
    state: &RestState,
    peer_id: &str,
    delete_request: &[u8],
) -> Result<String, String> {
    info!("send_delete_request_to_peer: sending to peer={}", peer_id);

    // Parse the peer string into a PeerId
    let target_peer_id: libp2p::PeerId = peer_id
        .parse()
        .map_err(|e| format!("Invalid peer ID '{}': {}", peer_id, e))?;

    // Send the delete request via libp2p control message
    let (reply_tx, mut reply_rx) = mpsc::unbounded_channel::<Result<String, String>>();

    let control_msg = crate::libp2p_beemesh::control::Libp2pControl::SendDeleteRequest {
        peer_id: target_peer_id,
        delete_request: delete_request.to_vec(),
        reply_tx,
    };

    if let Err(e) = state.control_tx.send(control_msg) {
        return Err(format!(
            "Failed to send delete request to libp2p control: {}",
            e
        ));
    }

    // Wait for response with timeout
    match tokio::time::timeout(Duration::from_secs(10), reply_rx.recv()).await {
        Ok(Some(Ok(result))) => {
            info!(
                "Delete request to peer {} completed successfully: {}",
                peer_id, result
            );
            Ok(result)
        }
        Ok(Some(Err(e))) => {
            error!("Delete request to peer {} failed: {}", peer_id, e);
            Err(e)
        }
        Ok(None) => {
            error!("Delete request to peer {} - reply channel closed", peer_id);
            Err("Reply channel closed".to_string())
        }
        Err(_) => {
            error!("Delete request to peer {} timed out", peer_id);
            Err("Request timed out".to_string())
        }
    }
}

/// Forward an ApplyRequest directly to a specific peer via libp2p
/// This bypasses centralized task storage and forwards the request directly
pub async fn apply_direct(
    Path(peer_id): Path<String>,
    State(state): State<RestState>,
    _headers: HeaderMap,
    Extension(_envelope_metadata): Extension<crate::restapi::envelope_handler::EnvelopeMetadata>,
    body: Bytes,
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    debug!("apply_direct: forwarding ApplyRequest to peer {}", peer_id);

    // The envelope middleware has already decrypted the payload for us
    let apply_request_bytes = body.to_vec();

    // Parse the peer string into a PeerId
    let target_peer_id: libp2p::PeerId = match peer_id.parse() {
        Ok(id) => id,
        Err(e) => {
            log::warn!("apply_direct: invalid peer ID '{}': {}", peer_id, e);
            let error_response = protocol::machine::build_apply_response(
                false,
                "unknown",
                &format!("Invalid peer ID: {}", e),
            );
            return create_response_with_fallback(&error_response).await;
        }
    };

    // Forward the ApplyRequest directly to the peer via libp2p
    let (reply_tx, mut reply_rx) = mpsc::unbounded_channel::<Result<String, String>>();
    if let Err(e) = state.control_tx.send(
        crate::libp2p_beemesh::control::Libp2pControl::SendApplyRequest {
            peer_id: target_peer_id,
            manifest: apply_request_bytes,
            reply_tx,
        },
    ) {
        log::error!("apply_direct: failed to send to libp2p control: {}", e);
        let error_response = protocol::machine::build_apply_response(
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
            protocol::machine::build_apply_response(
                true,
                "forwarded",
                "Request forwarded successfully",
            )
        }
        Ok(Some(Err(e))) => {
            log::warn!("apply_direct: error for peer {}: {}", peer_id, e);
            protocol::machine::build_apply_response(
                false,
                "forwarded",
                &format!("Forward failed: {}", e),
            )
        }
        _ => {
            log::warn!("apply_direct: timeout for peer {}", peer_id);
            protocol::machine::build_apply_response(
                false,
                "forwarded",
                "Timeout waiting for peer response",
            )
        }
    };

    // Return response (simplified - no encryption needed for this case)
    create_response_with_fallback(&result).await
}
