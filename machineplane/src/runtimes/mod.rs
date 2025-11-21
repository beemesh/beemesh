//! Runtime engine abstraction for container workload execution
//!
//! # Overview
//!
//! This module provides the runtime layer for deploying and managing containerized workloads
//! in BeeMesh. It abstracts over different container runtime engines to execute Kubernetes
//! manifests (Pods, Deployments, etc.) on cluster nodes.
//!
//! # Critical Requirement: Pod Support
//!
//! **IMPORTANT**: Any runtime engine added to this module MUST support Kubernetes Pods natively.
//!
//! ## Why Pods Are Required
//!
//! Kubernetes Pods are NOT just "multiple containers" - they are a fundamental primitive with
//! specific semantics that cannot be emulated with simple container orchestration:
//!
//! - **Shared Network Namespace**: All containers in a pod share the same IP address and can
//!   communicate via localhost. This is essential for sidecar patterns (service mesh, logging).
//! - **Shared Storage Volumes**: Containers can share data via mounted volumes.
//! - **Infra/Pause Container**: A special container holds the pod's namespaces even when
//!   application containers restart.
//! - **Atomic Lifecycle**: All containers in a pod are scheduled together on the same node
//!   and share the same lifecycle (start, stop, restart as a unit).
//!
//! ## Supported Runtimes
//!
//! ### ✅ Podman (Current Default)
//! - Native Kubernetes pod support via `podman kube play`
//! - Creates pods with infra containers automatically
//! - Full support for multi-container pods, sidecars, shared namespaces
//! - Can run rootless for improved security
//!
//! ### ❌ Docker (Removed - Does NOT Support Pods)
//! - Docker only supports individual containers, not pods
//! - Docker Compose creates separate containers with separate network namespaces
//! - Cannot implement true pod semantics without significant complexity
//! - **DO NOT re-add Docker support** unless it gains native pod support
//!
//! ### 🔮 Future Options (Not Yet Implemented)
//! - **CRI-O**: Native Kubernetes runtime, excellent pod support
//! - **containerd**: With CRI plugin, full pod support
//! - **Firecracker/Kata**: For VM-based isolation with pod semantics
//!
//! ## Adding New Runtime Engines
//!
//! Before adding a new runtime engine, verify it supports:
//!
//! 1. ✅ Kubernetes Pod API (not just containers)
//! 2. ✅ Shared network namespace between containers
//! 3. ✅ Infra/pause container pattern
//! 4. ✅ Can parse and execute Kubernetes YAML manifests
//!
//! If a runtime only supports individual containers (like Docker), it is **NOT suitable**
//! for BeeMesh and should not be added.

use async_trait::async_trait;
#[cfg(not(debug_assertions))]
use log::warn;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use thiserror::Error;

#[cfg(debug_assertions)]
pub mod mock;
pub mod podman;

/// Configure the Podman runtime using CLI-provided settings.
pub fn configure_podman_runtime(socket: Option<String>) {
    let force_remote = socket.is_some();
    podman::PodmanEngine::configure_runtime(socket, force_remote);
}

/// Errors that can occur during runtime operations
#[derive(Error, Debug)]
pub enum RuntimeError {
    #[error("Runtime engine not available: {0}")]
    EngineNotAvailable(String),

    #[error("Invalid manifest format: {0}")]
    InvalidManifest(String),

    #[error("Deployment failed: {0}")]
    DeploymentFailed(String),

    #[error("Workload not found: {0}")]
    WorkloadNotFound(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Command execution failed: {0}")]
    CommandFailed(String),
}

/// Result type for runtime operations
pub type RuntimeResult<T> = Result<T, RuntimeError>;

/// Status of a deployed workload
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum WorkloadStatus {
    /// Workload is starting up
    Starting,
    /// Workload is running successfully
    Running,
    /// Workload has stopped
    Stopped,
    /// Workload failed to start or crashed
    Failed(String),
    /// Status is unknown
    Unknown,
}

/// Information about a deployed workload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadInfo {
    /// Unique identifier for this workload instance
    pub id: String,
    /// The manifest ID this workload was created from
    pub manifest_id: String,
    /// Current status of the workload
    pub status: WorkloadStatus,
    /// Metadata associated with the workload
    pub metadata: HashMap<String, String>,
    /// When the workload was created
    pub created_at: std::time::SystemTime,
    /// When the workload was last updated
    pub updated_at: std::time::SystemTime,
    /// Exposed ports (if any)
    pub ports: Vec<PortMapping>,
}

/// Port mapping for a workload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortMapping {
    /// Container/internal port
    pub container_port: u16,
    /// Host port (if exposed)
    pub host_port: Option<u16>,
    /// Protocol (tcp, udp)
    pub protocol: String,
}

/// Configuration for deploying a workload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentConfig {
    /// Number of replicas to deploy
    pub replicas: u32,
    /// Resource limits
    pub resources: ResourceLimits,
    /// Environment variables
    pub env: HashMap<String, String>,
    /// Additional runtime-specific options
    pub runtime_options: HashMap<String, String>,
}

/// Resource limits for a workload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// CPU limit (in cores, e.g., 1.5)
    pub cpu: Option<f64>,
    /// Memory limit (in bytes)
    pub memory: Option<u64>,
    /// Storage limit (in bytes)
    pub storage: Option<u64>,
}

impl Default for DeploymentConfig {
    fn default() -> Self {
        Self {
            replicas: 1,
            resources: ResourceLimits {
                cpu: None,
                memory: None,
                storage: None,
            },
            env: HashMap::new(),
            runtime_options: HashMap::new(),
        }
    }
}

/// Trait that all runtime engines must implement
///
/// # Pod Support Requirement
///
/// **CRITICAL**: Implementations of this trait MUST support Kubernetes Pods, not just
/// individual containers. This means:
///
/// - Ability to deploy multi-container pods with shared network namespace
/// - Support for infra/pause containers to hold pod namespaces
/// - Proper handling of pod lifecycle (all containers start/stop together)
/// - Shared volumes between containers in the same pod
///
/// ## Why This Matters
///
/// BeeMesh uses Kubernetes manifests (Pods, Deployments) as its workload definition format.
/// These manifests often include:
/// - Sidecar containers (service mesh proxies, log collectors)
/// - Init containers (setup tasks before main container starts)
/// - Shared volumes for inter-container communication
///
/// A runtime that only supports individual containers (like Docker) **cannot properly
/// execute these workloads** because it lacks the pod abstraction.
///
/// ## Verification
///
/// Before implementing this trait for a new runtime, verify:
/// 1. Can it run `kubectl apply -f pod.yaml` or equivalent?
/// 2. Do containers in a pod share localhost networking?
/// 3. Does it create an infra/pause container?
/// 4. Can it handle Kubernetes YAML manifests directly?
///
/// If the answer to any of these is "no", the runtime is not suitable for BeeMesh.
#[async_trait]
pub trait RuntimeEngine: Send + Sync {
    /// Returns the name of this runtime engine
    fn name(&self) -> &str;

    /// Check if this runtime engine is available on the system
    async fn is_available(&self) -> bool;

    /// Enable downcasting to concrete types (for testing)
    fn as_any(&self) -> &dyn std::any::Any;

    /// Deploy a workload from a manifest
    ///
    /// # Arguments
    /// * `manifest_id` - Unique identifier for the manifest
    /// * `manifest_content` - The raw manifest content (YAML, JSON, etc.)
    /// * `config` - Deployment configuration
    ///
    /// # Returns
    /// * `WorkloadInfo` - Information about the deployed workload
    async fn deploy_workload(
        &self,
        manifest_id: &str,
        manifest_content: &[u8],
        config: &DeploymentConfig,
    ) -> RuntimeResult<WorkloadInfo>;

    /// Get status of a deployed workload
    async fn get_workload_status(&self, workload_id: &str) -> RuntimeResult<WorkloadInfo>;

    /// List all workloads managed by this engine
    async fn list_workloads(&self) -> RuntimeResult<Vec<WorkloadInfo>>;

    /// Stop and remove a workload
    async fn remove_workload(&self, workload_id: &str) -> RuntimeResult<()>;

    /// Get logs from a workload
    async fn get_workload_logs(
        &self,
        workload_id: &str,
        tail: Option<usize>,
    ) -> RuntimeResult<String>;

    /// Validate a manifest before deployment
    async fn validate_manifest(&self, manifest_content: &[u8]) -> RuntimeResult<()>;

    /// Export/generate a Kubernetes manifest from a running workload
    /// This is useful for debugging and testing to see the actual runtime state
    /// as a Kubernetes manifest.
    ///
    /// # Arguments
    /// * `workload_id` - The unique identifier of the running workload
    ///
    /// # Returns
    /// * The generated Kubernetes manifest as YAML bytes
    async fn export_manifest(&self, workload_id: &str) -> RuntimeResult<Vec<u8>>;

    /// Deploy a workload with local peer ID tracking
    async fn deploy_workload_with_peer(
        &self,
        manifest_id: &str,
        manifest_content: &[u8],
        config: &DeploymentConfig,
        local_peer_id: libp2p::PeerId,
    ) -> RuntimeResult<WorkloadInfo> {
        let mut workload_info = self
            .deploy_workload(manifest_id, manifest_content, config)
            .await?;
        // Add local peer ID to metadata
        workload_info
            .metadata
            .insert("local_peer_id".to_string(), local_peer_id.to_string());
        Ok(workload_info)
    }

    /// List workloads deployed by a specific local peer ID
    async fn list_workloads_by_peer(
        &self,
        local_peer_id: &str,
    ) -> RuntimeResult<Vec<WorkloadInfo>> {
        let all_workloads = self.list_workloads().await?;
        let filtered_workloads = all_workloads
            .into_iter()
            .filter(|workload| {
                workload
                    .metadata
                    .get("local_peer_id")
                    .map(|peer_id| peer_id == local_peer_id)
                    .unwrap_or(false)
            })
            .collect();
        Ok(filtered_workloads)
    }
}

/// Registry for managing multiple runtime engines
pub struct RuntimeRegistry {
    engines: HashMap<String, Box<dyn RuntimeEngine>>,
    default_engine: Option<String>,
}

impl RuntimeRegistry {
    /// Create a new runtime registry
    pub fn new() -> Self {
        Self {
            engines: HashMap::new(),
            default_engine: None,
        }
    }

    /// Register a new runtime engine
    pub fn register(&mut self, engine: Box<dyn RuntimeEngine>) {
        let name = engine.name().to_string();
        self.engines.insert(name.clone(), engine);

        // Set as default if it's the first engine
        if self.default_engine.is_none() {
            self.default_engine = Some(name);
        }
    }

    /// Get a runtime engine by name
    pub fn get_engine(&self, name: &str) -> Option<&dyn RuntimeEngine> {
        self.engines.get(name).map(|e| e.as_ref())
    }

    /// Get a mutable reference to a runtime engine by name (for testing)
    #[cfg(test)]
    pub fn get_engine_mut(&mut self, name: &str) -> Option<&mut Box<dyn RuntimeEngine>> {
        self.engines.get_mut(name)
    }

    /// Get the default runtime engine
    pub fn get_default_engine(&self) -> Option<&dyn RuntimeEngine> {
        self.default_engine
            .as_ref()
            .and_then(|name| self.get_engine(name))
    }

    /// Set the default runtime engine
    pub fn set_default_engine(&mut self, name: &str) -> RuntimeResult<()> {
        if self.engines.contains_key(name) {
            self.default_engine = Some(name.to_string());
            Ok(())
        } else {
            Err(RuntimeError::EngineNotAvailable(name.to_string()))
        }
    }

    /// List all registered engines
    pub fn list_engines(&self) -> Vec<&str> {
        self.engines.keys().map(|s| s.as_str()).collect()
    }

    /// Check which engines are available on the system
    pub async fn check_available_engines(&self) -> HashMap<String, bool> {
        let mut results = HashMap::new();
        for (name, engine) in &self.engines {
            results.insert(name.clone(), engine.is_available().await);
        }
        results
    }
}

impl Default for RuntimeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Create a default runtime registry with Podman engine
///
/// Only Podman is currently supported as it provides native Kubernetes pod support
/// via `podman kube play`. Docker does not support pods (only individual containers),
/// so it cannot run Kubernetes manifests properly.
///
/// Future runtime engines like CRI-O may be added when needed.
pub async fn create_default_registry() -> RuntimeRegistry {
    let mut registry = RuntimeRegistry::new();

    // Register Podman engine (only runtime that supports K8s pods)
    registry.register(Box::new(podman::PodmanEngine::new()));

    // Register mock engine for testing and debug builds only
    #[cfg(debug_assertions)]
    {
        registry.register(Box::new(mock::MockEngine::new()));
    }

    // Try to set the best available engine as default
    let available = registry.check_available_engines().await;

    // Prefer Podman if available, otherwise mock for testing
    if *available.get("podman").unwrap_or(&false) {
        let _ = registry.set_default_engine("podman");
    } else {
        #[cfg(debug_assertions)]
        {
            let _ = registry.set_default_engine("mock");
        }
        #[cfg(not(debug_assertions))]
        {
            log::warn!("Podman not available and no fallback runtime configured");
        }
    }

    registry
}

/// Create a mock-only runtime registry for testing
/// This registry only contains the MockEngine, useful for integration tests
/// where we want to verify manifest application without real containers
#[cfg(debug_assertions)]
pub async fn create_mock_only_registry() -> RuntimeRegistry {
    let mut registry = RuntimeRegistry::new();

    // Register only the mock engine for testing
    registry.register(Box::new(mock::MockEngine::new()));

    // Set mock as the default engine
    let _ = registry.set_default_engine("mock");

    registry
}

/// Fallback mock-only registry accessor for release builds where the mock engine
/// is not compiled in. We return the default registry to ensure callers have a
/// functional runtime configuration without linking the mock implementation.
#[cfg(not(debug_assertions))]
pub async fn create_mock_only_registry() -> RuntimeRegistry {
    warn!("mock runtime requested in release build; returning default registry");
    create_default_registry().await
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_runtime_registry() {
        let mut registry = RuntimeRegistry::new();

        // Register mock engine
        registry.register(Box::new(mock::MockEngine::new()));

        // Check that engine is registered
        assert!(registry.get_engine("mock").is_some());
        assert_eq!(registry.get_default_engine().unwrap().name(), "mock");

        // List engines
        let engines = registry.list_engines();
        assert!(engines.contains(&"mock"));
    }

    #[tokio::test]
    async fn test_default_deployment_config() {
        let config = DeploymentConfig::default();
        assert_eq!(config.replicas, 1);
        assert!(config.resources.cpu.is_none());
        assert!(config.env.is_empty());
    }
}
