//! Workload Manager
//!
//! This module provides the main interface for managing workloads in the machine plane.
//! It integrates runtime engines (Podman, Docker, etc.) with provider announcement
//! and discovery systems to provide a unified workload management experience.

use crate::provider::{ProviderConfig, ProviderManager};
use crate::runtime::{create_default_registry, DeploymentConfig, RuntimeRegistry, WorkloadStatus};
use libp2p::{PeerId, Swarm};
use log::{debug, error, info, warn};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::SystemTime;
use thiserror::Error;
use tokio::sync::mpsc;

use crate::libp2p_beemesh::behaviour::MyBehaviour;

/// Errors that can occur during workload management
#[derive(Error, Debug)]
pub enum WorkloadManagerError {
    #[error("Runtime error: {0}")]
    RuntimeError(#[from] crate::runtime::RuntimeError),

    #[error("Provider error: {0}")]
    ProviderError(#[from] crate::provider::ProviderError),

    #[error("Workload not found: {0}")]
    WorkloadNotFound(String),

    #[error("Manifest validation failed: {0}")]
    ManifestValidationFailed(String),

    #[error("No runtime engine available")]
    NoRuntimeEngineAvailable,

    #[error("Network error: {0}")]
    NetworkError(String),
}

pub type WorkloadManagerResult<T> = Result<T, WorkloadManagerError>;

/// Configuration for the workload manager
#[derive(Debug, Clone)]
pub struct WorkloadManagerConfig {
    /// Whether to announce this node as a provider when deploying workloads
    pub announce_as_provider: bool,
    /// TTL for provider announcements (in seconds)
    pub provider_ttl_seconds: u64,
    /// Preferred runtime engine (if None, will use default)
    pub preferred_runtime: Option<String>,
    /// Whether to validate manifests before deployment
    pub validate_manifests: bool,
    /// Maximum number of concurrent deployments
    pub max_concurrent_deployments: usize,
}

impl Default for WorkloadManagerConfig {
    fn default() -> Self {
        Self {
            announce_as_provider: true,
            provider_ttl_seconds: 3600, // 1 hour
            preferred_runtime: None,
            validate_manifests: true,
            max_concurrent_deployments: 10,
        }
    }
}

/// Status of a workload deployment
#[derive(Debug, Clone)]
pub struct DeploymentStatus {
    /// The workload ID
    pub workload_id: String,
    /// The manifest ID this workload was deployed from
    pub manifest_id: String,
    /// Current status
    pub status: WorkloadStatus,
    /// Which runtime engine is managing this workload
    pub runtime_engine: String,
    /// Whether this node announced itself as a provider
    pub announced_as_provider: bool,
    /// When the deployment was created
    pub deployed_at: SystemTime,
    /// Last status update
    pub last_updated: SystemTime,
    /// Deployment configuration used
    pub config: DeploymentConfig,
}

/// Main workload manager that coordinates runtime engines and provider announcements
pub struct WorkloadManager {
    /// Registry of available runtime engines
    runtime_registry: Arc<RwLock<RuntimeRegistry>>,
    /// Provider manager for announcements and discovery
    provider_manager: Arc<ProviderManager>,
    /// Currently deployed workloads
    deployments: Arc<RwLock<HashMap<String, DeploymentStatus>>>,
    /// Configuration
    config: WorkloadManagerConfig,
    /// Channel for deployment events
    event_sender: Option<mpsc::UnboundedSender<WorkloadEvent>>,
}

/// Events that can occur during workload management
#[derive(Debug, Clone)]
pub enum WorkloadEvent {
    /// A workload was successfully deployed
    WorkloadDeployed {
        workload_id: String,
        manifest_id: String,
        runtime_engine: String,
    },
    /// A workload deployment failed
    DeploymentFailed { manifest_id: String, error: String },
    /// A workload was removed
    WorkloadRemoved {
        workload_id: String,
        manifest_id: String,
    },
    /// Provider announcement was made
    ProviderAnnounced {
        manifest_id: String,
        peer_id: PeerId,
    },
    /// Provider announcement was stopped
    ProviderStopped { manifest_id: String },
}

impl WorkloadManager {
    /// Create a new workload manager
    pub async fn new(config: WorkloadManagerConfig) -> WorkloadManagerResult<Self> {
        info!("Creating workload manager with config: {:?}", config);

        // Create runtime registry
        let runtime_registry = create_default_registry().await;

        // Check if any runtime engines are available
        let available_engines = runtime_registry.check_available_engines().await;
        let has_available_engine = available_engines.values().any(|&available| available);

        if !has_available_engine {
            warn!("No runtime engines are available on this system");
            // In testing or development, this might be okay (mock engine)
        }

        info!("Available runtime engines: {:?}", available_engines);

        // Create provider manager
        let provider_config = ProviderConfig {
            default_ttl_seconds: config.provider_ttl_seconds,
            ..Default::default()
        };
        let provider_manager = ProviderManager::new(provider_config);

        Ok(Self {
            runtime_registry: Arc::new(RwLock::new(runtime_registry)),
            provider_manager: Arc::new(provider_manager),
            deployments: Arc::new(RwLock::new(HashMap::new())),
            config,
            event_sender: None,
        })
    }

    /// Create a workload manager with default configuration
    pub async fn new_default() -> WorkloadManagerResult<Self> {
        Self::new(WorkloadManagerConfig::default()).await
    }

    /// Set an event channel to receive workload events
    pub fn set_event_channel(&mut self, sender: mpsc::UnboundedSender<WorkloadEvent>) {
        self.event_sender = Some(sender);
    }

    /// Deploy a workload from a manifest
    pub async fn deploy_workload(
        &self,
        swarm: &mut Swarm<MyBehaviour>,
        manifest_id: &str,
        manifest_content: &[u8],
        config: &DeploymentConfig,
    ) -> WorkloadManagerResult<DeploymentStatus> {
        info!(
            "Deploying workload for manifest_id: {} (manifest size: {} bytes)",
            manifest_id,
            manifest_content.len()
        );

        // Validate manifest if configured
        if self.config.validate_manifests {
            self.validate_manifest_content(manifest_content).await?;
        }

        // Get the runtime engine to use
        let engine_name = self.get_runtime_engine().await?;

        info!(
            "Using runtime engine '{}' for manifest_id: {}",
            engine_name, manifest_id
        );

        // Deploy using the runtime engine
        let workload_info = {
            let registry = self.runtime_registry.read().unwrap();
            let engine = registry
                .get_engine(&engine_name)
                .ok_or(WorkloadManagerError::NoRuntimeEngineAvailable)?;
            engine
                .deploy_workload(manifest_id, manifest_content, config)
                .await?
        };

        info!(
            "Successfully deployed workload {} for manifest_id: {} using engine '{}'",
            workload_info.id, manifest_id, engine_name
        );

        // Create deployment status
        let deployment_status = DeploymentStatus {
            workload_id: workload_info.id.clone(),
            manifest_id: manifest_id.to_string(),
            status: workload_info.status,
            runtime_engine: engine_name.clone(),
            announced_as_provider: false,
            deployed_at: workload_info.created_at,
            last_updated: workload_info.updated_at,
            config: config.clone(),
        };

        // Store deployment
        {
            let mut deployments = self.deployments.write().unwrap();
            deployments.insert(workload_info.id.clone(), deployment_status.clone());
        }

        // Announce as provider if configured
        let mut final_status = deployment_status;
        if self.config.announce_as_provider {
            match self
                .announce_provider(swarm, manifest_id, &engine_name)
                .await
            {
                Ok(()) => {
                    final_status.announced_as_provider = true;
                    info!(
                        "Announced as provider for manifest_id: {} after successful deployment",
                        manifest_id
                    );

                    // Send event
                    if let Some(ref sender) = self.event_sender {
                        let _ = sender.send(WorkloadEvent::ProviderAnnounced {
                            manifest_id: manifest_id.to_string(),
                            peer_id: *swarm.local_peer_id(),
                        });
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to announce as provider for manifest_id {}: {}",
                        manifest_id, e
                    );
                }
            }
        }

        // Send deployment event
        if let Some(ref sender) = self.event_sender {
            let _ = sender.send(WorkloadEvent::WorkloadDeployed {
                workload_id: workload_info.id.clone(),
                manifest_id: manifest_id.to_string(),
                runtime_engine: engine_name,
            });
        }

        Ok(final_status)
    }

    /// Remove a deployed workload
    pub async fn remove_workload(
        &self,
        _swarm: &mut Swarm<MyBehaviour>,
        workload_id: &str,
    ) -> WorkloadManagerResult<()> {
        info!("Removing workload: {}", workload_id);

        // Get deployment info
        let deployment_status = {
            let deployments = self.deployments.read().unwrap();
            deployments
                .get(workload_id)
                .cloned()
                .ok_or_else(|| WorkloadManagerError::WorkloadNotFound(workload_id.to_string()))?
        };

        // Remove from runtime engine
        {
            let registry = self.runtime_registry.read().unwrap();
            let engine = registry
                .get_engine(&deployment_status.runtime_engine)
                .ok_or(WorkloadManagerError::NoRuntimeEngineAvailable)?;
            engine.remove_workload(workload_id).await?;
        }

        info!(
            "Successfully removed workload {} from runtime engine '{}'",
            workload_id, deployment_status.runtime_engine
        );

        // Stop provider announcement if we were announcing
        if deployment_status.announced_as_provider {
            match self
                .provider_manager
                .stop_providing(&deployment_status.manifest_id)
            {
                Ok(()) => {
                    info!(
                        "Stopped providing manifest_id: {} after workload removal",
                        deployment_status.manifest_id
                    );

                    // Send event
                    if let Some(ref sender) = self.event_sender {
                        let _ = sender.send(WorkloadEvent::ProviderStopped {
                            manifest_id: deployment_status.manifest_id.clone(),
                        });
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to stop providing manifest_id {}: {}",
                        deployment_status.manifest_id, e
                    );
                }
            }
        }

        // Remove from deployments
        {
            let mut deployments = self.deployments.write().unwrap();
            deployments.remove(workload_id);
        }

        // Send removal event
        if let Some(ref sender) = self.event_sender {
            let _ = sender.send(WorkloadEvent::WorkloadRemoved {
                workload_id: workload_id.to_string(),
                manifest_id: deployment_status.manifest_id,
            });
        }

        Ok(())
    }

    /// Get status of a deployed workload
    pub async fn get_workload_status(
        &self,
        workload_id: &str,
    ) -> WorkloadManagerResult<DeploymentStatus> {
        let deployment_status = {
            let deployments = self.deployments.read().unwrap();
            deployments
                .get(workload_id)
                .cloned()
                .ok_or_else(|| WorkloadManagerError::WorkloadNotFound(workload_id.to_string()))?
        };

        // Get fresh status from runtime engine
        let fresh_result = {
            let registry = self.runtime_registry.read().unwrap();
            let engine = registry
                .get_engine(&deployment_status.runtime_engine)
                .ok_or(WorkloadManagerError::NoRuntimeEngineAvailable)?;
            engine.get_workload_status(workload_id).await
        };

        match fresh_result {
            Ok(workload_info) => {
                let mut updated_status = deployment_status;
                updated_status.status = workload_info.status;
                updated_status.last_updated = workload_info.updated_at;

                // Update stored status
                {
                    let mut deployments = self.deployments.write().unwrap();
                    deployments.insert(workload_id.to_string(), updated_status.clone());
                }

                Ok(updated_status)
            }
            Err(e) => {
                warn!(
                    "Failed to get fresh status for workload {}: {}",
                    workload_id, e
                );
                Ok(deployment_status) // Return cached status
            }
        }
    }

    /// List all deployed workloads
    pub async fn list_workloads(&self) -> Vec<DeploymentStatus> {
        let deployments = self.deployments.read().unwrap();
        deployments.values().cloned().collect()
    }

    /// List workloads deployed by a specific local peer ID
    pub async fn list_workloads_by_peer(&self, local_peer_id: &str) -> Vec<DeploymentStatus> {
        let deployments = self.deployments.read().unwrap();
        let mut filtered_deployments = Vec::new();

        for deployment in deployments.values() {
            // Get the runtime registry to check the actual workload info
            if let Some(registry_guard) =
                crate::workload_integration::get_global_runtime_registry().await
            {
                if let Some(ref registry) = *registry_guard {
                    if let Some(engine) = registry.get_engine(&deployment.runtime_engine) {
                        // Check if this workload has the specified peer ID
                        if let Ok(workload_info) =
                            engine.get_workload_status(&deployment.workload_id).await
                        {
                            if workload_info
                                .metadata
                                .get("local_peer_id")
                                .map(|peer_id| peer_id == local_peer_id)
                                .unwrap_or(false)
                            {
                                filtered_deployments.push(deployment.clone());
                            }
                        }
                    }
                }
            }
        }

        filtered_deployments
    }

    /// Get logs from a workload
    pub async fn get_workload_logs(
        &self,
        workload_id: &str,
        tail: Option<usize>,
    ) -> WorkloadManagerResult<String> {
        let deployment_status = {
            let deployments = self.deployments.read().unwrap();
            deployments
                .get(workload_id)
                .cloned()
                .ok_or_else(|| WorkloadManagerError::WorkloadNotFound(workload_id.to_string()))?
        };

        let logs = {
            let registry = self.runtime_registry.read().unwrap();
            let engine = registry
                .get_engine(&deployment_status.runtime_engine)
                .ok_or(WorkloadManagerError::NoRuntimeEngineAvailable)?;
            engine.get_workload_logs(workload_id, tail).await?
        };

        Ok(logs)
    }

    /// Discover providers for a manifest
    pub async fn discover_providers(
        &self,
        swarm: &mut Swarm<MyBehaviour>,
        manifest_id: &str,
    ) -> WorkloadManagerResult<Vec<crate::provider::ProviderInfo>> {
        info!("Discovering providers for manifest_id: {}", manifest_id);

        let providers = self
            .provider_manager
            .discover_providers(swarm, manifest_id)
            .await?;

        info!(
            "Found {} providers for manifest_id: {}",
            providers.len(),
            manifest_id
        );

        Ok(providers)
    }

    /// Get statistics about the workload manager
    pub fn get_stats(&self) -> WorkloadManagerStats {
        let deployments = self.deployments.read().unwrap();
        let total_workloads = deployments.len();

        let mut status_counts = HashMap::new();
        let mut engine_counts = HashMap::new();
        let mut announced_count = 0;

        for deployment in deployments.values() {
            // Count by status
            let status_key = format!("{:?}", deployment.status);
            *status_counts.entry(status_key).or_insert(0) += 1;

            // Count by runtime engine
            *engine_counts
                .entry(deployment.runtime_engine.clone())
                .or_insert(0) += 1;

            // Count announced providers
            if deployment.announced_as_provider {
                announced_count += 1;
            }
        }

        let provider_stats = self.provider_manager.get_stats();

        WorkloadManagerStats {
            total_workloads,
            status_counts,
            engine_counts,
            announced_providers: announced_count,
            provider_stats,
        }
    }

    /// Start background tasks for workload management
    pub fn start_background_tasks(&self, swarm: &mut Swarm<MyBehaviour>) {
        info!("Starting workload manager background tasks");

        // Start provider manager background tasks
        self.provider_manager.start_background_tasks(swarm);

        // Start workload health monitoring task
        let deployments = Arc::clone(&self.deployments);
        let runtime_registry = Arc::clone(&self.runtime_registry);

        {
            let deployments_clone = Arc::clone(&deployments);
            let runtime_registry_clone = Arc::clone(&runtime_registry);

            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
                loop {
                    interval.tick().await;

                    debug!("Running workload health check");

                    // Get list of workloads to check
                    let workload_ids: Vec<String> = {
                        let deps = deployments_clone.read().unwrap();
                        deps.keys().cloned().collect()
                    };

                    // Check each workload's health
                    for workload_id in workload_ids {
                        let deployment_status = {
                            let deps = deployments_clone.read().unwrap();
                            deps.get(&workload_id).cloned()
                        };

                        if let Some(status) = deployment_status {
                            let engine_name = status.runtime_engine.clone();

                            // Get engine reference separately to avoid holding lock across await
                            let engine_available = {
                                let registry = runtime_registry_clone.read().unwrap();
                                registry.get_engine(&engine_name).is_some()
                            };

                            if engine_available {
                                // We would need to restructure this to avoid lifetime issues
                                // For now, just log that we would check status
                                debug!("Would check status for workload: {}", workload_id);
                            }
                        }
                    }
                }
            });
        }
    }

    /// Internal method to get the preferred runtime engine
    async fn get_runtime_engine(&self) -> WorkloadManagerResult<String> {
        let registry = self.runtime_registry.read().unwrap();

        // Try preferred engine first
        if let Some(ref preferred) = self.config.preferred_runtime {
            if let Some(engine) = registry.get_engine(preferred) {
                if engine.is_available().await {
                    return Ok(preferred.clone());
                } else {
                    warn!("Preferred runtime engine '{}' is not available", preferred);
                }
            }
        }

        // Fall back to default engine
        if let Some(engine) = registry.get_default_engine() {
            if engine.is_available().await {
                return Ok(engine.name().to_string());
            }
        }

        error!("No runtime engines are available");
        Err(WorkloadManagerError::NoRuntimeEngineAvailable)
    }

    /// Internal method to get a runtime engine by name
    #[allow(dead_code)]
    fn get_runtime_engine_by_name(&self, name: &str) -> WorkloadManagerResult<String> {
        let registry = self.runtime_registry.read().unwrap();
        if registry.get_engine(name).is_some() {
            Ok(name.to_string())
        } else {
            Err(WorkloadManagerError::NoRuntimeEngineAvailable)
        }
    }

    /// Internal method to validate manifest content
    async fn validate_manifest_content(
        &self,
        manifest_content: &[u8],
    ) -> WorkloadManagerResult<()> {
        let engine_name = self.get_runtime_engine().await?;
        let registry = self.runtime_registry.read().unwrap();
        let engine = registry
            .get_engine(&engine_name)
            .ok_or(WorkloadManagerError::NoRuntimeEngineAvailable)?;
        engine
            .validate_manifest(manifest_content)
            .await
            .map_err(WorkloadManagerError::RuntimeError)
    }

    /// Internal method to announce as provider
    async fn announce_provider(
        &self,
        swarm: &mut Swarm<MyBehaviour>,
        manifest_id: &str,
        engine_name: &str,
    ) -> WorkloadManagerResult<()> {
        let mut metadata = HashMap::new();
        metadata.insert("runtime_engine".to_string(), engine_name.to_string());
        metadata.insert("node_type".to_string(), "beemesh-machine".to_string());

        self.provider_manager
            .announce_provider(swarm, manifest_id, metadata)
            .map_err(WorkloadManagerError::ProviderError)
    }
}

/// Statistics about the workload manager
#[derive(Debug, Clone)]
pub struct WorkloadManagerStats {
    /// Total number of deployed workloads
    pub total_workloads: usize,
    /// Count of workloads by status
    pub status_counts: HashMap<String, usize>,
    /// Count of workloads by runtime engine
    pub engine_counts: HashMap<String, usize>,
    /// Number of workloads announced as providers
    pub announced_providers: usize,
    /// Provider manager statistics
    pub provider_stats: crate::provider::ProviderStats,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_workload_manager_creation() {
        let manager = WorkloadManager::new_default().await.unwrap();
        let stats = manager.get_stats();

        assert_eq!(stats.total_workloads, 0);
        assert_eq!(stats.announced_providers, 0);
    }

    #[tokio::test]
    async fn test_workload_manager_config() {
        let config = WorkloadManagerConfig {
            announce_as_provider: false,
            provider_ttl_seconds: 7200,
            preferred_runtime: Some("mock".to_string()),
            validate_manifests: false,
            max_concurrent_deployments: 5,
        };

        let manager = WorkloadManager::new(config).await.unwrap();
        assert!(!manager.config.announce_as_provider);
        assert_eq!(manager.config.provider_ttl_seconds, 7200);
        assert_eq!(manager.config.preferred_runtime, Some("mock".to_string()));
    }

    #[tokio::test]
    async fn test_list_empty_workloads() {
        let manager = WorkloadManager::new_default().await.unwrap();
        let workloads = manager.list_workloads().await;
        assert!(workloads.is_empty());
    }

    #[test]
    fn test_workload_manager_stats() {
        let stats = WorkloadManagerStats {
            total_workloads: 5,
            status_counts: HashMap::new(),
            engine_counts: HashMap::new(),
            announced_providers: 3,
            provider_stats: crate::provider::ProviderStats {
                local_providers: 2,
                remote_manifests: 1,
                total_remote_providers: 4,
                pending_queries: 0,
            },
        };

        assert_eq!(stats.total_workloads, 5);
        assert_eq!(stats.announced_providers, 3);
        assert_eq!(stats.provider_stats.local_providers, 2);
    }
}
