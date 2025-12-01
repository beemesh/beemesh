//! Kubernetes-compatible API handlers (stateless implementation)
//!
//! This module provides K8s API compatibility by deriving all state from
//! the Podman runtime. Following Beemesh's statelessness principle:
//! - GET/LIST: Query Podman runtime for actual pod state
//! - CREATE/UPDATE: Publish tender via Gossipsub (fire-and-forget)
//! - DELETE: Publish delete tender via Gossipsub (fire-and-forget)
//!
//! The machineplane does NOT store K8s resource state. All responses
//! reflect the current runtime state on this specific node.

use super::RestState;
use crate::signatures;
use crate::scheduler::register_local_manifest;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
};
use libp2p::identity::Keypair;
use log::{info, warn};
use rand::RngCore;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio::time::{Duration, timeout};
use uuid::Uuid;

// K8s resource kinds
const DEPLOYMENT_KIND: &str = "Deployment";
const REPLICASET_KIND: &str = "ReplicaSet";
const STATEFULSET_KIND: &str = "StatefulSet";
const POD_KIND: &str = "Pod";
const APPS_V1: &str = "apps/v1";
const CORE_V1: &str = "v1";

// Podman labels used to identify K8s resources
const LABEL_APP: &str = "app";
const LABEL_APP_NAME: &str = "app.kubernetes.io/name";
const LABEL_APP_INSTANCE: &str = "app.kubernetes.io/instance";
const LABEL_POD_NAMESPACE: &str = "io.kubernetes.pod.namespace";
const LABEL_WORKLOAD_KIND: &str = "beemesh.io/workload-kind";

// =============================================================================
// Router Setup
// =============================================================================

pub fn core_router() -> Router<RestState> {
    Router::new()
        .route("/", get(api_versions))
        .route("/v1", get(core_v1_resources))
        .route("/v1/namespaces/{namespace}/pods", get(list_pods))
        .route("/v1/namespaces/{namespace}/pods/{name}", get(get_pod))
        .route(
            "/v1/namespaces/{namespace}/pods/{name}/log",
            get(get_pod_logs),
        )
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
            get(get_deployment).delete(delete_deployment),
        )
        .route(
            "/namespaces/{namespace}/statefulsets",
            get(list_statefulsets).post(create_statefulset),
        )
        .route(
            "/namespaces/{namespace}/statefulsets/{name}",
            get(get_statefulset).delete(delete_statefulset),
        )
        .route(
            "/namespaces/{namespace}/replicasets",
            get(list_replicasets),
        )
        .route(
            "/namespaces/{namespace}/replicasets/{name}",
            get(get_replicaset),
        )
}

// =============================================================================
// API Discovery Endpoints
// =============================================================================

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
        "resources": [
            {
                "name": "pods",
                "singularName": "pod",
                "namespaced": true,
                "kind": POD_KIND,
                "verbs": ["get", "list"]
            },
            {
                "name": "pods/log",
                "singularName": "",
                "namespaced": true,
                "kind": POD_KIND,
                "verbs": ["get"]
            }
        ]
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
                "verbs": ["get", "list", "create", "delete"]
            },
            {
                "name": "statefulsets",
                "singularName": "statefulset",
                "namespaced": true,
                "kind": STATEFULSET_KIND,
                "verbs": ["get", "list", "create", "delete"]
            },
            {
                "name": "replicasets",
                "singularName": "replicaset",
                "namespaced": true,
                "kind": REPLICASET_KIND,
                "verbs": ["get", "list"]
            }
        ]
    }))
}

// =============================================================================
// Podman Client Helper
// =============================================================================

async fn get_podman_client() -> Result<crate::runtimes::podman_api::PodmanApiClient, StatusCode> {
    let socket = crate::runtimes::podman::PodmanEngine::get_socket()
        .ok_or_else(|| {
            warn!("Podman socket not configured");
            StatusCode::SERVICE_UNAVAILABLE
        })?;
    
    Ok(crate::runtimes::podman_api::PodmanApiClient::new(&socket))
}

// =============================================================================
// Workload Info Derived from Podman Pods
// =============================================================================

/// Workload information aggregated from Podman pods
#[derive(Debug, Clone)]
struct WorkloadInfo {
    name: String,
    namespace: String,
    kind: String,  // "Deployment" or "StatefulSet"
    replicas: u32,
    ready_replicas: u32,
    created: Option<String>,
    labels: HashMap<String, String>,
}

/// Extract workload info from pod labels
fn extract_workload_info(pod: &crate::runtimes::podman_api::PodListEntry) -> Option<WorkloadInfo> {
    let labels = pod.labels.clone().unwrap_or_default();
    
    // Get namespace
    let namespace = labels.get(LABEL_POD_NAMESPACE)
        .cloned()
        .unwrap_or_else(|| "default".to_string());
    
    // Get workload name from various label sources
    let workload_name = labels.get(LABEL_APP_NAME)
        .or_else(|| labels.get(LABEL_APP))
        .or_else(|| labels.get(LABEL_APP_INSTANCE))
        .cloned()?;
    
    // Determine workload kind - check for ordinal pattern (StatefulSet) or explicit label
    let kind = if let Some(k) = labels.get(LABEL_WORKLOAD_KIND) {
        k.clone()
    } else {
        // Check if pod name has ordinal suffix (e.g., "nginx-0", "nginx-1")
        let pod_name = pod.name.clone().unwrap_or_default();
        if pod_name.ends_with("-0") || pod_name.chars().last().map(|c| c.is_ascii_digit()).unwrap_or(false) {
            // Check for StatefulSet pattern: {name}-{ordinal}
            if let Some(pos) = pod_name.rfind('-') {
                let suffix = &pod_name[pos+1..];
                if suffix.parse::<u32>().is_ok() {
                    STATEFULSET_KIND.to_string()
                } else {
                    DEPLOYMENT_KIND.to_string()
                }
            } else {
                DEPLOYMENT_KIND.to_string()
            }
        } else {
            DEPLOYMENT_KIND.to_string()
        }
    };
    
    let is_running = pod.status.as_ref()
        .map(|s| s.to_lowercase() == "running")
        .unwrap_or(false);
    
    Some(WorkloadInfo {
        name: workload_name,
        namespace,
        kind,
        replicas: 1,
        ready_replicas: if is_running { 1 } else { 0 },
        created: pod.created.clone(),
        labels,
    })
}

/// Aggregate pods into workload groups
fn aggregate_workloads(
    pods: &[crate::runtimes::podman_api::PodListEntry],
    namespace_filter: &str,
    kind_filter: Option<&str>,
) -> Vec<WorkloadInfo> {
    let mut workloads: HashMap<String, WorkloadInfo> = HashMap::new();
    
    for pod in pods {
        if let Some(info) = extract_workload_info(pod) {
            // Filter by namespace
            if info.namespace != namespace_filter {
                continue;
            }
            
            // Filter by kind if specified
            if kind_filter.is_some_and(|kind| info.kind != kind) {
                continue;
            }
            
            let key = format!("{}/{}/{}", info.namespace, info.kind, info.name);
            
            let entry = workloads.entry(key).or_insert_with(|| WorkloadInfo {
                name: info.name.clone(),
                namespace: info.namespace.clone(),
                kind: info.kind.clone(),
                replicas: 0,
                ready_replicas: 0,
                created: info.created.clone(),
                labels: info.labels.clone(),
            });
            
            entry.replicas += 1;
            entry.ready_replicas += info.ready_replicas;
            
            // Use earliest creation time
            if entry.created.is_none() {
                entry.created = info.created.clone();
            }
        }
    }
    
    workloads.into_values().collect()
}

// =============================================================================
// Deployment Handlers (Read from Podman, Write via Tender)
// =============================================================================

fn build_deployment_response(info: &WorkloadInfo) -> Value {
    json!({
        "apiVersion": APPS_V1,
        "kind": DEPLOYMENT_KIND,
        "metadata": {
            "name": info.name,
            "namespace": info.namespace,
            "uid": format!("deploy-{}-{}", info.namespace, info.name),
            "resourceVersion": "1",
            "creationTimestamp": info.created,
            "labels": info.labels
        },
        "spec": {
            "replicas": info.replicas,
            "selector": {
                "matchLabels": {
                    "app": info.name
                }
            }
        },
        "status": {
            "replicas": info.replicas,
            "readyReplicas": info.ready_replicas,
            "availableReplicas": info.ready_replicas,
            "updatedReplicas": info.replicas,
            "observedGeneration": 1,
            "conditions": [{
                "type": "Available",
                "status": if info.ready_replicas > 0 { "True" } else { "False" },
                "reason": if info.ready_replicas > 0 { "MinimumReplicasAvailable" } else { "MinimumReplicasUnavailable" }
            }]
        }
    })
}

async fn list_deployments(
    Path(namespace): Path<String>,
    State(_state): State<RestState>,
) -> Result<Json<Value>, StatusCode> {
    let client = get_podman_client().await?;
    
    let pods = client.list_pods().await.map_err(|e| {
        warn!("Failed to list pods from Podman: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    
    let workloads = aggregate_workloads(&pods, &namespace, Some(DEPLOYMENT_KIND));
    let items: Vec<Value> = workloads.iter()
        .map(build_deployment_response)
        .collect();

    Ok(Json(json!({
        "kind": "DeploymentList",
        "apiVersion": APPS_V1,
        "metadata": { "resourceVersion": "1" },
        "items": items
    })))
}

async fn get_deployment(
    Path((namespace, name)): Path<(String, String)>,
    State(_state): State<RestState>,
) -> Result<Json<Value>, StatusCode> {
    let client = get_podman_client().await?;
    
    let pods = client.list_pods().await.map_err(|e| {
        warn!("Failed to list pods from Podman: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    
    let workloads = aggregate_workloads(&pods, &namespace, Some(DEPLOYMENT_KIND));
    
    for w in &workloads {
        if w.name == name {
            return Ok(Json(build_deployment_response(w)));
        }
    }

    Err(StatusCode::NOT_FOUND)
}

async fn create_deployment(
    Path(namespace): Path<String>,
    State(state): State<RestState>,
    body: Bytes,
) -> Result<Json<Value>, StatusCode> {
    let manifest = parse_manifest(&body)?;
    let (manifest, ns, name) = ensure_metadata(manifest, &namespace, DEPLOYMENT_KIND, APPS_V1)?;
    
    if ns != namespace {
        return Err(StatusCode::BAD_REQUEST);
    }
    
    // Fire-and-forget: publish tender and return accepted
    let tender_id = publish_workload_tender(&state, &manifest).await?;
    
    info!("Deployment {}/{} tender published (tender_id={})", namespace, name, tender_id);
    
    // Return a placeholder response - actual state will appear when pod starts
    Ok(Json(json!({
        "apiVersion": APPS_V1,
        "kind": DEPLOYMENT_KIND,
        "tender_id": tender_id,
        "metadata": {
            "name": name,
            "namespace": namespace,
            "uid": format!("deploy-{}-{}", namespace, name),
            "resourceVersion": "1"
        },
        "spec": {
            "replicas": 1
        },
        "status": {
            "replicas": 0,
            "readyReplicas": 0,
            "conditions": [{
                "type": "Progressing",
                "status": "True",
                "reason": "TenderPublished"
            }]
        }
    })))
}

async fn delete_deployment(
    Path((namespace, name)): Path<(String, String)>,
    State(state): State<RestState>,
) -> Result<Json<Value>, StatusCode> {
    // Build a delete manifest (replicas: 0 with delete annotation)
    let delete_manifest = build_delete_manifest(&namespace, &name, DEPLOYMENT_KIND, APPS_V1);
    
    // Fire-and-forget: publish delete tender
    let _ = publish_workload_tender(&state, &delete_manifest).await?;
    
    info!("Deployment {}/{} delete tender published", namespace, name);
    
    Ok(Json(json!({
        "kind": "Status",
        "apiVersion": "v1",
        "status": "Success",
        "details": {
            "name": name,
            "group": "apps",
            "kind": DEPLOYMENT_KIND
        }
    })))
}

// =============================================================================
// StatefulSet Handlers (Read from Podman, Write via Tender)
// =============================================================================

fn build_statefulset_response(info: &WorkloadInfo) -> Value {
    json!({
        "apiVersion": APPS_V1,
        "kind": STATEFULSET_KIND,
        "metadata": {
            "name": info.name,
            "namespace": info.namespace,
            "uid": format!("sts-{}-{}", info.namespace, info.name),
            "resourceVersion": "1",
            "creationTimestamp": info.created,
            "labels": info.labels
        },
        "spec": {
            "replicas": info.replicas,
            "serviceName": info.name,
            "selector": {
                "matchLabels": {
                    "app": info.name
                }
            }
        },
        "status": {
            "replicas": info.replicas,
            "readyReplicas": info.ready_replicas,
            "currentReplicas": info.replicas,
            "updatedReplicas": info.replicas,
            "currentRevision": format!("{}-1", info.name),
            "updateRevision": format!("{}-1", info.name),
            "observedGeneration": 1
        }
    })
}

async fn list_statefulsets(
    Path(namespace): Path<String>,
    State(_state): State<RestState>,
) -> Result<Json<Value>, StatusCode> {
    let client = get_podman_client().await?;
    
    let pods = client.list_pods().await.map_err(|e| {
        warn!("Failed to list pods from Podman: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    
    let workloads = aggregate_workloads(&pods, &namespace, Some(STATEFULSET_KIND));
    let items: Vec<Value> = workloads.iter()
        .map(build_statefulset_response)
        .collect();

    Ok(Json(json!({
        "kind": "StatefulSetList",
        "apiVersion": APPS_V1,
        "metadata": { "resourceVersion": "1" },
        "items": items
    })))
}

async fn get_statefulset(
    Path((namespace, name)): Path<(String, String)>,
    State(_state): State<RestState>,
) -> Result<Json<Value>, StatusCode> {
    let client = get_podman_client().await?;
    
    let pods = client.list_pods().await.map_err(|e| {
        warn!("Failed to list pods from Podman: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    
    let workloads = aggregate_workloads(&pods, &namespace, Some(STATEFULSET_KIND));
    
    for w in &workloads {
        if w.name == name {
            return Ok(Json(build_statefulset_response(w)));
        }
    }

    Err(StatusCode::NOT_FOUND)
}

async fn create_statefulset(
    Path(namespace): Path<String>,
    State(state): State<RestState>,
    body: Bytes,
) -> Result<Json<Value>, StatusCode> {
    let manifest = parse_manifest(&body)?;
    let (manifest, ns, name) = ensure_metadata(manifest, &namespace, STATEFULSET_KIND, APPS_V1)?;
    
    if ns != namespace {
        return Err(StatusCode::BAD_REQUEST);
    }
    
    // Fire-and-forget: publish tender
    let tender_id = publish_workload_tender(&state, &manifest).await?;
    
    info!("StatefulSet {}/{} tender published (tender_id={})", namespace, name, tender_id);
    
    Ok(Json(json!({
        "apiVersion": APPS_V1,
        "kind": STATEFULSET_KIND,
        "tender_id": tender_id,
        "metadata": {
            "name": name,
            "namespace": namespace,
            "uid": format!("sts-{}-{}", namespace, name),
            "resourceVersion": "1"
        },
        "spec": {
            "replicas": 1
        },
        "status": {
            "replicas": 0,
            "readyReplicas": 0
        }
    })))
}

async fn delete_statefulset(
    Path((namespace, name)): Path<(String, String)>,
    State(state): State<RestState>,
) -> Result<Json<Value>, StatusCode> {
    let delete_manifest = build_delete_manifest(&namespace, &name, STATEFULSET_KIND, APPS_V1);
    
    // Fire-and-forget: publish delete tender
    let _ = publish_workload_tender(&state, &delete_manifest).await?;
    
    info!("StatefulSet {}/{} delete tender published", namespace, name);
    
    Ok(Json(json!({
        "kind": "Status",
        "apiVersion": "v1",
        "status": "Success",
        "details": {
            "name": name,
            "group": "apps",
            "kind": STATEFULSET_KIND
        }
    })))
}

// =============================================================================
// ReplicaSet Handlers (Read-only, derived from Pods)
// =============================================================================

fn build_replicaset_response(info: &WorkloadInfo) -> Value {
    let rs_name = format!("{}-rs", info.name);
    json!({
        "apiVersion": APPS_V1,
        "kind": REPLICASET_KIND,
        "metadata": {
            "name": rs_name,
            "namespace": info.namespace,
            "uid": format!("rs-{}-{}", info.namespace, info.name),
            "resourceVersion": "1",
            "creationTimestamp": info.created,
            "ownerReferences": [{
                "apiVersion": APPS_V1,
                "kind": DEPLOYMENT_KIND,
                "name": info.name,
                "uid": format!("deploy-{}-{}", info.namespace, info.name),
                "controller": true
            }],
            "labels": {
                "app": info.name
            }
        },
        "spec": {
            "replicas": info.replicas,
            "selector": {
                "matchLabels": {
                    "app": info.name
                }
            }
        },
        "status": {
            "replicas": info.replicas,
            "readyReplicas": info.ready_replicas,
            "availableReplicas": info.ready_replicas,
            "observedGeneration": 1
        }
    })
}

async fn list_replicasets(
    Path(namespace): Path<String>,
    State(_state): State<RestState>,
) -> Result<Json<Value>, StatusCode> {
    let client = get_podman_client().await?;
    
    let pods = client.list_pods().await.map_err(|e| {
        warn!("Failed to list pods from Podman: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    
    // ReplicaSets are derived from Deployments only
    let workloads = aggregate_workloads(&pods, &namespace, Some(DEPLOYMENT_KIND));
    let items: Vec<Value> = workloads.iter()
        .map(build_replicaset_response)
        .collect();

    Ok(Json(json!({
        "kind": "ReplicaSetList",
        "apiVersion": APPS_V1,
        "metadata": { "resourceVersion": "1" },
        "items": items
    })))
}

async fn get_replicaset(
    Path((namespace, name)): Path<(String, String)>,
    State(_state): State<RestState>,
) -> Result<Json<Value>, StatusCode> {
    let client = get_podman_client().await?;
    
    let pods = client.list_pods().await.map_err(|e| {
        warn!("Failed to list pods from Podman: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    
    let workloads = aggregate_workloads(&pods, &namespace, Some(DEPLOYMENT_KIND));
    
    for w in &workloads {
        let rs_name = format!("{}-rs", w.name);
        if rs_name == name {
            return Ok(Json(build_replicaset_response(w)));
        }
    }

    Err(StatusCode::NOT_FOUND)
}

// =============================================================================
// Pod Handlers (Read-only, from Podman runtime)
// =============================================================================

fn podman_status_to_k8s_phase(status: &str) -> &'static str {
    match status.to_lowercase().as_str() {
        "running" => "Running",
        "exited" | "stopped" => "Succeeded",
        "error" | "dead" => "Failed",
        "created" | "initialized" => "Pending",
        "paused" => "Running",
        "degraded" => "Running",
        _ => "Unknown",
    }
}

fn build_pod_response(pod: &crate::runtimes::podman_api::PodListEntry) -> Value {
    let labels = pod.labels.clone().unwrap_or_default();
    let name = pod.name.clone().unwrap_or_default();
    let namespace = labels.get(LABEL_POD_NAMESPACE)
        .cloned()
        .unwrap_or_else(|| "default".to_string());
    let status = pod.status.clone().unwrap_or_else(|| "Unknown".to_string());
    
    let container_statuses: Vec<Value> = pod.containers
        .as_ref()
        .map(|containers| {
            containers.iter().filter_map(|c| {
                let container_name = c.name.clone()?;
                if container_name.ends_with("-infra") {
                    return None;
                }
                let container_status = c.status.clone().unwrap_or_else(|| "unknown".to_string());
                let is_running = container_status.to_lowercase() == "running";
                Some(json!({
                    "name": container_name,
                    "state": if is_running {
                        json!({ "running": { "startedAt": pod.created } })
                    } else {
                        json!({ "terminated": { "exitCode": 0, "reason": container_status } })
                    },
                    "ready": is_running,
                    "restartCount": 0,
                    "containerID": c.id.clone().unwrap_or_default()
                }))
            }).collect()
        })
        .unwrap_or_default();

    json!({
        "apiVersion": CORE_V1,
        "kind": POD_KIND,
        "metadata": {
            "name": name,
            "namespace": namespace,
            "uid": pod.id.clone().unwrap_or_default(),
            "resourceVersion": "1",
            "creationTimestamp": pod.created,
            "labels": labels
        },
        "spec": {
            "containers": container_statuses.iter().map(|cs| {
                json!({
                    "name": cs.get("name").and_then(|v| v.as_str()).unwrap_or("")
                })
            }).collect::<Vec<_>>()
        },
        "status": {
            "phase": podman_status_to_k8s_phase(&status),
            "conditions": [
                { "type": "Ready", "status": if status.to_lowercase() == "running" { "True" } else { "False" } },
                { "type": "PodScheduled", "status": "True" }
            ],
            "containerStatuses": container_statuses,
            "podIP": "",
            "hostIP": ""
        }
    })
}

async fn list_pods(
    Path(namespace): Path<String>,
    State(_state): State<RestState>,
) -> Result<Json<Value>, StatusCode> {
    let client = get_podman_client().await?;
    
    let pods = client.list_pods().await.map_err(|e| {
        warn!("Failed to list pods from Podman: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let items: Vec<Value> = pods
        .iter()
        .filter(|pod| {
            pod.labels
                .as_ref()
                .and_then(|l| l.get(LABEL_POD_NAMESPACE))
                .map(|ns| ns == &namespace)
                .unwrap_or(namespace == "default")
        })
        .map(build_pod_response)
        .collect();

    Ok(Json(json!({
        "kind": "PodList",
        "apiVersion": CORE_V1,
        "metadata": { "resourceVersion": "1" },
        "items": items
    })))
}

async fn get_pod(
    Path((namespace, name)): Path<(String, String)>,
    State(_state): State<RestState>,
) -> Result<Json<Value>, StatusCode> {
    let client = get_podman_client().await?;
    
    match client.inspect_pod(&name).await {
        Ok(pod_inspect) => {
            let pod_entry = crate::runtimes::podman_api::PodListEntry {
                id: pod_inspect.id,
                name: pod_inspect.name,
                status: pod_inspect.state,
                created: pod_inspect.created,
                labels: None,
                containers: pod_inspect.containers.map(|cs| {
                    cs.into_iter().map(|c| crate::runtimes::podman_api::PodContainer {
                        id: c.id,
                        name: c.name,
                        status: c.state,
                    }).collect()
                }),
                infra_id: pod_inspect.infra_container_id,
            };
            Ok(Json(build_pod_response(&pod_entry)))
        }
        Err(crate::runtimes::RuntimeError::InstanceNotFound(_)) => {
            Err(StatusCode::NOT_FOUND)
        }
        Err(e) => {
            warn!("Failed to inspect pod {} in namespace {}: {}", name, namespace, e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn get_pod_logs(
    Path((_namespace, name)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
    State(_state): State<RestState>,
) -> Result<String, StatusCode> {
    let client = get_podman_client().await?;
    
    let tail = params.get("tailLines").and_then(|v| v.parse().ok());
    
    match client.get_pod_logs(&name, tail).await {
        Ok(logs) => Ok(logs),
        Err(crate::runtimes::RuntimeError::InstanceNotFound(_)) => {
            Err(StatusCode::NOT_FOUND)
        }
        Err(e) => {
            warn!("Failed to get logs for pod {}: {}", name, e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

// =============================================================================
// Tender Publishing (Fire-and-Forget)
// =============================================================================

fn parse_manifest(body: &[u8]) -> Result<Value, StatusCode> {
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
    manifest["metadata"] = Value::Object(metadata);

    Ok((manifest, namespace, name))
}

fn build_delete_manifest(namespace: &str, name: &str, kind: &str, api_version: &str) -> Value {
    let mut annotations = Map::new();
    annotations.insert("beemesh.io/operation".into(), Value::String("delete".to_string()));

    let mut metadata = Map::new();
    metadata.insert("name".into(), Value::String(name.to_string()));
    metadata.insert("namespace".into(), Value::String(namespace.to_string()));
    metadata.insert("annotations".into(), Value::Object(annotations));

    let mut spec = Map::new();
    spec.insert("replicas".into(), Value::Number(0.into()));

    json!({
        "apiVersion": api_version,
        "kind": kind,
        "metadata": metadata,
        "spec": spec
    })
}

/// Publish a workload tender and return the tender_id.
async fn publish_workload_tender(state: &RestState, manifest: &Value) -> Result<String, StatusCode> {
    let manifest_str = serde_json::to_string(manifest)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .as_millis() as u64;

    let manifest_digest = hex::encode(Sha256::digest(manifest_str.as_bytes()));
    let tender_id = Uuid::new_v4().to_string();
    
    // Register manifest locally for when we win the tender
    register_local_manifest(&tender_id, &manifest_str);

    let keypair = crate::network::get_node_keypair_for_peer(Some(&state.local_peer_id_bytes))
        .and_then(|(_, sk)| Keypair::from_protobuf_encoding(&sk).ok())
        .ok_or_else(|| {
            warn!("Tender publication aborted: missing node keypair");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let mut tender = crate::messages::Tender {
        id: tender_id.clone(),
        manifest_digest,
        qos_preemptible: false,
        timestamp,
        nonce: rand::thread_rng().next_u64(),
        signature: Vec::new(),
    };

    signatures::sign_tender(&mut tender, &keypair).map_err(|e| {
        warn!("Failed to sign tender: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let tender_bytes = bincode::serialize(&tender).expect("tender serialization");

    let (reply_tx, mut reply_rx) = mpsc::unbounded_channel::<Result<(), String>>();
    state
        .control_tx
        .send(crate::network::control::Libp2pControl::PublishTender {
            payload: tender_bytes,
            reply_tx,
        })
        .map_err(|e| {
            warn!("Failed to dispatch tender: {}", e);
            StatusCode::BAD_GATEWAY
        })?;

    // Wait briefly for publication confirmation
    match timeout(Duration::from_secs(5), reply_rx.recv()).await {
        Ok(Some(Ok(_))) => Ok(tender_id),
        Ok(Some(Err(e))) => {
            warn!("Tender publication failed: {}", e);
            Err(StatusCode::BAD_GATEWAY)
        }
        Ok(None) => {
            warn!("Tender publication channel closed");
            Err(StatusCode::BAD_GATEWAY)
        }
        Err(_) => {
            warn!("Timed out publishing tender");
            Err(StatusCode::GATEWAY_TIMEOUT)
        }
    }
}
