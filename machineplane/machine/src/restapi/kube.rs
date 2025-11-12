use super::{KubeResourceRecord, RestState, TaskRecord};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, patch},
};
use serde_json::{Map, Value, json};
use serde_yaml;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::SystemTime;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

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
        .route("/namespaces/:namespace/deployments", get(list_deployments))
        .route(
            "/namespaces/:namespace/deployments/:name",
            get(get_deployment).patch(apply_deployment),
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
                "verbs": ["get", "list", "patch"]
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

async fn upsert_deployment(
    state: &RestState,
    namespace: &str,
    name: &str,
    manifest: Value,
) -> Result<Value, StatusCode> {
    let mut store = state.task_store.write().await;
    let existing_key = store
        .iter()
        .find(|(_, rec)| {
            rec.kube
                .as_ref()
                .map(|k| k.kind == DEPLOYMENT_KIND && k.namespace == namespace && k.name == name)
                .unwrap_or(false)
        })
        .map(|(k, _)| k.clone());

    if let Some(key) = existing_key {
        if let Some(record) = store.get_mut(&key) {
            if let Some(kube) = record.kube.as_mut() {
                kube.resource_version += 1;
                kube.manifest = manifest.clone();
            }
            record.manifest_bytes =
                serde_json::to_vec(&manifest).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            record.manifest_cid = Some(key.clone());
            let response = build_deployment_response(&key, record);
            return Ok(response);
        }
    }

    let manifest_id = compute_manifest_id(namespace, name, DEPLOYMENT_KIND);
    let uid = manifest_id.clone();
    let creation_timestamp = SystemTime::now();
    let kube_record = KubeResourceRecord {
        api_version: APPS_V1.to_string(),
        kind: DEPLOYMENT_KIND.to_string(),
        namespace: namespace.to_string(),
        name: name.to_string(),
        uid: uid.clone(),
        resource_version: 1,
        manifest: manifest.clone(),
        creation_timestamp,
    };
    let task_record = TaskRecord {
        manifest_bytes: serde_json::to_vec(&manifest)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        created_at: creation_timestamp,
        manifests_distributed: HashMap::new(),
        assigned_peers: None,
        manifest_cid: Some(manifest_id.clone()),
        last_operation_id: Some(format!("kube:apps/v1:Deployment:{}:{}", namespace, name)),
        owner_pubkey: Vec::new(),
        kube: Some(kube_record),
    };
    store.insert(manifest_id.clone(), task_record);
    let record = store.get(&manifest_id).unwrap();
    Ok(build_deployment_response(&manifest_id, record))
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
