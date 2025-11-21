use super::{KubeResourceRecord, RestState, TaskRecord};
use crate::crypto::encrypt_payload_for_recipient;
use crate::scheduler::{
    NodeCandidate, NodeCapabilities, Scheduler, SchedulerConfig, SchedulingStrategy,
};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
};
use base64::Engine;
use log::{error, warn};
use serde_json::{Map, Value, json};
use serde_yaml;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio::sync::mpsc;
use tokio::time::{Duration, timeout};
use uuid::Uuid;

const DEPLOYMENT_KIND: &str = "Deployment";
const APPS_V1: &str = "apps/v1";

pub fn core_router() -> Router<RestState> {
    Router::new()
        .route("/", get(api_versions))
        .route("/v1", get(core_v1_resources))
}

pub fn apps_v1_router() -> Router<RestState> {
    Router::new()
        .route("/", get(apps_v1_resources))
        .route(
            "/namespaces/{namespace}/deployments",
            get(list_deployments).post(create_deployment),
        )
        .route(
            "/namespaces/{namespace}/deployments/{name}",
            get(get_deployment)
                .patch(apply_deployment)
                .put(replace_deployment)
                .delete(delete_deployment),
        )
}

pub async fn api_group_list() -> Json<Value> {
    Json(json!({
        "kind": "APIGroupList",
        "apiVersion": "v1",
        "groups": [
            {
                "name": "apps",
                "versions": [{ "groupVersion": "apps/v1", "version": "v1" }],
                "preferredVersion": { "groupVersion": "apps/v1", "version": "v1" }
            }
        ]
    }))
}

pub async fn version() -> Json<Value> {
    Json(json!({
        "major": "1",
        "minor": "28",
        "gitVersion": "v1.28.0-beemesh",
        "platform": "linux/amd64"
    }))
}

async fn api_versions() -> Json<Value> {
    Json(json!({
        "kind": "APIVersions",
        "apiVersion": "v1",
        "versions": ["v1"],
        "serverAddressByClientCIDRs": []
    }))
}

async fn core_v1_resources() -> Json<Value> {
    Json(json!({
        "kind": "APIResourceList",
        "groupVersion": "v1",
        "resources": []
    }))
}

async fn apps_v1_resources() -> Json<Value> {
    Json(json!({
        "kind": "APIResourceList",
        "groupVersion": "apps/v1",
        "resources": [
            {
                "name": "deployments",
                "singularName": "deployment",
                "namespaced": true,
                "kind": DEPLOYMENT_KIND,
                "verbs": ["get", "list", "create", "delete", "patch", "update"]
            }
        ]
    }))
}

fn parse_resource(body: &[u8]) -> Result<Value, StatusCode> {
    if body.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    serde_json::from_slice(body).or_else(|_| {
        let yaml: serde_yaml::Value =
            serde_yaml::from_slice(body).map_err(|_| StatusCode::BAD_REQUEST)?;
        serde_json::to_value(yaml).map_err(|_| StatusCode::BAD_REQUEST)
    })
}

fn ensure_metadata(
    mut manifest: Value,
    namespace_from_path: &str,
    expected_kind: &str,
    expected_api_version: &str,
) -> Result<(Value, String, String), StatusCode> {
    let kind = manifest
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or(expected_kind);
    if kind != expected_kind {
        return Err(StatusCode::BAD_REQUEST);
    }
    manifest["kind"] = Value::String(expected_kind.to_string());

    let api_version = manifest
        .get("apiVersion")
        .and_then(|v| v.as_str())
        .unwrap_or(expected_api_version);
    if api_version != expected_api_version {
        return Err(StatusCode::BAD_REQUEST);
    }
    manifest["apiVersion"] = Value::String(expected_api_version.to_string());

    let mut metadata = manifest
        .get("metadata")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_else(Map::new);

    let name = metadata
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or(StatusCode::BAD_REQUEST)?;
    let namespace = metadata
        .get("namespace")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| namespace_from_path.to_string());

    metadata.insert("namespace".to_string(), Value::String(namespace.clone()));
    metadata.remove("uid");
    metadata.remove("resourceVersion");
    metadata.remove("creationTimestamp");
    metadata.remove("managedFields");

    manifest["metadata"] = Value::Object(metadata);

    Ok((manifest, namespace, name))
}

fn format_timestamp(ts: SystemTime) -> Option<String> {
    let datetime = OffsetDateTime::from(ts);
    datetime.format(&Rfc3339).ok()
}

fn build_deployment_response(task_id: &str, record: &TaskRecord) -> Value {
    if let Some(kube) = &record.kube {
        let mut metadata = kube
            .manifest
            .get("metadata")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_else(Map::new);
        metadata.insert("name".into(), Value::String(kube.name.clone()));
        metadata.insert("namespace".into(), Value::String(kube.namespace.clone()));
        metadata.insert("uid".into(), Value::String(kube.uid.clone()));
        metadata.insert(
            "resourceVersion".into(),
            Value::String(kube.resource_version.to_string()),
        );
        if let Some(ts) = format_timestamp(kube.creation_timestamp) {
            metadata.insert("creationTimestamp".into(), Value::String(ts));
        }

        return Value::Object({
            let mut map = Map::new();
            map.insert("apiVersion".into(), Value::String(kube.api_version.clone()));
            map.insert("kind".into(), Value::String(kube.kind.clone()));
            map.insert("metadata".into(), Value::Object(metadata));
            map.insert(
                "spec".into(),
                kube.manifest
                    .get("spec")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
            );
            map.insert(
                "status".into(),
                json!({
                    "observedGeneration": kube.resource_version,
                    "conditions": []
                }),
            );
            map.insert("manifest_id".into(), Value::String(task_id.to_string()));
            map
        });
    }
    json!({})
}

fn compute_manifest_id(namespace: &str, name: &str, kind: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    namespace.hash(&mut hasher);
    name.hash(&mut hasher);
    kind.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn desired_replicas(manifest: &Value) -> usize {
    manifest
        .get("spec")
        .and_then(|spec| spec.get("replicas"))
        .and_then(|replicas| replicas.as_u64())
        .unwrap_or(1) as usize
}

async fn send_apply_to_peer(
    state: &RestState,
    peer_id: &str,
    peer_pubkey_b64: &str,
    manifest_json: &str,
    manifest_id: &str,
) -> Result<(), StatusCode> {
    let node_pubkey_bytes = base64::engine::general_purpose::STANDARD
        .decode(peer_pubkey_b64)
        .map_err(|e| {
            warn!("Failed to decode peer pubkey for {}: {}", peer_id, e);
            StatusCode::BAD_REQUEST
        })?;

    let encrypted_blob =
        encrypt_payload_for_recipient(&node_pubkey_bytes, manifest_json.as_bytes()).map_err(
            |e| {
                error!("Failed to encrypt manifest for {}: {}", peer_id, e);
                StatusCode::INTERNAL_SERVER_ERROR
            },
        )?;

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let envelope = crate::protocol::machine::build_envelope_canonical_with_peer(
        &encrypted_blob,
        "manifest",
        "",
        ts,
        "ml-kem-512",
        peer_id,
        None,
    );

    let operation_id = Uuid::new_v4().to_string();
    let manifest_json_b64 = base64::engine::general_purpose::STANDARD.encode(&envelope);
    let apply_request_bytes = crate::protocol::machine::build_apply_request(
        1,
        &operation_id,
        &manifest_json_b64,
        "",
        manifest_id,
    );

    let target_peer_id: libp2p::PeerId = peer_id.parse().map_err(|e| {
        warn!("Invalid peer id {}: {}", peer_id, e);
        StatusCode::BAD_REQUEST
    })?;

    let (reply_tx, mut reply_rx) = mpsc::unbounded_channel::<Result<String, String>>();
    state
        .control_tx
        .send(
            crate::libp2p_beemesh::control::Libp2pControl::SendApplyRequest {
                peer_id: target_peer_id,
                manifest: apply_request_bytes,
                reply_tx,
            },
        )
        .map_err(|e| {
            error!("Failed to dispatch apply to {}: {}", peer_id, e);
            StatusCode::BAD_GATEWAY
        })?;

    match timeout(Duration::from_secs(15), reply_rx.recv()).await {
        Ok(Some(Ok(_))) => Ok(()),
        Ok(Some(Err(e))) => {
            warn!("Apply request failed for {}: {}", peer_id, e);
            Err(StatusCode::BAD_GATEWAY)
        }
        Ok(None) => {
            warn!("Apply channel closed for {}", peer_id);
            Err(StatusCode::BAD_GATEWAY)
        }
        Err(_) => {
            warn!("Timed out sending apply to {}", peer_id);
            Err(StatusCode::GATEWAY_TIMEOUT)
        }
    }
}

async fn schedule_deployment(
    state: &RestState,
    manifest_id: &str,
    manifest: &Value,
) -> Result<Vec<String>, StatusCode> {
    let replicas = std::cmp::max(1, desired_replicas(manifest));
    let manifest_str =
        serde_json::to_string(manifest).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let max_candidates = std::cmp::max(replicas * 2, 5);
    let candidates = super::collect_candidate_pubkeys(state, manifest_id, max_candidates).await?;
    if candidates.len() < replicas {
        warn!(
            "schedule_deployment: insufficient candidates (need {}, got {})",
            replicas,
            candidates.len()
        );
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }

    let node_candidates: Vec<NodeCandidate> = candidates
        .iter()
        .map(|(peer_id, _)| NodeCandidate {
            node_id: peer_id.clone(),
            load_factor: 0.0,
            available: true,
            capabilities: NodeCapabilities::default(),
        })
        .collect();

    let scheduler_config = SchedulerConfig {
        strategy: SchedulingStrategy::RoundRobin,
        max_candidates: None,
        enable_load_balancing: false,
    };
    let scheduler = Scheduler::new(scheduler_config);
    let plan = scheduler
        .schedule_workload(&node_candidates, replicas)
        .map_err(|e| {
            error!("schedule_deployment: scheduler error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let mut assigned = Vec::new();
    for idx in plan.selected_candidates {
        if let Some((peer_id, pubkey)) = candidates.get(idx) {
            send_apply_to_peer(state, peer_id, pubkey, &manifest_str, manifest_id).await?;
            assigned.push(peer_id.clone());
        }
    }

    Ok(assigned)
}

async fn delete_manifest_from_peers(state: &RestState, manifest_id: &str, peers: &[String]) {
    if peers.is_empty() {
        return;
    }

    let operation_id = Uuid::new_v4().to_string();
    let delete_request =
        crate::protocol::machine::build_delete_request(manifest_id, &operation_id, "", true);

    for peer in peers {
        if let Err(e) = super::send_delete_request_to_peer(state, peer, &delete_request).await {
            warn!("Failed to send delete to {}: {}", peer, e);
        }
    }
}

async fn cleanup_old_assignments(
    state: &RestState,
    manifest_id: &str,
    previous: &[String],
    current: &[String],
) {
    if previous.is_empty() {
        return;
    }

    let stale: Vec<String> = previous
        .iter()
        .filter(|peer| !current.contains(peer))
        .cloned()
        .collect();

    delete_manifest_from_peers(state, manifest_id, &stale).await;
}

async fn upsert_deployment(
    state: &RestState,
    namespace: &str,
    name: &str,
    manifest: Value,
) -> Result<Value, StatusCode> {
    let existing_snapshot = {
        let store = state.task_store.read().await;
        find_deployment(&store, namespace, name).map(|(id, rec)| (id.clone(), rec.clone()))
    };

    let manifest_id = compute_manifest_id(namespace, name, DEPLOYMENT_KIND);
    let record_key = existing_snapshot
        .as_ref()
        .map(|(id, _)| id.clone())
        .unwrap_or_else(|| manifest_id.clone());
    let creation_timestamp = existing_snapshot
        .as_ref()
        .and_then(|(_, rec)| rec.kube.as_ref().map(|k| k.creation_timestamp))
        .unwrap_or_else(SystemTime::now);
    let created_at = existing_snapshot
        .as_ref()
        .map(|(_, rec)| rec.created_at)
        .unwrap_or(creation_timestamp);
    let previous_assignments = existing_snapshot
        .as_ref()
        .and_then(|(_, rec)| rec.assigned_peers.clone())
        .unwrap_or_default();
    let resource_version = existing_snapshot
        .as_ref()
        .and_then(|(_, rec)| rec.kube.as_ref().map(|k| k.resource_version + 1))
        .unwrap_or(1);

    let assigned_peers = schedule_deployment(state, &record_key, &manifest).await?;
    let last_operation_id = format!(
        "kube:apps/v1:Deployment:{}:{}:{}",
        namespace,
        name,
        Uuid::new_v4()
    );
    super::store_operation_manifest_mapping(&last_operation_id, &record_key).await;

    let manifest_bytes =
        serde_json::to_vec(&manifest).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let response_value = {
        let mut store = state.task_store.write().await;
        if let Some(record) = store.get_mut(&record_key) {
            record.manifest_bytes = manifest_bytes.clone();
            record.created_at = created_at;
            record.assigned_peers = Some(assigned_peers.clone());
            record.manifest_cid = Some(record_key.clone());
            record.last_operation_id = Some(last_operation_id.clone());
            if let Some(kube_meta) = record.kube.as_mut() {
                kube_meta.manifest = manifest.clone();
                kube_meta.resource_version = resource_version;
                kube_meta.creation_timestamp = creation_timestamp;
            } else {
                record.kube = Some(KubeResourceRecord {
                    api_version: APPS_V1.to_string(),
                    kind: DEPLOYMENT_KIND.to_string(),
                    namespace: namespace.to_string(),
                    name: name.to_string(),
                    uid: record_key.clone(),
                    resource_version,
                    manifest: manifest.clone(),
                    creation_timestamp,
                });
            }
            build_deployment_response(&record_key, record)
        } else {
            let kube_record = KubeResourceRecord {
                api_version: APPS_V1.to_string(),
                kind: DEPLOYMENT_KIND.to_string(),
                namespace: namespace.to_string(),
                name: name.to_string(),
                uid: record_key.clone(),
                resource_version,
                manifest: manifest.clone(),
                creation_timestamp,
            };
            let task_record = TaskRecord {
                manifest_bytes: manifest_bytes.clone(),
                created_at,
                manifests_distributed: HashMap::new(),
                assigned_peers: Some(assigned_peers.clone()),
                manifest_cid: Some(record_key.clone()),
                last_operation_id: Some(last_operation_id.clone()),
                owner_pubkey: existing_snapshot
                    .as_ref()
                    .map(|(_, rec)| rec.owner_pubkey.clone())
                    .unwrap_or_default(),
                kube: Some(kube_record),
            };
            store.insert(record_key.clone(), task_record);
            let record = store.get(&record_key).unwrap();
            build_deployment_response(&record_key, record)
        }
    };

    cleanup_old_assignments(state, &record_key, &previous_assignments, &assigned_peers).await;
    Ok(response_value)
}

fn find_deployment<'a>(
    store: &'a HashMap<String, TaskRecord>,
    namespace: &str,
    name: &str,
) -> Option<(&'a String, &'a TaskRecord)> {
    store.iter().find(|(_, rec)| {
        rec.kube
            .as_ref()
            .map(|k| k.kind == DEPLOYMENT_KIND && k.namespace == namespace && k.name == name)
            .unwrap_or(false)
    })
}

async fn list_deployments(
    Path(namespace): Path<String>,
    State(state): State<RestState>,
) -> Json<Value> {
    let store = state.task_store.read().await;
    let items: Vec<Value> = store
        .iter()
        .filter_map(|(id, rec)| {
            if rec
                .kube
                .as_ref()
                .map(|k| k.kind == DEPLOYMENT_KIND && k.namespace == namespace)
                .unwrap_or(false)
            {
                Some(build_deployment_response(id, rec))
            } else {
                None
            }
        })
        .collect();
    Json(json!({
        "kind": "DeploymentList",
        "apiVersion": "apps/v1",
        "metadata": { "resourceVersion": "1" },
        "items": items
    }))
}

async fn get_deployment(
    Path((namespace, name)): Path<(String, String)>,
    State(state): State<RestState>,
) -> Result<Json<Value>, StatusCode> {
    let store = state.task_store.read().await;
    if let Some((id, record)) = find_deployment(&store, &namespace, &name) {
        return Ok(Json(build_deployment_response(id, record)));
    }
    Err(StatusCode::NOT_FOUND)
}

async fn create_deployment(
    Path(namespace): Path<String>,
    State(state): State<RestState>,
    body: Bytes,
) -> Result<Json<Value>, StatusCode> {
    let manifest = parse_resource(&body)?;
    let (manifest, ns, name) = ensure_metadata(manifest, &namespace, DEPLOYMENT_KIND, APPS_V1)?;
    if ns != namespace {
        return Err(StatusCode::BAD_REQUEST);
    }

    let store = state.task_store.read().await;
    let already_exists = find_deployment(&store, &ns, &name).is_some();
    drop(store);
    if already_exists {
        return Err(StatusCode::CONFLICT);
    }

    let response = upsert_deployment(&state, &ns, &name, manifest).await?;
    Ok(Json(response))
}

async fn replace_deployment(
    Path((namespace, name)): Path<(String, String)>,
    State(state): State<RestState>,
    body: Bytes,
) -> Result<Json<Value>, StatusCode> {
    let manifest = parse_resource(&body)?;
    let (manifest, ns, nm) = ensure_metadata(manifest, &namespace, DEPLOYMENT_KIND, APPS_V1)?;
    if ns != namespace || nm != name {
        return Err(StatusCode::BAD_REQUEST);
    }

    let response = upsert_deployment(&state, &ns, &nm, manifest).await?;
    Ok(Json(response))
}

async fn apply_deployment(
    Path((namespace, name)): Path<(String, String)>,
    State(state): State<RestState>,
    _params: Query<HashMap<String, String>>,
    body: Bytes,
) -> Result<Json<Value>, StatusCode> {
    let manifest = parse_resource(&body)?;
    let (manifest, ns, nm) = ensure_metadata(manifest, &namespace, DEPLOYMENT_KIND, APPS_V1)?;
    if ns != namespace || nm != name {
        return Err(StatusCode::BAD_REQUEST);
    }

    let response = upsert_deployment(&state, &ns, &nm, manifest).await?;
    Ok(Json(response))
}

async fn delete_deployment(
    Path((namespace, name)): Path<(String, String)>,
    State(state): State<RestState>,
) -> Result<Json<Value>, StatusCode> {
    let mut store = state.task_store.write().await;
    let key_opt = store
        .iter()
        .find(|(_, rec)| {
            rec.kube
                .as_ref()
                .map(|k| k.kind == DEPLOYMENT_KIND && k.namespace == namespace && k.name == name)
                .unwrap_or(false)
        })
        .map(|(k, _)| k.clone());

    if let Some(key) = key_opt {
        let record = store.remove(&key);
        drop(store);

        if let Some(record) = record {
            let assigned = record.assigned_peers.unwrap_or_default();
            delete_manifest_from_peers(&state, &key, &assigned).await;
        }

        let _ = crate::run::remove_workloads_by_manifest_id(&key).await;
        return Ok(Json(json!({
            "kind": "Status",
            "apiVersion": "v1",
            "status": "Success",
            "details": {
                "name": name,
                "group": "apps",
                "kind": DEPLOYMENT_KIND
            }
        })));
    }

    Err(StatusCode::NOT_FOUND)
}
