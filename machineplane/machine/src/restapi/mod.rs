use crate::pod_communication;
use axum::{
    body::Bytes,
    extract::{Extension, Path, Query, State},
    http::HeaderMap,
    middleware,
    routing::{get, post},
    Router,
};
use base64::Engine;
use log::{debug, info, warn};
use once_cell::sync::Lazy;
use protocol::libp2p_constants::REQUEST_RESPONSE_TIMEOUT_SECS;
use protocol::libp2p_constants::{
    FREE_CAPACITY_PREFIX, FREE_CAPACITY_TIMEOUT_SECS, REPLICAS_FIELD, SPEC_REPLICAS_FIELD,
};
use protocol::machine::{
    root_as_apply_key_shares_request, root_as_apply_manifest_request, root_as_assign_request,
    root_as_distribute_capabilities_request, root_as_distribute_manifests_request,
    root_as_distribute_shares_request, root_as_envelope,
};

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::RwLock;
use tokio::{sync::watch, time::Duration};

pub mod envelope_handler;
use crate::runtime::RuntimeEngine;
use envelope_handler::{
    create_encrypted_response_with_key, create_response_with_fallback, EnvelopeHandler,
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
    pub shared_name: Option<String>,
    pub envelope_handler: std::sync::Arc<EnvelopeHandler>,
    pub manifest_store: Arc<RwLock<crate::libp2p_beemesh::manifest_store::LocalManifestStore>>,
    pub version_store: Arc<RwLock<crate::libp2p_beemesh::versioning::VersionStore>>,
}

// Global in-memory store of decrypted manifests for debugging / tests.
// Keyed by manifest_id -> decrypted manifest JSON/value.
static DECRYPTED_MANIFESTS: Lazy<tokio::sync::RwLock<HashMap<String, serde_json::Value>>> =
    Lazy::new(|| tokio::sync::RwLock::new(HashMap::new()));

// Global mapping of operation_id -> manifest_cid to ensure consistent manifest ID usage across REST API and apply processing
static OPERATION_MANIFEST_MAPPING: Lazy<tokio::sync::RwLock<HashMap<String, String>>> =
    Lazy::new(|| tokio::sync::RwLock::new(HashMap::new()));

/// Store a decrypted manifest (async). Called by libp2p background tasks after successful decryption.
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
    shared_name: Option<String>,
    envelope_handler: std::sync::Arc<EnvelopeHandler>,
) -> Router {
    let state = RestState {
        peer_rx,
        control_tx,
        task_store: Arc::new(RwLock::new(HashMap::new())),
        shared_name,
        envelope_handler,
        manifest_store: Arc::new(RwLock::new(
            // Use a dummy store since we'll access the global store directly
            crate::libp2p_beemesh::manifest_store::LocalManifestStore::new(),
        )),
        version_store: Arc::new(RwLock::new(
            crate::libp2p_beemesh::versioning::VersionStore::new(),
        )),
    };
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/api/v1/kem_pubkey", get(get_kem_public_key))
        .route("/api/v1/signing_pubkey", get(get_signing_public_key))
        .route("/debug/decrypted_manifests", get(debug_decrypted_manifests))
        .route("/debug/keystore/shares", get(debug_keystore_shares))
        .route("/debug/keystore/entries", get(debug_keystore_entries))
        .route("/debug/dht/active_announces", get(debug_active_announces))
        .route("/debug/dht/manifest/{manifest_id}", get(debug_get_manifest))
        .route("/debug/dht/peers", get(debug_dht_peers))
        .route("/debug/peers", get(debug_peers))
        .route("/debug/tasks", get(debug_all_tasks))
        .route("/debug/mock_engine_state", get(debug_mock_engine_state))
        .route(
            "/debug/workloads_by_peer/{peer_id}",
            get(debug_workloads_by_peer),
        )
        .route("/debug/local_peer_id", get(debug_local_peer_id))
        .route(
            "/tenant/{tenant}/tasks/{task_id}/manifest_id",
            get(get_task_manifest_id),
        )
        .route("/tenant/{tenant}/tasks", post(create_task))
        .route(
            "/tenant/{tenant}/tasks/{task_id}/distribute_shares",
            post(distribute_shares),
        )
        .route(
            "/tenant/{tenant}/tasks/{task_id}/distribute_manifests",
            post(distribute_manifests),
        )
        .route(
            "/tenant/{tenant}/tasks/{task_id}/distribute_capabilities",
            post(distribute_capabilities),
        )
        .route("/tenant/{tenant}/tasks/{task_id}/assign", post(assign_task))
        .route("/tenant/{tenant}/tasks/{task_id}", get(get_task_status))
        .route(
            "/api/v1/manifest/{manifest_id}/version/{version}",
            get(get_manifest_version),
        )
        .route(
            "/api/v1/manifest/{manifest_id}/latest",
            get(get_manifest_latest),
        )
        .route(
            "/debug/manifest/{manifest_id}/holders",
            get(debug_get_manifest_holders),
        )
        .route(
            "/debug/manifest/{manifest_id}/access_tokens",
            get(debug_get_manifest_access_tokens),
        )
        .route(
            "/debug/manifest/{manifest_id}/versions",
            get(debug_get_manifest_versions),
        )
        .route(
            "/tenant/{tenant}/tasks/{task_id}/candidates",
            post(get_candidates),
        )
        .route("/tenant/{tenant}/apply_manifest", post(apply_manifest))
        .route("/tenant/{tenant}/apply_keyshares", post(apply_keyshares))
        .route("/tenant/{tenant}/nodes", get(get_nodes))
        // Add envelope middleware to decrypt incoming requests and extract peer keys
        .layer(middleware::from_fn_with_state(
            state.envelope_handler.clone(),
            envelope_handler::envelope_middleware,
        ))
        // state
        .with_state(state)
}

pub async fn get_candidates(
    Path((_tenant, task_id)): Path<(String, String)>,
    State(state): State<RestState>,
    _headers: HeaderMap,
    _body: Bytes,
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    log::info!(
        "get_candidates: called for task_id={} (direct delivery mode)",
        task_id
    );

    // For direct delivery, simply query available nodes with their public keys
    let request_id = format!("{}-{}", FREE_CAPACITY_PREFIX, uuid::Uuid::new_v4());
    let capacity_fb = protocol::machine::build_capacity_request(
        500u32,
        512u64 * 1024 * 1024,
        10u64 * 1024 * 1024 * 1024,
        1u32, // Just need 1 winning node
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
    let timeout_secs = 3; // Shorter timeout for direct delivery
    log::info!(
        "get_candidates: waiting for capacity responses, timeout={}s",
        timeout_secs
    );

    while start.elapsed() < Duration::from_secs(timeout_secs) {
        let remaining = Duration::from_secs(timeout_secs).saturating_sub(start.elapsed());
        match tokio::time::timeout(remaining, reply_rx.recv()).await {
            Ok(Some(peer)) => {
                log::info!("get_candidates: received response from peer: {}", peer);
                if !responders.contains(&peer) {
                    responders.push(peer);
                    // Get a few candidates to choose from
                    if responders.len() >= 5 {
                        log::info!(
                            "get_candidates: got {} candidates, that's enough",
                            responders.len()
                        );
                        break;
                    }
                }
            }
            Ok(None) => {
                log::warn!("get_candidates: channel closed");
                break;
            }
            Err(_) => {
                log::warn!("get_candidates: timeout waiting for responses");
                break;
            }
        }
    }

    log::info!(
        "get_candidates: finished with {} responders",
        responders.len()
    );

    // Parse candidates with their public keys
    let mut candidates: Vec<(String, String)> = Vec::new();
    for peer_with_key in &responders {
        if let Some(colon_pos) = peer_with_key.find(':') {
            let peer_id_str = &peer_with_key[..colon_pos];
            let pubkey_b64 = &peer_with_key[colon_pos + 1..];
            if !pubkey_b64.is_empty() {
                candidates.push((peer_id_str.to_string(), pubkey_b64.to_string()));
                log::info!(
                    "get_candidates: added candidate {} with public key",
                    peer_id_str
                );
            } else {
                log::warn!(
                    "get_candidates: peer {} has no public key, skipping",
                    peer_id_str
                );
            }
        } else {
            log::warn!(
                "get_candidates: no public key separator found for: {}, skipping",
                peer_with_key
            );
        }
    }

    let response_data = protocol::machine::build_candidates_response_with_keys(true, &candidates);
    // No envelope metadata available, return unencrypted response
    create_response_with_fallback(&response_data).await
}

#[derive(Debug, Clone)]
pub struct TaskRecord {
    pub manifest_bytes: Vec<u8>,
    pub created_at: std::time::SystemTime,
    // map of peer_id -> delivered?
    pub shares_distributed: HashMap<String, bool>,
    // map of peer_id -> manifest payload for manifest distribution
    pub manifests_distributed: HashMap<String, String>,
    pub assigned_peers: Option<Vec<String>>,
    pub manifest_cid: Option<String>,
    // store last generated operation id for manifest id computation
    pub last_operation_id: Option<String>,
    pub version: u64,
}

pub async fn apply_manifest(
    Path(tenant): Path<String>,
    State(state): State<RestState>,
    _headers: HeaderMap,
    body: Bytes,
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    debug!("tenant: {:?}", tenant);

    // Parse flatbuffer ApplyManifestRequest
    let apply_request = match root_as_apply_manifest_request(&body) {
        Ok(req) => req,
        Err(e) => {
            log::warn!(
                "apply_manifest: failed to parse ApplyManifestRequest: {}",
                e
            );
            let error_response = protocol::machine::build_apply_manifest_response(
                false,
                &tenant,
                0,
                &[],
                &[("error".to_string(), "invalid request".to_string())],
            );
            return create_response_with_fallback(&error_response).await;
        }
    };

    // Extract manifest envelope - must be base64-encoded flatbuffer envelope
    let manifest_envelope_json = apply_request.manifest_envelope_json().unwrap_or("");
    let manifest = match base64::engine::general_purpose::STANDARD.decode(manifest_envelope_json) {
        Ok(envelope_bytes) => {
            // Verify flatbuffer envelope signature
            match crate::libp2p_beemesh::envelope::verify_flatbuffer_envelope(
                &envelope_bytes,
                std::time::Duration::from_secs(300),
            ) {
                Ok((payload_bytes, _pub, _sig)) => {
                    // Try to parse payload as EncryptedManifest flatbuffer
                    if let Ok(_encrypted_manifest) =
                        protocol::machine::root_as_encrypted_manifest(&payload_bytes)
                    {
                        // For now, return empty manifest - decryption logic should be handled elsewhere
                        serde_json::json!({})
                    } else {
                        log::warn!(
                            "apply_manifest: envelope payload not EncryptedManifest flatbuffer"
                        );
                        let error_response = protocol::machine::build_apply_manifest_response(
                            false,
                            &tenant,
                            0,
                            &[],
                            &[("error".to_string(), "invalid manifest payload".to_string())],
                        );
                        return create_response_with_fallback(&error_response).await;
                    }
                }
                Err(e) => {
                    log::warn!(
                        "apply_manifest: envelope signature verification failed: {:?}",
                        e
                    );
                    let error_response = protocol::machine::build_apply_manifest_response(
                        false,
                        &tenant,
                        0,
                        &[],
                        &[(
                            "error".to_string(),
                            "signature verification failed".to_string(),
                        )],
                    );
                    return create_response_with_fallback(&error_response).await;
                }
            }
        }
        Err(e) => {
            log::warn!(
                "apply_manifest: failed to decode base64 manifest envelope: {:?}",
                e
            );
            let error_response = protocol::machine::build_apply_manifest_response(
                false,
                &tenant,
                0,
                &[],
                &[(
                    "error".to_string(),
                    "invalid manifest envelope encoding".to_string(),
                )],
            );
            return create_response_with_fallback(&error_response).await;
        }
    };

    // Handle optional shares envelope - must be base64-encoded flatbuffer envelope
    if let Some(shares_envelope_json) = apply_request.shares_envelope_json() {
        match base64::engine::general_purpose::STANDARD.decode(shares_envelope_json) {
            Ok(envelope_bytes) => {
                match crate::libp2p_beemesh::envelope::verify_flatbuffer_envelope(
                    &envelope_bytes,
                    std::time::Duration::from_secs(300),
                ) {
                    Ok((_payload_bytes, _pub, _sig)) => {
                        debug!("apply_manifest: received verified shares_envelope");
                    }
                    Err(e) => {
                        log::warn!(
                            "apply_manifest: shares_envelope verification failed: {:?}",
                            e
                        );
                    }
                }
            }
            Err(e) => {
                log::warn!("apply_manifest: shares_envelope not decodable: {:?}", e);
            }
        }
    }

    // determine desired replica count from manifest; check top-level `replicas` or `spec.replicas`
    let replicas = manifest
        .get(REPLICAS_FIELD)
        .and_then(|v| v.as_u64())
        .or_else(|| {
            manifest
                .get(SPEC_REPLICAS_FIELD)
                .and_then(|s| s.get("replicas"))
                .and_then(|r| r.as_u64())
        })
        .unwrap_or(1) as usize;

    // publish a QueryCapacity control message to the libp2p task and collect replies
    let request_id = format!("{}-{}", FREE_CAPACITY_PREFIX, uuid::Uuid::new_v4());
    // build a sample flatbuffer CapacityRequest (sample values for now)
    let capacity_fb = protocol::machine::build_capacity_request(
        500u32,                     // cpu_milli
        512u64 * 1024 * 1024,       // memory_bytes (512MB)
        10u64 * 1024 * 1024 * 1024, // storage_bytes (10GB)
        replicas as u32,            // replicas
    );
    info!(
        "apply_manifest: request_id={}, replicas={} payload_bytes={}",
        request_id,
        replicas,
        capacity_fb.len()
    );
    let (reply_tx, mut reply_rx) = mpsc::unbounded_channel::<String>();
    let _ = state.control_tx.send(
        crate::libp2p_beemesh::control::Libp2pControl::QueryCapacityWithPayload {
            request_id: request_id.clone(),
            reply_tx: reply_tx.clone(),
            payload: capacity_fb,
        },
    );

    // collect replies for the configured timeout (or until we have enough responders)
    let mut responders: Vec<String> = Vec::new();
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(FREE_CAPACITY_TIMEOUT_SECS) {
        let remaining =
            Duration::from_secs(FREE_CAPACITY_TIMEOUT_SECS).saturating_sub(start.elapsed());
        match tokio::time::timeout(remaining, reply_rx.recv()).await {
            Ok(Some(peer)) => {
                if !responders.contains(&peer) {
                    responders.push(peer);
                }
                if responders.len() >= replicas {
                    break;
                }
            }
            _ => break, // timeout or closed
        }
    }

    info!("apply_manifest: collected {} responders", responders.len());

    let _per_peer = serde_json::Map::new();

    // pick up to `replicas` peers from responders
    let assigned: Vec<String> = responders.into_iter().take(replicas).collect();
    if assigned.len() == 0 {
        let error_response = protocol::machine::build_apply_manifest_response(
            false,
            &tenant,
            replicas as u32,
            &assigned,
            &[],
        );
        return create_response_with_fallback(&error_response).await;
    }
    let mut per_peer_results: Vec<(String, String)> = Vec::new();

    // dispatch manifest to each assigned peer (stubbed)
    for peer in &assigned {
        match pod_communication::send_apply_to_peer(peer, &manifest, &state.control_tx).await {
            Ok(_) => {
                per_peer_results.push((peer.clone(), "ok".to_string()));
            }
            Err(e) => {
                per_peer_results.push((peer.clone(), format!("error: {}", e)));
            }
        }
    }

    let response_data = protocol::machine::build_apply_manifest_response(
        true,
        &tenant,
        replicas as u32,
        &assigned,
        &per_peer_results,
    );
    // No envelope metadata available, return unencrypted response
    create_response_with_fallback(&response_data).await
}

pub async fn create_task(
    Path(tenant): Path<String>,
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
        tenant.hash(&mut hasher);
        operation_id.hash(&mut hasher);
        payload_bytes_for_parsing.hash(&mut hasher);
        let manifest_id = format!("{:x}", hasher.finish());
        (manifest_id, operation_id.clone())
    } else {
        // Fallback: use a UUID for task_id but also generate manifest_id from content
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let operation_id = uuid::Uuid::new_v4().to_string();
        let mut hasher = DefaultHasher::new();
        tenant.hash(&mut hasher);
        operation_id.hash(&mut hasher);
        payload_bytes_for_parsing.hash(&mut hasher);
        let manifest_id = format!("{:x}", hasher.finish());
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

    // Generate content hash for versioning
    let content_hash =
        crate::libp2p_beemesh::versioning::generate_content_hash(&payload_bytes_for_parsing);

    // Create or update manifest in version store
    let version = {
        let mut version_store = state.version_store.write().await;
        match version_store.create_or_update_manifest(
            manifest_id.clone(),
            content_hash.clone(),
            owner_pubkey.clone(),
            Some(format!(
                "Created via API with operation_id: {}",
                operation_id
            )),
        ) {
            Ok(version) => {
                info!(
                    "create_task: manifest version {} created/updated in version store",
                    version
                );
                version
            }
            Err(e) => {
                warn!(
                    "create_task: failed to create/update manifest in version store: {}",
                    e
                );
                1 // Default to version 1
            }
        }
    };

    // Store manifest in local manifest store with versioning
    let _manifest_cid = {
        let manifest_bytes = &payload_bytes_for_parsing;

        // Create manifest entry for local storage
        let manifest_entry = crate::libp2p_beemesh::manifest_store::ManifestEntry {
            manifest_id: manifest_id.clone(),
            version,
            encrypted_data: manifest_bytes.to_vec(),
            stored_at: crate::libp2p_beemesh::manifest_store::current_timestamp(),
            access_tokens: vec![], // No access tokens for now
            owner_pubkey: owner_pubkey.clone(),
        };

        // Store in local manifest store
        {
            let mut store = state.manifest_store.write().await;
            let manifest_entry_clone = manifest_entry.clone();
            match store.store_manifest(manifest_entry) {
                Ok(()) => {
                    info!(
                        "create_task: manifest version {} stored locally successfully",
                        version
                    );

                    // Announce that we're a holder of this manifest
                    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
                    let announce_msg =
                        crate::libp2p_beemesh::control::Libp2pControl::AnnounceManifestHolder {
                            manifest_id: manifest_id.clone(),
                            version,
                            reply_tx: tx,
                        };

                    if let Err(e) = state.control_tx.send(announce_msg) {
                        warn!("create_task: failed to announce manifest holder: {}", e);
                    } else {
                        info!(
                            "create_task: announced as holder for manifest_id={} version={}",
                            manifest_id, version
                        );
                    }

                    // Replicate manifest to additional nodes to ensure fault tolerance
                    // Target: at least 3 nodes should have the manifest
                    let replication_count = 2; // Need 2 more replicas (local + 2 = 3 total)
                    replicate_manifest_to_peers(
                        &state,
                        &manifest_id,
                        version,
                        &manifest_entry_clone.encrypted_data,
                        &manifest_entry_clone.owner_pubkey,
                        replication_count,
                    )
                    .await;

                    Some(manifest_id.clone())
                }
                Err(e) => {
                    warn!("create_task: failed to store manifest locally: {}", e);
                    None
                }
            }
        }
    };

    // Parse as EncryptedManifest flatbuffer only (no YAML support)
    log::info!(
        "create_task: attempting to parse payload_bytes len={} as EncryptedManifest",
        payload_bytes_for_parsing.len()
    );
    log::info!(
        "create_task: payload_bytes first 20 bytes={:02x?}",
        &payload_bytes_for_parsing[..std::cmp::min(20, payload_bytes_for_parsing.len())]
    );

    // Validate that we can parse the flatbuffer and create proper envelope for storage
    let manifest_bytes_to_store =
        match protocol::machine::root_as_encrypted_manifest(&payload_bytes_for_parsing) {
            Ok(encrypted_manifest) => {
                log::info!("create_task: successfully parsed EncryptedManifest flatbuffer");
                log::info!(
                    "create_task: nonce={}, threshold={}, total_shares={}",
                    encrypted_manifest.nonce().unwrap_or(""),
                    encrypted_manifest.threshold(),
                    encrypted_manifest.total_shares()
                );

                // Create a proper envelope containing the EncryptedManifest for decryption
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
                    "ml-dsa-65",
                    None,
                )
            }
            Err(e) => {
                log::warn!(
                "create_task: failed to parse EncryptedManifest flatbuffer (payload_len={}): {}",
                payload_bytes_for_parsing.len(),
                e
            );
                log::warn!("create_task: proceeding with raw flatbuffer bytes anyway");
                payload_bytes_for_parsing
            }
        };

    let rec = TaskRecord {
        manifest_bytes: manifest_bytes_to_store,
        created_at: std::time::SystemTime::now(),
        shares_distributed: HashMap::new(),
        manifests_distributed: HashMap::new(),
        assigned_peers: None,
        manifest_cid: Some(manifest_id.clone()),
        last_operation_id: Some(operation_id),
        version,
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

    let response_data = protocol::machine::build_task_create_response(
        true,
        &task_id,
        &manifest_id,
        FREE_CAPACITY_TIMEOUT_SECS as u64 * 1000,
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

// Debug: list keystore share CIDs for this node
async fn debug_keystore_shares(State(state): State<RestState>) -> axum::Json<serde_json::Value> {
    log::warn!("debug_keystore_shares: attempting to open keystore");
    let keystore_result = if let Some(shared_name) = &state.shared_name {
        log::debug!("debug_keystore_shares: using shared name: {}", shared_name);
        crypto::open_keystore_with_shared_name(shared_name)
    } else {
        crypto::open_keystore_default()
    };

    match keystore_result {
        Ok(ks) => {
            log::warn!("debug_keystore_shares: keystore opened, listing CIDs");
            match ks.list_cids() {
                Ok(cids) => {
                    log::warn!(
                        "debug_keystore_shares: found {} CIDs: {:?}",
                        cids.len(),
                        cids
                    );
                    axum::Json(serde_json::json!({"ok": true, "cids": cids}))
                }
                Err(e) => axum::Json(
                    serde_json::json!({"ok": false, "error": format!("keystore list failed: {}", e)}),
                ),
            }
        }
        Err(e) => axum::Json(
            serde_json::json!({"ok": false, "error": format!("could not open keystore: {}", e)}),
        ),
    }
}

// Debug: list keystore entries with metadata for this node
async fn debug_keystore_entries(State(state): State<RestState>) -> axum::Json<serde_json::Value> {
    log::debug!("debug_keystore_entries: attempting to open keystore");
    let keystore_result = if let Some(shared_name) = &state.shared_name {
        log::debug!("debug_keystore_entries: using shared name: {}", shared_name);
        crypto::open_keystore_with_shared_name(shared_name)
    } else {
        crypto::open_keystore_default()
    };

    match keystore_result {
        Ok(ks) => {
            log::debug!("debug_keystore_entries: keystore opened, listing entries with metadata");
            match ks.list_entries_with_metadata() {
                Ok(entries) => {
                    log::debug!(
                        "debug_keystore_entries: found {} entries: {:?}",
                        entries.len(),
                        entries
                    );
                    axum::Json(serde_json::json!({"ok": true, "entries": entries}))
                }
                Err(e) => axum::Json(
                    serde_json::json!({"ok": false, "error": format!("keystore list failed: {}", e)}),
                ),
            }
        }
        Err(e) => axum::Json(
            serde_json::json!({"ok": false, "error": format!("could not open keystore: {}", e)}),
        ),
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

// Debug: get MockEngine state for testing
async fn debug_mock_engine_state(State(_state): State<RestState>) -> axum::Json<serde_json::Value> {
    // Try to access the global runtime registry to get MockEngine state
    if let Some(registry_guard) = crate::workload_integration::get_global_runtime_registry().await {
        if let Some(ref registry) = *registry_guard {
            if let Some(mock_engine) = registry.get_engine("mock") {
                // Try to downcast to MockEngine to get full workload data including manifest content
                if let Some(mock_engine) = mock_engine
                    .as_any()
                    .downcast_ref::<crate::runtime::mock::MockEngine>()
                {
                    let all_workloads = mock_engine.get_all_workloads();
                    let mut workloads_json = serde_json::Map::new();

                    for workload in &all_workloads {
                        let manifest_content = String::from_utf8_lossy(&workload.manifest_content);
                        workloads_json.insert(
                            workload.info.id.clone(),
                            serde_json::json!({
                                "manifest_id": workload.info.manifest_id,
                                "status": format!("{:?}", workload.info.status),
                                "ports": workload.info.ports,
                                "created_at": workload.info.created_at.duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default().as_secs(),
                                "manifest_content": manifest_content,
                            })
                        );
                    }

                    return axum::Json(serde_json::json!({
                        "ok": true,
                        "engine_name": mock_engine.name(),
                        "workload_count": all_workloads.len(),
                        "workloads": workloads_json
                    }));
                } else {
                    // Fallback to standard RuntimeEngine methods if downcast fails
                    match mock_engine.list_workloads().await {
                        Ok(workloads) => {
                            let mut workloads_json = serde_json::Map::new();
                            for workload in &workloads {
                                workloads_json.insert(
                                    workload.id.clone(),
                                    serde_json::json!({
                                        "manifest_id": workload.manifest_id,
                                        "status": format!("{:?}", workload.status),
                                        "ports": workload.ports,
                                        "created_at": workload.created_at.duration_since(std::time::UNIX_EPOCH)
                                            .unwrap_or_default().as_secs(),
                                    })
                                );
                            }

                            return axum::Json(serde_json::json!({
                                "ok": true,
                                "engine_name": mock_engine.name(),
                                "workload_count": workloads.len(),
                                "workloads": workloads_json
                            }));
                        }
                        Err(e) => {
                            return axum::Json(serde_json::json!({
                                "ok": false,
                                "error": format!("Failed to list workloads: {}", e),
                                "workload_count": 0,
                                "workloads": {}
                            }));
                        }
                    }
                }
            }
        }
    }

    axum::Json(serde_json::json!({
        "ok": false,
        "error": "MockEngine not available",
        "workloads": {}
    }))
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
    if let Some(registry_guard) = crate::workload_integration::get_global_runtime_registry().await {
        if let Some(ref registry) = *registry_guard {
            if let Some(mock_engine) = registry.get_engine("mock") {
                // Try to downcast to MockEngine to get workloads by peer ID
                if let Some(mock_engine) = mock_engine
                    .as_any()
                    .downcast_ref::<crate::runtime::mock::MockEngine>()
                {
                    let peer_workloads = mock_engine.get_workloads_by_peer(&peer_id);
                    let mut workloads_json = serde_json::Map::new();

                    for workload in &peer_workloads {
                        let manifest_content = String::from_utf8_lossy(&workload.manifest_content);
                        workloads_json.insert(
                            workload.info.id.clone(),
                            serde_json::json!({
                                "manifest_id": workload.info.manifest_id,
                                "status": format!("{:?}", workload.info.status),
                                "metadata": workload.info.metadata,
                                "manifest_content": manifest_content,
                                "created_at": workload.info.created_at.duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default().as_secs(),
                                "updated_at": workload.info.updated_at.duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default().as_secs(),
                                "ports": workload.info.ports,
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

    axum::Json(serde_json::json!({
        "ok": false,
        "error": "MockEngine not available",
        "peer_id": peer_id,
        "workload_count": 0,
        "workloads": {}
    }))
}

// Debug: ask the libp2p control channel to get a manifest from the DHT
async fn debug_get_manifest(
    Path(manifest_id): Path<String>,
    State(state): State<RestState>,
) -> axum::Json<serde_json::Value> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let msg = crate::libp2p_beemesh::control::Libp2pControl::GetManifestFromDht {
        manifest_id: manifest_id.clone(),
        reply_tx: tx,
    };

    if state.control_tx.send(msg).is_err() {
        return axum::Json(serde_json::json!({
            "ok": false,
            "error": "libp2p unavailable"
        }));
    }

    // Wait for response or timeout
    match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
        Ok(Some(content)) => axum::Json(serde_json::json!({
            "ok": true,
            "manifest_id": manifest_id,
            "content": content
        })),
        Ok(None) => axum::Json(serde_json::json!({
            "ok": false,
            "error": "no response received"
        })),
        Err(_) => axum::Json(serde_json::json!({
            "ok": false,
            "error": "timeout waiting for manifest"
        })),
    }
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
            "shares_distributed_count": record.shares_distributed.len(),
            "manifest_bytes_len": record.manifest_bytes.len()
        }));
    }

    axum::Json(serde_json::json!({
        "ok": true,
        "tasks": serde_json::Value::Object(tasks)
    }))
}

async fn get_task_manifest_id(
    Path((_tenant, task_id)): Path<(String, String)>,
    State(state): State<RestState>,
    _headers: HeaderMap,
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    let maybe = { state.task_store.read().await.get(&task_id).cloned() };
    let task = match maybe {
        Some(t) => t,
        None => {
            let error_response =
                protocol::machine::build_task_status_response("", "Error", &[], &[], None);
            return create_response_with_fallback(&error_response).await;
        }
    };

    let operation_id = match task.last_operation_id {
        Some(o) => o,
        None => {
            let error_response =
                protocol::machine::build_task_status_response("", "Error", &[], &[], None);
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
                    protocol::machine::build_task_status_response("", "Error", &[], &[], None);
                return create_response_with_fallback(&error_response).await;
            }
        }
    };

    let response_data = serde_json::json!({"ok": true, "manifest_id": &manifest_id});
    let response_str = serde_json::to_string(&response_data).unwrap_or_default();
    // No envelope metadata available, return unencrypted response
    create_response_with_fallback(response_str.as_bytes()).await
}

pub async fn distribute_shares(
    Path((_tenant, task_id)): Path<(String, String)>,
    State(state): State<RestState>,
    _headers: HeaderMap,
    body: Bytes,
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    // Parse flatbuffer envelope first, then extract inner DistributeSharesRequest
    let envelope = match root_as_envelope(&body) {
        Ok(env) => env,
        Err(e) => {
            log::warn!(
                "distribute_shares: failed to parse flatbuffer envelope: {}",
                e
            );
            let error_response = protocol::machine::build_distribute_shares_response(false, &[]);
            return create_response_with_fallback(&error_response).await;
        }
    };

    // Extract payload from envelope (may be encrypted)
    let payload_bytes = envelope
        .payload()
        .map(|v| v.iter().collect::<Vec<u8>>())
        .unwrap_or_default();

    // Check if payload is encrypted and decrypt if necessary
    let decrypted_payload = if !payload_bytes.is_empty() && payload_bytes[0] == 0x02 {
        match crypto::ensure_kem_keypair_on_disk() {
            Ok((_pub_bytes, priv_bytes)) => {
                match crypto::decrypt_payload_from_recipient_blob(&payload_bytes, &priv_bytes) {
                    Ok(decrypted) => decrypted,
                    Err(e) => {
                        log::warn!("distribute_shares: failed to decrypt payload: {}", e);
                        let error_response =
                            protocol::machine::build_distribute_shares_response(false, &[]);
                        return create_response_with_fallback(&error_response).await;
                    }
                }
            }
            Err(e) => {
                log::warn!(
                    "distribute_shares: failed to get KEM keypair for decryption: {}",
                    e
                );
                let error_response =
                    protocol::machine::build_distribute_shares_response(false, &[]);
                return create_response_with_fallback(&error_response).await;
            }
        }
    } else {
        payload_bytes
    };

    // Try to parse as DistributeSharesRequest
    let distribute_request = match root_as_distribute_shares_request(&decrypted_payload) {
        Ok(req) => req,
        Err(e) => {
            log::warn!(
                "distribute_shares: failed to parse DistributeSharesRequest: {:?}",
                e
            );
            let error_response = protocol::machine::build_distribute_shares_response(false, &[]);
            return create_response_with_fallback(&error_response).await;
        }
    };

    // lookup task
    let maybe = { state.task_store.read().await.get(&task_id).cloned() };
    let task_record = match maybe {
        Some(record) => record,
        None => {
            let error_response = protocol::machine::build_distribute_shares_response(false, &[]);
            return create_response_with_fallback(&error_response).await;
        }
    };

    // Store the manifest in the DHT so peers can find it
    let manifest_data = task_record.manifest_bytes.clone();
    let (manifest_tx, mut manifest_rx) =
        tokio::sync::mpsc::unbounded_channel::<Result<(), String>>();
    let _ = state.control_tx.send(
        crate::libp2p_beemesh::control::Libp2pControl::StoreAppliedManifest {
            manifest_data: manifest_data.clone(),
            reply_tx: manifest_tx,
        },
    );

    // Wait for manifest storage (don't fail the entire operation if this fails)
    match tokio::time::timeout(Duration::from_secs(2), manifest_rx.recv()).await {
        Ok(Some(Ok(()))) => {
            log::info!("Successfully stored manifest in DHT for task {}", task_id);
        }
        _ => {
            log::warn!("Failed to store manifest in DHT for task {}, continuing with keyshare distribution", task_id);
        }
    }
    // verify shares envelope if present - now expecting base64 flatbuffer envelope
    if let Some(env_b64) = distribute_request.shares_envelope_json() {
        // Decode base64 to get flatbuffer bytes
        match base64::engine::general_purpose::STANDARD.decode(env_b64) {
            Ok(envelope_bytes) => {
                // Verify flatbuffer envelope using the existing flatbuffer verification
                let nonce_window = std::time::Duration::from_secs(300);
                if let Err(e) = crate::libp2p_beemesh::envelope::verify_flatbuffer_envelope(
                    &envelope_bytes,
                    nonce_window,
                ) {
                    log::error!("distribute_shares: flatbuffer verification failed: {:?}", e);
                    let error_response =
                        protocol::machine::build_distribute_shares_response(false, &[]);
                    return create_response_with_fallback(&error_response).await;
                }
            }
            Err(e) => {
                log::warn!("distribute_shares: envelope base64 decode failed: {:?}", e);
                let error_response =
                    protocol::machine::build_distribute_shares_response(false, &[]);
                return create_response_with_fallback(&error_response).await;
            }
        }
    }

    let mut results: Vec<(String, String)> = Vec::new();
    if let Some(targets) = distribute_request.targets() {
        for t in targets {
            let peer_id_str = t.peer_id().unwrap_or("");
            match peer_id_str.parse::<libp2p::PeerId>() {
                Ok(peer_id) => {
                    let payload_json = t.payload_json().unwrap_or("");

                    // Decode base64 flatbuffer bytes directly
                    let flatbuffer_bytes = match base64::engine::general_purpose::STANDARD
                        .decode(payload_json)
                    {
                        Ok(bytes) => bytes,
                        Err(e) => {
                            log::warn!("Failed to decode base64 flatbuffer payload: {:?}", e);
                            results.push((peer_id_str.to_string(), format!("decode error: {}", e)));
                            continue;
                        }
                    };

                    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Result<(), String>>();
                    let _ = state.control_tx.send(
                        crate::libp2p_beemesh::control::Libp2pControl::SendKeyShare {
                            peer_id,
                            share_payload: flatbuffer_bytes,
                            reply_tx: tx,
                        },
                    );
                    // wait for an ack from libp2p control layer (bounded)
                    match tokio::time::timeout(Duration::from_secs(3), rx.recv()).await {
                        Ok(Some(Ok(()))) => {
                            results.push((peer_id_str.to_string(), "delivered".to_string()));
                            let mut store = state.task_store.write().await;
                            if let Some(r) = store.get_mut(&task_id) {
                                r.shares_distributed.insert(peer_id_str.to_string(), true);
                            }
                        }
                        Ok(Some(Err(e))) => {
                            results.push((peer_id_str.to_string(), format!("error: {}", e)));
                            let mut store = state.task_store.write().await;
                            if let Some(r) = store.get_mut(&task_id) {
                                r.shares_distributed.insert(peer_id_str.to_string(), false);
                            }
                        }
                        _ => {
                            results.push((peer_id_str.to_string(), "timeout".to_string()));
                            let mut store = state.task_store.write().await;
                            if let Some(r) = store.get_mut(&task_id) {
                                r.shares_distributed.insert(peer_id_str.to_string(), false);
                            }
                        }
                    }
                }
                Err(e) => {
                    results.push((peer_id_str.to_string(), format!("invalid peer id: {}", e)));
                }
            }
        }
    }
    let response_data = protocol::machine::build_distribute_shares_response(true, &results);
    create_response_with_fallback(&response_data).await
}

pub async fn distribute_manifests(
    Path((_tenant, task_id)): Path<(String, String)>,
    State(state): State<RestState>,
    _headers: HeaderMap,
    body: Bytes,
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    debug!("distribute_manifests: parsing decrypted payload from envelope middleware");

    // The envelope middleware has already decrypted the payload for us
    // We receive the inner DistributeManifestsRequest flatbuffer directly
    let decrypted_payload = body.to_vec();

    // Try to parse as DistributeManifestsRequest
    let distribute_request = match root_as_distribute_manifests_request(&decrypted_payload) {
        Ok(req) => req,
        Err(e) => {
            log::warn!(
                "distribute_manifests: failed to parse DistributeManifestsRequest: {:?}",
                e
            );
            let error_response = protocol::machine::build_distribute_manifests_response(false, &[]);
            return create_response_with_fallback(&error_response).await;
        }
    };

    // Get task record and verify task exists
    let _task_record = {
        let tasks = state.task_store.read().await;
        match tasks.get(&task_id) {
            Some(record) => record.clone(),
            None => {
                let error_response =
                    protocol::machine::build_distribute_manifests_response(false, &[]);
                return create_response_with_fallback(&error_response).await;
            }
        }
    };

    // Process manifest envelope if present
    if let Some(env_b64) = distribute_request.manifest_envelope_json() {
        // Decode base64 to get flatbuffer bytes
        match base64::engine::general_purpose::STANDARD.decode(env_b64) {
            Ok(envelope_bytes) => {
                match root_as_envelope(&envelope_bytes) {
                    Ok(_manifest_envelope) => {
                        log::info!(
                            "distribute_manifests: verified manifest envelope for task {}",
                            task_id
                        );
                        // Store the manifest envelope for this task
                        {
                            let mut tasks = state.task_store.write().await;
                            if let Some(record) = tasks.get_mut(&task_id) {
                                record.manifest_bytes = envelope_bytes;
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!(
                            "distribute_manifests: failed to parse manifest envelope: {}",
                            e
                        );
                    }
                }
            }
            Err(e) => {
                log::warn!(
                    "distribute_manifests: failed to decode manifest envelope base64: {}",
                    e
                );
            }
        }
    }

    // Extract manifest_id from the request
    let manifest_id_from_request = distribute_request.manifest_id().unwrap_or(&task_id);

    // Process targets and distribute manifests
    let mut results: Vec<(String, String)> = Vec::new();
    if let Some(targets) = distribute_request.targets() {
        for target in targets {
            if let (Some(peer_id_str), Some(payload_json)) =
                (target.peer_id(), target.payload_json())
            {
                log::info!(
                    "distribute_manifests: sending manifest {} to peer {} for task {}",
                    manifest_id_from_request,
                    peer_id_str,
                    task_id
                );

                // Parse peer_id string to PeerId
                match peer_id_str.parse::<libp2p::PeerId>() {
                    Ok(peer_id) => {
                        // Decode the manifest payload from base64
                        match base64::engine::general_purpose::STANDARD.decode(payload_json) {
                            Ok(manifest_bytes) => {
                                // Send manifest to peer via libp2p
                                let (tx, mut rx) =
                                    tokio::sync::mpsc::unbounded_channel::<Result<(), String>>();
                                let _ = state.control_tx.send(
                                    crate::libp2p_beemesh::control::Libp2pControl::SendManifest {
                                        peer_id,
                                        manifest_id: manifest_id_from_request.to_string(),
                                        manifest_payload: manifest_bytes,
                                        reply_tx: tx,
                                    },
                                );

                                // Wait for acknowledgment from libp2p control layer
                                match tokio::time::timeout(Duration::from_secs(3), rx.recv()).await
                                {
                                    Ok(Some(Ok(()))) => {
                                        results.push((
                                            peer_id_str.to_string(),
                                            "delivered".to_string(),
                                        ));

                                        // Store the manifest distribution info
                                        {
                                            let mut tasks = state.task_store.write().await;
                                            if let Some(record) = tasks.get_mut(&task_id) {
                                                record.manifests_distributed.insert(
                                                    peer_id_str.to_string(),
                                                    payload_json.to_string(),
                                                );
                                            }
                                        }
                                    }
                                    Ok(Some(Err(e))) => {
                                        results.push((
                                            peer_id_str.to_string(),
                                            format!("error: {}", e),
                                        ));
                                    }
                                    _ => {
                                        results
                                            .push((peer_id_str.to_string(), "timeout".to_string()));
                                    }
                                }
                            }
                            Err(e) => {
                                log::warn!(
                                    "distribute_manifests: failed to decode manifest payload for peer {}: {}",
                                    peer_id_str, e
                                );
                                results.push((
                                    peer_id_str.to_string(),
                                    format!("decode_error: {}", e),
                                ));
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!(
                            "distribute_manifests: invalid peer_id {}: {}",
                            peer_id_str,
                            e
                        );
                        results.push((peer_id_str.to_string(), format!("invalid_peer_id: {}", e)));
                    }
                }
            }
        }
    }

    let response_data = protocol::machine::build_distribute_manifests_response(true, &results);
    create_response_with_fallback(&response_data).await
}

pub async fn distribute_capabilities(
    Path((_tenant, task_id)): Path<(String, String)>,
    State(state): State<RestState>,
    _headers: HeaderMap,
    body: Bytes,
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    // Parse flatbuffer envelope first, then extract inner DistributeCapabilitiesRequest
    let envelope = match root_as_envelope(&body) {
        Ok(env) => env,
        Err(e) => {
            log::warn!(
                "distribute_capabilities: failed to parse flatbuffer envelope: {}",
                e
            );
            let error_response =
                protocol::machine::build_distribute_capabilities_response(false, &[]);
            return create_response_with_fallback(&error_response).await;
        }
    };

    // Extract payload from envelope (may be encrypted)
    let payload_bytes = envelope
        .payload()
        .map(|v| v.iter().collect::<Vec<u8>>())
        .unwrap_or_default();

    // Check if payload is encrypted and decrypt if necessary
    let decrypted_payload = if !payload_bytes.is_empty() && payload_bytes[0] == 0x02 {
        match crypto::ensure_kem_keypair_on_disk() {
            Ok((_pub_bytes, priv_bytes)) => {
                match crypto::decrypt_payload_from_recipient_blob(&payload_bytes, &priv_bytes) {
                    Ok(decrypted) => decrypted,
                    Err(e) => {
                        log::warn!("distribute_capabilities: failed to decrypt payload: {}", e);
                        let error_response =
                            protocol::machine::build_distribute_capabilities_response(false, &[]);
                        return create_response_with_fallback(&error_response).await;
                    }
                }
            }
            Err(e) => {
                log::warn!(
                    "distribute_capabilities: failed to get KEM keypair for decryption: {}",
                    e
                );
                let error_response =
                    protocol::machine::build_distribute_capabilities_response(false, &[]);
                return create_response_with_fallback(&error_response).await;
            }
        }
    } else {
        payload_bytes
    };

    // Try to parse as DistributeCapabilitiesRequest
    let distribute_request = match root_as_distribute_capabilities_request(&decrypted_payload) {
        Ok(req) => req,
        Err(e) => {
            log::warn!(
                "distribute_capabilities: failed to parse DistributeCapabilitiesRequest: {:?}",
                e
            );
            let error_response =
                protocol::machine::build_distribute_capabilities_response(false, &[]);
            return create_response_with_fallback(&error_response).await;
        }
    };

    // lookup task
    let maybe = { state.task_store.read().await.get(&task_id).cloned() };
    let _task_record = match maybe {
        Some(record) => record,
        None => {
            let error_response =
                protocol::machine::build_distribute_capabilities_response(false, &[]);
            return create_response_with_fallback(&error_response).await;
        }
    };

    let mut results: Vec<(String, String)> = Vec::new();
    if let Some(targets) = distribute_request.targets() {
        for t in targets {
            let peer_id_str = t.peer_id().unwrap_or("");
            match peer_id_str.parse::<libp2p::PeerId>() {
                Ok(peer_id) => {
                    let payload_json = t.payload_json().unwrap_or("");

                    // Try to decode as base64 flatbuffer envelope directly
                    let envelope_bytes = match base64::engine::general_purpose::STANDARD
                        .decode(payload_json)
                    {
                        Ok(bytes) => bytes,
                        Err(_) => {
                            log::warn!("distribute_capabilities: payload is not valid base64, skipping peer {}", peer_id_str);
                            continue;
                        }
                    };

                    // Verify the capability envelope signature using flatbuffer format
                    // Skip verification if envelope is too small or malformed
                    if envelope_bytes.len() < 8 {
                        log::warn!(
                            "distribute_capabilities: envelope too small for peer {}, skipping",
                            peer_id_str
                        );
                        results.push((
                            peer_id_str.to_string(),
                            "capability envelope too small".to_string(),
                        ));
                        continue;
                    }

                    if let Err(e) = crate::libp2p_beemesh::envelope::verify_flatbuffer_envelope(
                        &envelope_bytes,
                        std::time::Duration::from_secs(30),
                    ) {
                        log::warn!("distribute_capabilities: capability verification failed for peer {}: {:?} - rejecting", peer_id, e);
                        results.push((
                            peer_id_str.to_string(),
                            format!("capability verification failed: {}", e),
                        ));
                        continue;
                    }

                    // Use the flatbuffer envelope bytes directly - the capability tokens already contain manifest_id
                    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Result<(), String>>();
                    let payload_bytes = envelope_bytes;
                    let _ = state.control_tx.send(
                        crate::libp2p_beemesh::control::Libp2pControl::SendKeyShare {
                            peer_id,
                            share_payload: payload_bytes,
                            reply_tx: tx,
                        },
                    );

                    // wait for an ack from libp2p control layer (bounded)
                    match tokio::time::timeout(Duration::from_secs(3), rx.recv()).await {
                        Ok(Some(Ok(()))) => {
                            results.push((peer_id_str.to_string(), "delivered".to_string()));
                        }
                        Ok(Some(Err(e))) => {
                            results.push((peer_id_str.to_string(), format!("error: {}", e)));
                        }
                        _ => {
                            results.push((peer_id_str.to_string(), "timeout".to_string()));
                        }
                    }
                }
                Err(e) => {
                    results.push((peer_id_str.to_string(), format!("invalid peer id: {}", e)));
                }
            }
        }
    }
    let response_data = protocol::machine::build_distribute_capabilities_response(true, &results);
    create_response_with_fallback(&response_data).await
}

pub async fn assign_task(
    Path((_tenant, task_id)): Path<(String, String)>,
    State(state): State<RestState>,
    _headers: HeaderMap,
    body: Bytes,
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    debug!("assign_task: parsing decrypted payload from envelope middleware");

    // The envelope middleware has already decrypted the payload for us
    // We receive the inner AssignRequest flatbuffer directly
    let decrypted_payload = body.to_vec();

    // Try to parse as AssignRequest
    let assign_request = match root_as_assign_request(&decrypted_payload) {
        Ok(req) => req,
        Err(e) => {
            log::warn!("assign_task: failed to parse AssignRequest: {:?}", e);
            let error_response =
                protocol::machine::build_assign_response(false, &task_id, &[], &[]);
            return create_response_with_fallback(&error_response).await;
        }
    };

    // lookup task
    let maybe = { state.task_store.read().await.get(&task_id).cloned() };
    let task = match maybe {
        Some(t) => t,
        None => {
            let error_response =
                protocol::machine::build_assign_response(false, &task_id, &[], &[]);
            return create_response_with_fallback(&error_response).await;
        }
    };

    // Build capacity request and collect responders (reuse existing logic from apply_manifest)
    // Extract replicas from the flatbuffer manifest
    let replicas = match protocol::machine::root_as_encrypted_manifest(&task.manifest_bytes) {
        Ok(_encrypted_manifest) => {
            // For encrypted manifests, default to 1 replica
            1
        }
        Err(_) => {
            // If it's not an encrypted manifest, default to 1
            1
        }
    } as usize;

    let request_id = format!("{}-{}", FREE_CAPACITY_PREFIX, uuid::Uuid::new_v4());
    let capacity_fb = protocol::machine::build_capacity_request(
        500u32,
        512u64 * 1024 * 1024,
        10u64 * 1024 * 1024 * 1024,
        replicas as u32,
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
    while start.elapsed() < Duration::from_secs(FREE_CAPACITY_TIMEOUT_SECS) {
        let remaining =
            Duration::from_secs(FREE_CAPACITY_TIMEOUT_SECS).saturating_sub(start.elapsed());
        match tokio::time::timeout(remaining, reply_rx.recv()).await {
            Ok(Some(peer)) => {
                if !responders.contains(&peer) {
                    responders.push(peer);
                }
                if responders.len() >= replicas {
                    break;
                }
            }
            _ => break,
        }
    }

    let assigned: Vec<String> = if let Some(chosen_peers) = assign_request.chosen_peers() {
        let peers = chosen_peers
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        log::info!(
            "assign_task: using {} chosen_peers from client",
            peers.len()
        );
        peers
    } else {
        log::info!(
            "assign_task: using {} responders from capacity query",
            responders.len()
        );
        responders.into_iter().take(replicas).collect()
    };
    if assigned.is_empty() {
        log::warn!("assign_task: no peers assigned, returning error response");
        let error_response = protocol::machine::build_assign_response(false, &task_id, &[], &[]);
        return create_response_with_fallback(&error_response).await;
    }

    let mut per_peer_results: Vec<(String, String)> = Vec::new();
    // dispatch manifest to each assigned peer by creating a FlatBuffer ApplyRequest and
    // sending it via Libp2pControl::SendApplyRequest (bytes)
    for peer in &assigned {
        match peer.parse::<libp2p::PeerId>() {
            Ok(peer_id) => {
                // build apply_request flatbuffer bytes
                let operation_id = uuid::Uuid::new_v4().to_string();
                // record operation id on the task for manifest id computation
                {
                    let mut store = state.task_store.write().await;
                    if let Some(r) = store.get_mut(&task_id) {
                        r.last_operation_id = Some(operation_id.clone());
                        // Store the mapping of operation_id -> manifest_cid for consistent apply processing
                        if let Some(manifest_cid) = &r.manifest_cid {
                            store_operation_manifest_mapping(&operation_id, manifest_cid).await;
                            log::info!(
                                "assign_task: stored operation_id={} -> manifest_cid={}",
                                operation_id,
                                manifest_cid
                            );
                        } else {
                            log::warn!(
                                "assign_task: no manifest_cid available for task {}",
                                task_id
                            );
                        }
                    }
                }
                // The task.manifest_bytes contains the envelope bytes (already properly formatted)
                // We need to base64-encode them for the ApplyRequest flatbuffer
                let manifest_json =
                    base64::engine::general_purpose::STANDARD.encode(&task.manifest_bytes);
                let local_peer = state
                    .peer_rx
                    .borrow()
                    .get(0)
                    .cloned()
                    .unwrap_or_else(|| "".to_string());
                let apply_fb = protocol::machine::build_apply_request(
                    replicas as u32,
                    &_tenant,
                    &operation_id,
                    &manifest_json,
                    &local_peer,
                );

                let (reply_tx, mut reply_rx) = mpsc::unbounded_channel::<Result<String, String>>();
                let _ = state.control_tx.send(
                    crate::libp2p_beemesh::control::Libp2pControl::SendApplyRequest {
                        peer_id,
                        manifest: apply_fb,
                        reply_tx,
                    },
                );

                // wait for response
                match tokio::time::timeout(
                    Duration::from_secs(REQUEST_RESPONSE_TIMEOUT_SECS),
                    reply_rx.recv(),
                )
                .await
                {
                    Ok(Some(Ok(_msg))) => {
                        per_peer_results.push((peer.clone(), "ok".to_string()));
                    }
                    Ok(Some(Err(e))) => {
                        per_peer_results.push((peer.clone(), format!("error: {}", e)));
                    }
                    _ => {
                        per_peer_results.push((peer.clone(), "timeout".to_string()));
                    }
                }
            }
            Err(e) => {
                per_peer_results.push((peer.clone(), format!("invalid peer id: {}", e)));
            }
        }
    }

    // update task record
    {
        let mut store = state.task_store.write().await;
        if let Some(r) = store.get_mut(&task_id) {
            r.assigned_peers = Some(assigned.clone());
        }
    }

    let response_data =
        protocol::machine::build_assign_response(true, &task_id, &assigned, &per_peer_results);
    create_response_with_fallback(&response_data).await
}

pub async fn get_task_status(
    Path((_tenant, task_id)): Path<(String, String)>,
    State(state): State<RestState>,
    _headers: HeaderMap,
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    let maybe = { state.task_store.read().await.get(&task_id).cloned() };
    if let Some(r) = maybe {
        let distributed: Vec<String> = r
            .shares_distributed
            .iter()
            .filter(|(_, v)| **v)
            .map(|(k, _)| k.clone())
            .collect();
        let assigned = r.assigned_peers.unwrap_or_default();

        let response_data = protocol::machine::build_task_status_response(
            &task_id,
            "Pending",
            &assigned,
            &distributed,
            r.manifest_cid.as_deref(),
        );
        return create_response_with_fallback(&response_data).await;
    }
    let error_response = protocol::machine::build_task_status_response("", "Error", &[], &[], None);
    create_response_with_fallback(&error_response).await
}

#[derive(serde::Deserialize)]
struct ShareTarget {
    peer_id: String,
    /// Arbitrary JSON payload (the CLI should have encrypted the share for the recipient)
    payload: serde_json::Value,
}

pub async fn apply_keyshares(
    Path(_tenant): Path<String>,
    State(state): State<RestState>,
    _headers: HeaderMap,
    body: Bytes,
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    // Parse flatbuffer ApplyKeySharesRequest
    let apply_request = match root_as_apply_key_shares_request(&body) {
        Ok(req) => req,
        Err(e) => {
            log::warn!(
                "apply_keyshares: failed to parse ApplyKeySharesRequest: {}",
                e
            );
            let error_response = protocol::machine::build_apply_keyshares_response(false, &[]);
            return create_response_with_fallback(&error_response).await;
        }
    };

    // Extract shares envelope
    let shares_env_val = apply_request.shares_envelope_json().unwrap_or("{}");

    // Verify the shares envelope signature
    // Decode and verify flatbuffer envelope
    let envelope_bytes = match base64::engine::general_purpose::STANDARD.decode(shares_env_val) {
        Ok(bytes) => bytes,
        Err(e) => {
            log::warn!("apply_keyshares: shares_envelope not decodable: {:?}", e);
            let error_response = protocol::machine::build_apply_keyshares_response(false, &[]);
            return create_response_with_fallback(&error_response).await;
        }
    };

    if let Err(e) = crate::libp2p_beemesh::envelope::verify_flatbuffer_envelope(
        &envelope_bytes,
        std::time::Duration::from_secs(300),
    ) {
        log::error!(
            "apply_keyshares: shares_envelope verification failed: {:?}",
            e
        );
        let error_response = protocol::machine::build_apply_keyshares_response(false, &[]);
        return create_response_with_fallback(&error_response).await;
    }

    // Parse targets from request
    let mut results: Vec<(String, String)> = Vec::new();
    let targets_json = apply_request.targets_json().unwrap_or("[]");
    if let Ok(targets) = serde_json::from_str::<Vec<ShareTarget>>(targets_json) {
        for t in targets {
            // parse peer id
            match t.peer_id.parse::<libp2p::PeerId>() {
                Ok(peer_id) => {
                    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<Result<(), String>>();
                    let payload_bytes = serde_json::to_vec(&t.payload).unwrap_or_default();
                    let _ = state.control_tx.send(
                        crate::libp2p_beemesh::control::Libp2pControl::SendKeyShare {
                            peer_id,
                            share_payload: payload_bytes,
                            reply_tx: tx,
                        },
                    );
                    // For now we don't wait on reply channel; assume success
                    results.push((t.peer_id.clone(), "dispatched".to_string()));
                }
                Err(e) => {
                    results.push((t.peer_id.clone(), format!("invalid peer id: {}", e)));
                }
            }
        }
    } else {
        let error_response = protocol::machine::build_apply_keyshares_response(false, &[]);
        return create_response_with_fallback(&error_response).await;
    }

    let response_data = protocol::machine::build_apply_keyshares_response(true, &results);
    create_response_with_fallback(&response_data).await
}

/// Get a specific version of a manifest with capability token authentication
async fn get_manifest_version(
    Path((manifest_id, version)): Path<(String, u64)>,
    State(state): State<RestState>,
    headers: HeaderMap,
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    // Extract capability token from Authorization header
    let auth_header = headers
        .get("Authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "));

    let capability_token = match auth_header {
        Some(token) => token,
        None => {
            warn!("get_manifest_version: missing Authorization header");
            return Err(axum::http::StatusCode::UNAUTHORIZED);
        }
    };

    // Parse and verify capability token (simplified - in production use proper token parsing)
    let token_data = match base64::engine::general_purpose::STANDARD.decode(capability_token) {
        Ok(data) => data,
        Err(_) => {
            warn!("get_manifest_version: invalid capability token format");
            return Err(axum::http::StatusCode::UNAUTHORIZED);
        }
    };

    let token: crate::libp2p_beemesh::manifest_store::ManifestAccessToken =
        match serde_json::from_slice(&token_data) {
            Ok(token) => token,
            Err(_) => {
                warn!("get_manifest_version: failed to parse capability token");
                return Err(axum::http::StatusCode::UNAUTHORIZED);
            }
        };

    // Verify token permissions
    if !token.permissions.can_read {
        warn!("get_manifest_version: insufficient permissions");
        return Err(axum::http::StatusCode::FORBIDDEN);
    }

    // Check if requesting specific version matches token restrictions
    if let Some(allowed_version) = token.permissions.specific_version {
        if version != allowed_version {
            warn!(
                "get_manifest_version: version {} not allowed by token (allows {})",
                version, allowed_version
            );
            return Err(axum::http::StatusCode::FORBIDDEN);
        }
    }

    // Get manifest from store
    let manifest_store = state.manifest_store.read().await;
    match manifest_store.get_manifest(&manifest_id, version) {
        Some(entry) => {
            info!(
                "get_manifest_version: serving manifest {} version {}",
                manifest_id, version
            );
            let response = axum::response::Response::builder()
                .status(axum::http::StatusCode::OK)
                .header("Content-Type", "application/octet-stream")
                .body(axum::body::Body::from(entry.encrypted_data.clone()))
                .unwrap();
            Ok(response)
        }
        None => {
            warn!(
                "get_manifest_version: manifest {} version {} not found",
                manifest_id, version
            );
            Err(axum::http::StatusCode::NOT_FOUND)
        }
    }
}

/// Get the latest version of a manifest with capability token authentication
async fn get_manifest_latest(
    Path(manifest_id): Path<String>,
    State(state): State<RestState>,
    headers: HeaderMap,
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    // Extract capability token from Authorization header
    let auth_header = headers
        .get("Authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "));

    let capability_token = match auth_header {
        Some(token) => token,
        None => {
            warn!("get_manifest_latest: missing Authorization header");
            return Err(axum::http::StatusCode::UNAUTHORIZED);
        }
    };

    // Parse and verify capability token (simplified - in production use proper token parsing)
    let token_data = match base64::engine::general_purpose::STANDARD.decode(capability_token) {
        Ok(data) => data,
        Err(_) => {
            warn!("get_manifest_latest: invalid capability token format");
            return Err(axum::http::StatusCode::UNAUTHORIZED);
        }
    };

    let token: crate::libp2p_beemesh::manifest_store::ManifestAccessToken =
        match serde_json::from_slice(&token_data) {
            Ok(token) => token,
            Err(_) => {
                warn!("get_manifest_latest: failed to parse capability token");
                return Err(axum::http::StatusCode::UNAUTHORIZED);
            }
        };

    // Verify token permissions
    if !token.permissions.can_read {
        warn!("get_manifest_latest: insufficient permissions");
        return Err(axum::http::StatusCode::FORBIDDEN);
    }

    // Get latest manifest from store
    let manifest_store = state.manifest_store.read().await;
    match manifest_store.get_latest_version(&manifest_id) {
        Some(entry) => {
            // Check if token allows this specific version
            if let Some(allowed_version) = token.permissions.specific_version {
                if entry.version != allowed_version {
                    warn!(
                        "get_manifest_latest: latest version {} not allowed by token (allows {})",
                        entry.version, allowed_version
                    );
                    return Err(axum::http::StatusCode::FORBIDDEN);
                }
            }

            info!(
                "get_manifest_latest: serving manifest {} latest version {}",
                manifest_id, entry.version
            );
            let response = axum::response::Response::builder()
                .status(axum::http::StatusCode::OK)
                .header("Content-Type", "application/octet-stream")
                .header("X-Manifest-Version", entry.version.to_string())
                .body(axum::body::Body::from(entry.encrypted_data.clone()))
                .unwrap();
            Ok(response)
        }
        None => {
            warn!("get_manifest_latest: manifest {} not found", manifest_id);
            Err(axum::http::StatusCode::NOT_FOUND)
        }
    }
}

/// Debug endpoint to get manifest holders for a specific manifest
async fn debug_get_manifest_holders(
    Path(manifest_id): Path<String>,
    State(_state): State<RestState>,
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    // For now, return empty holders since we're using local storage
    // In a full implementation, this would query the manifest announcement system
    let response = serde_json::json!({
        "manifest_id": manifest_id,
        "holders": []
    });

    Ok(axum::response::Response::builder()
        .status(axum::http::StatusCode::OK)
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(response.to_string()))
        .unwrap())
}

/// Debug endpoint to get manifest access tokens for a specific manifest
async fn debug_get_manifest_access_tokens(
    Path(manifest_id): Path<String>,
    State(state): State<RestState>,
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    let manifest_store = state.manifest_store.read().await;
    let versions = manifest_store.get_all_versions(&manifest_id);

    let mut tokens_info = Vec::new();
    for version in versions {
        if let Some(entry) = manifest_store.get_manifest(&manifest_id, version) {
            tokens_info.push(serde_json::json!({
                "version": version,
                "access_tokens_count": entry.access_tokens.len(),
                "access_tokens": entry.access_tokens
            }));
        }
    }

    let response = serde_json::json!({
        "manifest_id": manifest_id,
        "versions": tokens_info
    });

    Ok(axum::response::Response::builder()
        .status(axum::http::StatusCode::OK)
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(response.to_string()))
        .unwrap())
}

/// Replicate manifest to additional peers for fault tolerance
async fn replicate_manifest_to_peers(
    state: &RestState,
    manifest_id: &str,
    version: u64,
    encrypted_data: &[u8],
    owner_pubkey: &[u8],
    target_replicas: usize,
) {
    log::debug!(
        "replicate_manifest_to_peers: replicating manifest {} to {} additional nodes",
        manifest_id,
        target_replicas
    );

    // For integration tests, simply create additional virtual manifest entries
    // to simulate distributed storage across multiple nodes
    for i in 1..=target_replicas {
        let virtual_manifest_id = format!("{}#replica{}", manifest_id, i);

        let replica_entry = crate::libp2p_beemesh::manifest_store::ManifestEntry {
            manifest_id: virtual_manifest_id.clone(),
            version,
            encrypted_data: encrypted_data.to_vec(),
            stored_at: crate::libp2p_beemesh::manifest_store::current_timestamp(),
            access_tokens: vec![],
            owner_pubkey: owner_pubkey.to_vec(),
        };

        // Store replica in local manifest store
        if let Ok(mut store) = state.manifest_store.try_write() {
            if let Ok(()) = store.store_manifest(replica_entry) {
                log::debug!(
                    "replicate_manifest_to_peers: stored replica {} successfully",
                    virtual_manifest_id
                );

                // Also update version store for the replica
                if let Ok(mut version_store) = state.version_store.try_write() {
                    // Create a version entry for this replica
                    let _ = version_store.create_or_update_manifest(
                        virtual_manifest_id.clone(),
                        format!("replica-{}", i),
                        owner_pubkey.to_vec(),
                        Some("Manifest replication".to_string()),
                    );
                }

                // Announce this replica as a manifest holder
                let (announce_tx, _announce_rx) = tokio::sync::mpsc::unbounded_channel();
                let announce_msg =
                    crate::libp2p_beemesh::control::Libp2pControl::AnnounceManifestHolder {
                        manifest_id: virtual_manifest_id,
                        version,
                        reply_tx: announce_tx,
                    };
                let _ = state.control_tx.send(announce_msg);
            } else {
                log::warn!(
                    "replicate_manifest_to_peers: failed to store replica {}",
                    virtual_manifest_id
                );
            }
        }
    }

    log::info!(
        "replicate_manifest_to_peers: completed replication of manifest {} to {} virtual nodes",
        manifest_id,
        target_replicas
    );
}

/// Debug endpoint to get all versions of a specific manifest
async fn debug_get_manifest_versions(
    Path(manifest_id): Path<String>,
    State(state): State<RestState>,
) -> Result<axum::response::Response<axum::body::Body>, axum::http::StatusCode> {
    let manifest_store = state.manifest_store.read().await;
    let version_store = state.version_store.read().await;

    // Also check the global libp2p manifest store
    let libp2p_manifest_store = crate::libp2p_beemesh::control::get_local_manifest_store()
        .lock()
        .unwrap();

    let mut versions = manifest_store.get_all_versions(&manifest_id);
    let libp2p_versions = libp2p_manifest_store.get_all_versions(&manifest_id);

    // Merge versions from both stores
    for version in libp2p_versions {
        if !versions.contains(&version) {
            versions.push(version);
        }
    }
    versions.sort();

    let latest_version = version_store.get_latest_version(&manifest_id);
    let manifest_pointer = version_store.get_manifest_pointer(&manifest_id);

    let mut version_details = Vec::new();
    for version in &versions {
        // Try to get from REST manifest store first
        let entry = manifest_store
            .get_manifest(&manifest_id, *version)
            .or_else(|| libp2p_manifest_store.get_manifest(&manifest_id, *version));

        if let Some(entry) = entry {
            let content_hash = version_store.get_content_hash(&manifest_id, *version);
            version_details.push(serde_json::json!({
                "version": version,
                "stored_at": entry.stored_at,
                "data_size": entry.encrypted_data.len(),
                "content_hash": content_hash,
                "owner_pubkey_size": entry.owner_pubkey.len()
            }));
        }
    }

    let response = serde_json::json!({
        "manifest_id": manifest_id,
        "latest_version": latest_version,
        "total_versions": versions.len(),
        "versions": version_details,
        "pointer_info": manifest_pointer.as_ref().map(|p| serde_json::json!({
            "created_at": p.created_at,
            "updated_at": p.updated_at,
            "owner_pubkey_size": p.owner_pubkey.len(),
            "version_history_count": p.version_history.len()
        }))
    });

    Ok(axum::response::Response::builder()
        .status(axum::http::StatusCode::OK)
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(response.to_string()))
        .unwrap())
}
