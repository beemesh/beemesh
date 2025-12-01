//! Podman runtime engine implementation
//!
//! # Overview
//!
//! This module implements the `RuntimeEngine` trait using Podman as the underlying container
//! runtime. Podman is currently the **only supported production runtime** for BeeMesh.
//!
//! # Why Podman?
//!
//! Podman is the only container runtime that provides **native Kubernetes Pod support**
//! without requiring Kubernetes. Key features:
//!
//! ## Native Pod Support via `podman kube play`
//!
//! - **Direct Kubernetes YAML execution**: Can run `podman kube play manifest.yaml`
//! - **Infra containers**: Automatically creates pause/infra containers to hold pod namespaces
//! - **Shared network namespace**: All containers in a pod share the same IP and can
//!   communicate via localhost (essential for sidecars)
//! - **Shared volumes**: Containers can mount the same volumes for data sharing
//! - **Atomic lifecycle**: Pod containers start/stop as a unit
//!
//! ## Rootless Mode
//!
//! - Can run without root privileges for improved security
//! - User namespaces for isolation
//! - Compatible with BeeMesh's security model
//!
//! ## Remote API Support
//!
//! - Uses the Podman REST API over Unix domain sockets for efficient communication
//! - Enables distributed pod execution across nodes
//!
//! # Implementation Details
//!
//! ## Pod Naming Convention
//!
//! - Pod ID: UUID v4 (e.g., `d3528d82-0402-471c-aee2-41dba93fc27a`)
//! - Pod name: UUID v4 (Podman may add `-pod` suffix, resulting in max 40 chars)
//! - Uses the same UUID format as tender_id for consistency
//!
//! ## Manifest Modification
//!
//! The engine modifies incoming Kubernetes manifests to:
//! - Set pod name to UUID v4 for DNS compliance and uniqueness
//! - Add resource coordinate labels (namespace, kind, name)
//! - Enable pod identification and cleanup via labels
//!
//! ## API Communication
//!
//! - Uses the Podman REST API over Unix sockets for all operations
//! - Propagates API errors directly so callers can surface actionable diagnostics
//!
//! # Why Not Docker?
//!
//! Docker was removed because it **does not support Kubernetes Pods**:
//! - Docker only runs individual containers
//! - Docker Compose creates separate containers with separate IPs
//! - No shared network namespace (containers can't use localhost to communicate)
//! - No infra/pause container pattern
//! - Cannot properly run sidecar patterns (service mesh, logging agents)
//!
//! # Example Usage
//!
//! ```rust,no_run
//! use machineplane::runtimes::{RuntimeEngine, DeploymentConfig};
//! use machineplane::runtimes::podman::PodmanEngine;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let engine = PodmanEngine::new();
//!
//! // Check if Podman is available
//! if !engine.is_available().await {
//!     eprintln!("Podman not available!");
//!     return Ok(());
//! }
//!
//! // Deploy a Kubernetes Pod manifest
//! let manifest = br#"
//! apiVersion: v1
//! kind: Pod
//! metadata:
//!   name: nginx
//! spec:
//!   containers:
//!   - name: nginx
//!     image: nginx:latest
//!     ports:
//!     - containerPort: 80
//! "#;
//!
//! let config = DeploymentConfig::default();
//! let instance = engine.apply(manifest, &config).await?;
//! println!("Deployed instance: {}", instance.id);
//! # Ok(())
//! # }
//! ```

use crate::runtimes::podman_api::PodmanApiClient;
use crate::runtimes::{
    DeploymentConfig, PortMapping, RuntimeEngine, RuntimeError, RuntimeResult, PodInfo,
    PodStatus,
};
use async_trait::async_trait;
use log::{debug, error, info, warn};
use serde_yaml::{Mapping, Value};
use std::collections::HashMap;
use uuid::Uuid;
use std::sync::{LazyLock, RwLock};

/// Podman runtime engine
pub struct PodmanEngine {
    podman_socket: Option<String>,
}

const LOCAL_PEER_LABEL_KEY: &str = "beemesh.local_peer_id";
const POD_ID_LABEL_KEY: &str = "beemesh.pod_id";
/// K8s-compatible namespace label for filtering pods by namespace (Podman convention).
/// BeeMesh uses this as the canonical namespace label (aliased as beemesh.namespace).
const NAMESPACE_LABEL_KEY: &str = "io.kubernetes.pod.namespace";
/// K8s resource kind label for filtering pods by kind
const KIND_LABEL_KEY: &str = "beemesh.kind";
/// K8s resource name label for filtering pods by name
const NAME_LABEL_KEY: &str = "beemesh.name";
/// K8s-compatible app name label for pod identification
const APP_NAME_LABEL_KEY: &str = "app.kubernetes.io/name";

static PODMAN_SOCKET_OVERRIDE: LazyLock<RwLock<Option<String>>> = LazyLock::new(|| RwLock::new(None));

impl PodmanEngine {
    /// Create a new Podman engine instance
    pub fn new() -> Self {
        Self {
            podman_socket: Self::detect_podman_socket(),
        }
    }

    /// Configure the Podman runtime socket used for Libpod API interactions.
    pub fn configure_runtime(socket: Option<String>) {
        let normalized = socket.and_then(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(Self::normalize_socket(trimmed))
            }
        });

        let mut socket_guard = PODMAN_SOCKET_OVERRIDE
            .write()
            .expect("podman socket override rwlock poisoned");
        *socket_guard = normalized;
    }

    fn socket_override() -> Option<String> {
        PODMAN_SOCKET_OVERRIDE
            .read()
            .expect("podman socket override rwlock poisoned")
            .clone()
    }

    /// Get the currently configured Podman socket path.
    /// Returns the socket URL (with unix:// prefix) if configured.
    pub fn get_socket() -> Option<String> {
        Self::detect_podman_socket()
    }

    pub fn normalize_socket(value: &str) -> String {
        if value.contains("://") {
            value.to_string()
        } else {
            format!("unix://{}", value)
        }
    }

    /// Detect a podman socket URL from configuration or `CONTAINER_HOST`.
    pub fn detect_podman_socket() -> Option<String> {
        if let Some(socket) = Self::socket_override() {
            return Some(socket);
        }

        if let Ok(value) = std::env::var("CONTAINER_HOST") {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(Self::normalize_socket(trimmed));
            }
        }

        None
    }

    /// Create a REST API client for the configured socket
    fn create_api_client(&self) -> Option<PodmanApiClient> {
        self.podman_socket
            .as_ref()
            .map(|socket| PodmanApiClient::new(socket))
    }

    fn require_api_client(&self) -> RuntimeResult<PodmanApiClient> {
        self.create_api_client().ok_or_else(|| {
            RuntimeError::EngineNotAvailable(
                "Podman socket not configured; cannot reach Libpod API".to_string(),
            )
        })
    }

    /// Parse Kubernetes manifest to extract metadata
    fn parse_manifest_metadata(
        &self,
        manifest_content: &[u8],
    ) -> RuntimeResult<HashMap<String, String>> {
        let manifest_str = String::from_utf8_lossy(manifest_content);
        let mut metadata = HashMap::new();

        // Try to parse as YAML
        if let Ok(doc) = serde_yaml::from_str::<Value>(&manifest_str) {
            if let Some(kind) = doc.get("kind").and_then(|k| k.as_str()) {
                metadata.insert("kind".to_string(), kind.to_string());
            }
            if let Some(api_version) = doc.get("apiVersion").and_then(|v| v.as_str()) {
                metadata.insert("apiVersion".to_string(), api_version.to_string());
            }
            if let Some(meta) = doc.get("metadata") {
                if let Some(name) = meta.get("name").and_then(|n| n.as_str()) {
                    metadata.insert("name".to_string(), name.to_string());
                }
                if let Some(namespace) = meta.get("namespace").and_then(|n| n.as_str()) {
                    metadata.insert("namespace".to_string(), namespace.to_string());
                }
            }
        }

        Ok(metadata)
    }

    /// Extract port mappings from pod info
    async fn extract_port_mappings(&self, _pod_name: &str) -> RuntimeResult<Vec<PortMapping>> {
        // Port extraction would require inspecting the pod via API
        // For now, return empty - this matches the original behavior
        Ok(Vec::new())
    }

    /// Generate a unique pod ID for deployment tracking.
    ///
    /// Returns a UUID v4 string (36 chars). With Podman's potential `-pod` suffix,
    /// the final pod name is max 40 chars, well within the 63-char DNS hostname limit.
    ///
    /// Uses the same UUID v4 format as tender_id for consistency across the codebase.
    fn generate_pod_id(&self, _manifest_content: &[u8]) -> String {
        Uuid::new_v4().to_string()
    }

    /// Modify the manifest to set the pod name to our pod ID and optional peer labels
    fn modify_manifest_for_deployment(
        &self,
        manifest_content: &[u8],
        pod_id: &str,
        local_peer_id: Option<&str>,
    ) -> RuntimeResult<Vec<u8>> {
        let manifest_str = String::from_utf8_lossy(manifest_content);
        let mut doc: serde_yaml::Value = serde_yaml::from_str(&manifest_str)
            .map_err(|e| RuntimeError::InvalidManifest(format!("YAML parse error: {}", e)))?;

        // Use the pod_id (UUID v4) as the pod name
        let pod_name = pod_id;

        // Extract namespace from manifest metadata (default to "default")
        let namespace = doc
            .get("metadata")
            .and_then(|m| m.get("namespace"))
            .and_then(|n| n.as_str())
            .unwrap_or("default")
            .to_string();

        // Extract kind from manifest (required field)
        let kind = doc
            .get("kind")
            .and_then(|k| k.as_str())
            .unwrap_or("Pod")
            .to_string();

        // Extract original resource name from manifest metadata
        let resource_name = doc
            .get("metadata")
            .and_then(|m| m.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("unknown")
            .to_string();

        // Extract app name from manifest metadata (use manifest name as fallback)
        let app_name = resource_name.clone();

        // Update metadata name to use our generated pod name
        if let Some(metadata) = doc.get_mut("metadata")
            && let Some(metadata_map) = metadata.as_mapping_mut() {
                metadata_map.insert(
                    serde_yaml::Value::String("name".to_string()),
                    serde_yaml::Value::String(pod_name.to_string()),
                );
                Self::insert_pod_id_label(metadata_map, pod_id);
                Self::insert_namespace_label(metadata_map, &namespace);
                Self::insert_kind_label(metadata_map, &kind);
                Self::insert_name_label(metadata_map, &resource_name);
                Self::insert_app_name_label(metadata_map, &app_name);
                if let Some(peer_id) = local_peer_id {
                    Self::insert_peer_label(metadata_map, peer_id);
                }
            }

        // For Deployments, also update the pod template metadata if it exists
        if let Some(spec) = doc.get_mut("spec")
            && let Some(template) = spec.get_mut("template")
                && let Some(template_metadata) = template.get_mut("metadata")
                    && let Some(template_metadata_map) = template_metadata.as_mapping_mut() {
                        Self::insert_pod_id_label(template_metadata_map, pod_id);
                        Self::insert_namespace_label(template_metadata_map, &namespace);
                        Self::insert_kind_label(template_metadata_map, &kind);
                        Self::insert_name_label(template_metadata_map, &resource_name);
                        Self::insert_app_name_label(template_metadata_map, &app_name);
                        if let Some(peer_id) = local_peer_id {
                            Self::insert_peer_label(template_metadata_map, peer_id);
                        }
                    }

        // Serialize back to YAML
        let modified_manifest = serde_yaml::to_string(&doc)
            .map_err(|e| RuntimeError::InvalidManifest(format!("YAML serialize error: {}", e)))?;

        debug!(
            "Modified manifest for deployment with pod name: {}",
            pod_name
        );
        Ok(modified_manifest.into_bytes())
    }

    fn insert_label(target: &mut Mapping, key: &str, value: &str) {
        let labels_key = serde_yaml::Value::String("labels".to_string());

        if !target.contains_key(&labels_key) {
            target.insert(
                labels_key.clone(),
                serde_yaml::Value::Mapping(Mapping::new()),
            );
        }

        if let Some(labels_value) = target.get_mut(&labels_key)
            && let Some(labels_map) = labels_value.as_mapping_mut() {
                labels_map.insert(
                    serde_yaml::Value::String(key.to_string()),
                    serde_yaml::Value::String(value.to_string()),
                );
            }
    }

    fn insert_peer_label(target: &mut Mapping, peer_id: &str) {
        Self::insert_label(target, LOCAL_PEER_LABEL_KEY, peer_id);
    }

    fn insert_pod_id_label(target: &mut Mapping, pod_id: &str) {
        Self::insert_label(target, POD_ID_LABEL_KEY, pod_id);
    }

    fn insert_namespace_label(target: &mut Mapping, namespace: &str) {
        Self::insert_label(target, NAMESPACE_LABEL_KEY, namespace);
    }

    fn insert_kind_label(target: &mut Mapping, kind: &str) {
        Self::insert_label(target, KIND_LABEL_KEY, kind);
    }

    fn insert_name_label(target: &mut Mapping, name: &str) {
        Self::insert_label(target, NAME_LABEL_KEY, name);
    }

    fn insert_app_name_label(target: &mut Mapping, app_name: &str) {
        Self::insert_label(target, APP_NAME_LABEL_KEY, app_name);
    }

    /// Check if a name looks like a valid beemesh pod_id (UUID format)
    fn pod_id_looks_valid(name: &str) -> bool {
        uuid::Uuid::parse_str(name).is_ok()
    }

    fn pod_status_is_running(status: Option<&str>) -> bool {
        status
            .map(|value| value.to_ascii_lowercase().contains("running"))
            .unwrap_or(false)
    }

    async fn deploy_with_peer_label(
        &self,
        manifest_content: &[u8],
        config: &DeploymentConfig,
        local_peer_id: Option<&str>,
    ) -> RuntimeResult<PodInfo> {
        let _ = config;

        self.validate(manifest_content).await?;

        // Extract resource coordinates from original manifest
        let manifest_str = String::from_utf8_lossy(manifest_content);
        let manifest_doc: serde_yaml::Value = serde_yaml::from_str(&manifest_str)
            .map_err(|e| RuntimeError::InvalidManifest(format!("YAML parse error: {}", e)))?;

        let namespace = manifest_doc
            .get("metadata")
            .and_then(|m| m.get("namespace"))
            .and_then(|n| n.as_str())
            .unwrap_or("default")
            .to_string();

        let kind = manifest_doc
            .get("kind")
            .and_then(|k| k.as_str())
            .unwrap_or("Pod")
            .to_string();

        let resource_name = manifest_doc
            .get("metadata")
            .and_then(|m| m.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("unknown")
            .to_string();

        info!(
            "Deploying pod for {}/{}/{}{}",
            namespace,
            kind,
            resource_name,
            local_peer_id
                .map(|peer| format!(" (local peer: {})", peer))
                .unwrap_or_default()
        );

        let pod_id = self.generate_pod_id(manifest_content);
        let modified_manifest = self.modify_manifest_for_deployment(
            manifest_content,
            &pod_id,
            local_peer_id,
        )?;

        let client = self.require_api_client()?;
        if let Err(e) = self.deploy_via_api(&client, &modified_manifest).await {
            error!("Podman REST API deployment failed: {}", e);
            return Err(e);
        }
        info!("Pod deployed via REST API: {}", pod_id);

        let metadata = self
            .parse_manifest_metadata(manifest_content)
            .unwrap_or_default();

        let ports = self
            .extract_port_mappings(&pod_id)
            .await
            .unwrap_or_default();

        let now = std::time::SystemTime::now();
        Ok(PodInfo {
            id: pod_id,
            namespace,
            kind,
            name: resource_name,
            status: PodStatus::Running,
            metadata,
            created_at: now,
            updated_at: now,
            ports,
        })
    }

    /// Deploy using REST API
    async fn deploy_via_api(
        &self,
        client: &PodmanApiClient,
        manifest_content: &[u8],
    ) -> RuntimeResult<()> {
        debug!("Deploying pod via Podman REST API");

        let response = client.play_kube(manifest_content, true).await?;

        // Check if any pods were created
        if response.pods.is_empty() {
            return Err(RuntimeError::DeploymentFailed(
                "No pods created from manifest".to_string(),
            ));
        }

        // Collect container errors from all pods
        let mut all_container_errors = Vec::new();
        for pod in &response.pods {
            if let Some(id) = &pod.id {
                debug!("Created pod with ID: {}", id);
            }
            if !pod.container_errors.is_empty() {
                all_container_errors.extend(pod.container_errors.clone());
            }
        }

        // If there are container errors, fail the deployment
        // Common causes: pod name > 63 chars (sethostname: Invalid argument)
        if !all_container_errors.is_empty() {
            let error_msg = format!(
                "Container startup failed: {}",
                all_container_errors.join("; ")
            );
            error!("{}", error_msg);
            return Err(RuntimeError::DeploymentFailed(error_msg));
        }

        Ok(())
    }

    /// List pods using REST API
    async fn list_pods_via_api(
        &self,
        client: &PodmanApiClient,
    ) -> RuntimeResult<Vec<PodInfo>> {
        debug!("Listing pods via Podman REST API");

        let pods = client.list_pods().await?;
        let mut pod_infos = Vec::new();

        for pod in pods {
            let pod_id_label = pod
                .labels
                .as_ref()
                .and_then(|labels| labels.get(POD_ID_LABEL_KEY).cloned());
            let pod_name = pod.name.clone();

            // Include pod if it has a beemesh pod_id label or if the name looks like a UUID
            let include_pod = pod_id_label.is_some()
                || pod_name
                    .as_deref()
                    .map(Self::pod_id_looks_valid)
                    .unwrap_or(false);

            if !include_pod {
                continue;
            }

            if let Some(pod_name) = &pod_name {
                // pod_id is either from label or pod_name (which is UUID)
                let pod_id = pod_id_label
                    .clone()
                    .unwrap_or_else(|| pod_name.clone());

                // Parse pod status
                // Podman pod states: Created, Error, Exited, Paused, Running, Degraded, Stopped
                debug!(
                    "Pod {} has status field value: {:?}",
                    pod_name, pod.status
                );
                let status = match pod.status.as_deref() {
                    Some("Running") => PodStatus::Running,
                    // Degraded means at least one container is running, treat as Running
                    Some("Degraded") => PodStatus::Running,
                    Some("Stopped") | Some("Exited") | Some("Paused") => PodStatus::Stopped,
                    // Created means pod exists but hasn't started yet, treat as Starting
                    Some("Created") => PodStatus::Starting,
                    Some("Error") => PodStatus::Failed("Pod in error state".to_string()),
                    other => {
                        warn!(
                            "Pod {} has unexpected status value: {:?}, treating as Unknown",
                            pod_name, other
                        );
                        PodStatus::Unknown
                    }
                };

                // Extract metadata from pod labels if available
                let mut metadata = pod.labels.clone().unwrap_or_default();
                if let Some(peer_id) = metadata.get(LOCAL_PEER_LABEL_KEY).cloned() {
                    metadata.insert("local_peer_id".to_string(), peer_id);
                }

                // Extract resource coordinates from labels
                let namespace = metadata
                    .get(NAMESPACE_LABEL_KEY)
                    .cloned()
                    .unwrap_or_else(|| "default".to_string());
                let kind = metadata
                    .get(KIND_LABEL_KEY)
                    .cloned()
                    .unwrap_or_else(|| "Pod".to_string());
                let name = metadata
                    .get(NAME_LABEL_KEY)
                    .cloned()
                    .unwrap_or_else(|| pod_name.clone());

                let pod_info = PodInfo {
                    id: pod_id,
                    namespace,
                    kind,
                    name,
                    status,
                    metadata,
                    created_at: std::time::SystemTime::now(),
                    updated_at: std::time::SystemTime::now(),
                    ports: Vec::new(),
                };

                debug!(
                    "Found beemesh pod: {} (pod: {})",
                    pod_info.id, pod_name
                );
                pod_infos.push(pod_info);
            }
        }

        debug!("Found {} pods via API", pod_infos.len());
        Ok(pod_infos)
    }

    /// Remove pod using REST API
    async fn remove_pod_via_api(
        &self,
        client: &PodmanApiClient,
        pod_id: &str,
    ) -> RuntimeResult<()> {
        debug!("Removing pod via Podman REST API: {}", pod_id);

        // Prefer precise deletion using label filters so we don't depend on guessed names.
        let mut removed_by_label = 0usize;
        match client
            .list_pods_by_label(POD_ID_LABEL_KEY, pod_id)
            .await
        {
            Ok(pods) => {
                for pod in pods {
                    if let Some(name) = pod.name.as_deref() {
                        match client.remove_pod(name, true).await {
                            Ok(_) => {
                                info!(
                                    "Successfully removed labeled pod {} for pod_id {}",
                                    name, pod_id
                                );
                                removed_by_label += 1;
                            }
                            Err(err) => {
                                warn!(
                                    "Failed to remove labeled pod {} for pod_id {}: {}",
                                    name, pod_id, err
                                );
                            }
                        }
                    }
                }
            }
            Err(err) => {
                warn!(
                    "Failed to list pods for pod_id {} using labels: {}",
                    pod_id, err
                );
            }
        }

        if removed_by_label > 0 {
            return Ok(());
        }

        // Fallback: try exact pod_id as name (pod_id is UUID, used as pod name)
        if client.remove_pod(pod_id, true).await.is_ok() {
            info!("Successfully removed pod: {}", pod_id);
        }

        Ok(())
    }

    /// Generate kube manifest using REST API
    async fn generate_kube_via_api(
        &self,
        client: &PodmanApiClient,
        pod_id: &str,
    ) -> RuntimeResult<Vec<u8>> {
        debug!("Generating kube manifest via REST API for: {}", pod_id);
        let pods = client
            .list_pods_by_label(POD_ID_LABEL_KEY, pod_id)
            .await?;

        if pods.is_empty() {
            return Err(RuntimeError::InstanceNotFound(format!(
                "No Podman pod found with label {}={}",
                POD_ID_LABEL_KEY, pod_id
            )));
        }

        let mut last_status: Option<String> = None;
        let mut generation_error: Option<RuntimeError> = None;

        for pod in pods {
            if let Some(status) = pod.status.clone() {
                last_status = Some(status);
            }

            if !Self::pod_status_is_running(pod.status.as_deref()) {
                continue;
            }

            if let Some(name) = pod.name.as_deref() {
                match client.generate_kube(name).await {
                    Ok(manifest) => {
                        info!("Generated manifest for running pod {}", name);
                        return Ok(manifest.into_bytes());
                    }
                    Err(err) => {
                        warn!(
                            "Failed to generate manifest from pod {} for pod_id {}: {}",
                            name, pod_id, err
                        );
                        generation_error = Some(err);
                    }
                }
            }
        }

        if let Some(err) = generation_error {
            return Err(err);
        }

        Err(RuntimeError::InstanceNotReady(format!(
            "Pod {} has Podman pods but none are running yet (last status: {})",
            pod_id,
            last_status.unwrap_or_else(|| "unknown".to_string())
        )))
    }

    /// Get pod logs using REST API
    async fn get_logs_via_api(
        &self,
        client: &PodmanApiClient,
        pod_id: &str,
        tail: Option<usize>,
    ) -> RuntimeResult<String> {
        debug!("Getting logs via REST API for: {}", pod_id);

        client.get_pod_logs(pod_id, tail).await
    }
}

impl Default for PodmanEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl RuntimeEngine for PodmanEngine {
    fn name(&self) -> &str {
        "podman"
    }

    async fn is_available(&self) -> bool {
        let Some(client) = self.create_api_client() else {
            debug!("Podman socket not configured; runtime unavailable");
            return false;
        };

        match client.check_availability().await {
            Ok(info) => {
                if let Some(version) = info.version {
                    debug!("Podman API available, version: {:?}", version.version);
                }
                true
            }
            Err(e) => {
                error!("Podman REST API unavailable: {}", e);
                false
            }
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn validate(&self, manifest_content: &[u8]) -> RuntimeResult<()> {
        let manifest_str = String::from_utf8_lossy(manifest_content);

        // Basic YAML validation
        match serde_yaml::from_str::<Value>(&manifest_str) {
            Ok(doc) => {
                // Check for required Kubernetes fields
                if doc.get("apiVersion").is_none() {
                    return Err(RuntimeError::InvalidManifest(
                        "Missing apiVersion field".to_string(),
                    ));
                }
                if doc.get("kind").is_none() {
                    return Err(RuntimeError::InvalidManifest(
                        "Missing kind field".to_string(),
                    ));
                }

                info!("Manifest validation passed");
                Ok(())
            }
            Err(e) => Err(RuntimeError::InvalidManifest(format!(
                "YAML parse error: {}",
                e
            ))),
        }
    }

    async fn apply(
        &self,
        manifest_content: &[u8],
        config: &DeploymentConfig,
    ) -> RuntimeResult<PodInfo> {
        self.deploy_with_peer_label(manifest_content, config, None)
            .await
    }

    async fn get_status(&self, pod_id: &str) -> RuntimeResult<PodInfo> {
        debug!("Getting status for pod: {}", pod_id);

        // List all pods and find the matching one
        let pods = self.list().await?;

        for pod in pods {
            if pod.id == pod_id {
                return Ok(pod);
            }
        }

        // Return a basic status if not found
        let now = std::time::SystemTime::now();
        Ok(PodInfo {
            id: pod_id.to_string(),
            namespace: "default".to_string(),
            kind: "Pod".to_string(),
            name: "unknown".to_string(),
            status: PodStatus::Unknown,
            metadata: HashMap::new(),
            created_at: now,
            updated_at: now,
            ports: Vec::new(),
        })
    }

    async fn list(&self) -> RuntimeResult<Vec<PodInfo>> {
        debug!("Listing all pods");
        let client = self.require_api_client()?;
        self.list_pods_via_api(&client).await
    }

    async fn delete(&self, pod_id: &str) -> RuntimeResult<()> {
        info!("Deleting pod: {}", pod_id);
        let client = self.require_api_client()?;
        self.remove_pod_via_api(&client, pod_id).await
    }

    async fn logs(
        &self,
        pod_id: &str,
        tail: Option<usize>,
    ) -> RuntimeResult<String> {
        debug!("Getting logs for pod: {}", pod_id);
        let client = self.require_api_client()?;
        self.get_logs_via_api(&client, pod_id, tail).await
    }

    async fn export(&self, pod_id: &str) -> RuntimeResult<Vec<u8>> {
        info!("Exporting manifest for pod: {}", pod_id);
        let client = self.require_api_client()?;
        self.generate_kube_via_api(&client, pod_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::ffi::OsString;

    /// RAII guard for safely modifying environment variables in tests.
    ///
    /// # Safety
    ///
    /// All tests using `EnvGuard` must be marked with `#[serial]` to ensure
    /// sequential execution. This prevents data races from concurrent env access.
    struct EnvGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var_os(key);
            // SAFETY: Test is #[serial], ensuring no concurrent env access.
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: Test is #[serial], ensuring no concurrent env access.
            // Restoring original value maintains test isolation.
            if let Some(value) = &self.previous {
                unsafe {
                    std::env::set_var(self.key, value);
                }
            } else {
                unsafe {
                    std::env::remove_var(self.key);
                }
            }
        }
    }

    #[serial]
    #[tokio::test]
    async fn test_podman_engine_creation() {
        PodmanEngine::configure_runtime(None);
        let engine = PodmanEngine::new();
        assert_eq!(engine.name(), "podman");
    }

    #[serial]
    #[tokio::test]
    async fn test_manifest_validation() {
        PodmanEngine::configure_runtime(None);
        let engine = PodmanEngine::new();

        // Valid manifest
        let valid_manifest = br#"
apiVersion: v1
kind: Pod
metadata:
  name: test-pod
spec:
  containers:
  - name: nginx
    image: nginx:latest
"#;

        assert!(engine.validate(valid_manifest).await.is_ok());

        // Invalid manifest (missing apiVersion)
        let invalid_manifest = br#"
kind: Pod
metadata:
  name: test-pod
spec:
  containers:
  - name: nginx
    image: nginx:latest
"#;

        assert!(engine.validate(invalid_manifest).await.is_err());
    }

    #[serial]
    #[tokio::test]
    async fn test_parse_manifest_metadata() {
        PodmanEngine::configure_runtime(None);
        let engine = PodmanEngine::new();

        let manifest = br#"
apiVersion: v1
kind: Pod
metadata:
  name: test-pod
  namespace: default
spec:
  containers:
  - name: nginx
    image: nginx:latest
"#;

        let metadata = engine.parse_manifest_metadata(manifest).unwrap();
        assert_eq!(metadata.get("kind"), Some(&"Pod".to_string()));
        assert_eq!(metadata.get("apiVersion"), Some(&"v1".to_string()));
        assert_eq!(metadata.get("name"), Some(&"test-pod".to_string()));
        assert_eq!(metadata.get("namespace"), Some(&"default".to_string()));
    }

    #[test]
    fn test_manifest_modification_inserts_pod_id_label() {
        let engine = PodmanEngine::new();
        let manifest = include_bytes!("../../tests/sample_manifests/nginx.yml");

        let pod_id = "manifest-123-deadbeef";
        let modified = engine
            .modify_manifest_for_deployment(manifest, pod_id, None)
            .expect("manifest modification must succeed");

        let doc: serde_yaml::Value = serde_yaml::from_slice(&modified).unwrap();
        let labels = doc["metadata"]["labels"].as_mapping().unwrap();
        assert_eq!(
            labels
                .get(&serde_yaml::Value::String(
                    POD_ID_LABEL_KEY.to_string()
                ))
                .and_then(|value| value.as_str()),
            Some(pod_id)
        );
    }

    #[test]
    fn test_deployment_template_receives_pod_id_label() {
        let engine = PodmanEngine::new();
        let manifest = include_bytes!("../../tests/sample_manifests/nginx.yml");

        let pod_id = "manifest-abc-cafebabe";
        let modified = engine
            .modify_manifest_for_deployment(manifest, pod_id, None)
            .expect("manifest modification must succeed");

        let doc: serde_yaml::Value = serde_yaml::from_slice(&modified).unwrap();
        let labels = doc["spec"]["template"]["metadata"]["labels"]
            .as_mapping()
            .unwrap();
        assert_eq!(
            labels
                .get(&serde_yaml::Value::String(
                    POD_ID_LABEL_KEY.to_string()
                ))
                .and_then(|value| value.as_str()),
            Some(pod_id)
        );
    }

    #[serial]
    #[tokio::test]
    async fn test_pod_id_generation() {
        PodmanEngine::configure_runtime(None);
        let engine = PodmanEngine::new();

        let manifest1 = b"apiVersion: v1\nkind: Pod\nmetadata:\n  name: test-pod";
        let manifest2 = b"apiVersion: v1\nkind: Pod\nmetadata:\n  name: different-pod";
        let manifest_without_name = b"apiVersion: v1\nkind: Pod";

        let id1 = engine.generate_pod_id(manifest1);
        let id2 = engine.generate_pod_id(manifest1);
        let id3 = engine.generate_pod_id(manifest2);
        let id4 = engine.generate_pod_id(manifest_without_name);

        // IDs should be valid UUID v4 format (36 chars with hyphens)
        assert_eq!(id1.len(), 36, "UUID should be 36 chars: {}", id1);
        assert_eq!(id3.len(), 36, "UUID should be 36 chars: {}", id3);
        assert_eq!(id4.len(), 36, "UUID should be 36 chars: {}", id4);

        // Each call should produce a unique ID (random UUID)
        assert_ne!(id1, id2, "Same manifest should produce different IDs");
        assert_ne!(id1, id3, "Different manifests should produce different IDs");

        // Should be parseable as UUID
        assert!(
            uuid::Uuid::parse_str(&id1).is_ok(),
            "ID should be valid UUID: {}",
            id1
        );
        assert!(
            uuid::Uuid::parse_str(&id3).is_ok(),
            "ID should be valid UUID: {}",
            id3
        );
    }

    #[serial]
    #[tokio::test]
    async fn detects_podman_socket_from_env() {
        PodmanEngine::configure_runtime(None);

        let _guard = EnvGuard::set("CONTAINER_HOST", "/tmp/env-podman.sock");

        let engine = PodmanEngine::new();
        assert_eq!(
            engine.podman_socket.as_deref(),
            Some("unix:///tmp/env-podman.sock")
        );

        PodmanEngine::configure_runtime(None);
    }
}
