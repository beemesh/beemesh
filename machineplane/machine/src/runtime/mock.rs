//! Mock runtime engine for testing
//!
//! This module provides a mock runtime engine that simulates container deployment
//! without actually running any containers. It's useful for testing and development
//! where you don't want to depend on external container runtimes.

use crate::runtime::{
    DeploymentConfig, PortMapping, RuntimeEngine, RuntimeError, RuntimeResult, WorkloadInfo,
    WorkloadStatus,
};
use async_trait::async_trait;
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// A deployed workload in the mock engine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MockWorkload {
    pub info: WorkloadInfo,
    pub config: DeploymentConfig,
}

/// Mock runtime engine that simulates container operations
pub struct MockEngine {
    /// In-memory storage of deployed workloads
    workloads: Arc<Mutex<HashMap<String, MockWorkload>>>,
    /// Configuration for mock behavior
    config: MockConfig,
}

/// Configuration options for the mock engine
#[derive(Debug, Clone)]
pub struct MockConfig {
    /// Whether deployments should succeed or fail
    pub deployment_success_rate: f64,
    /// Simulated deployment delay in milliseconds
    pub deployment_delay_ms: u64,
    /// Whether to simulate port mappings
    pub simulate_ports: bool,
    /// Default port mappings to simulate
    pub default_ports: Vec<PortMapping>,
}

impl Default for MockConfig {
    fn default() -> Self {
        Self {
            deployment_success_rate: 1.0, // 100% success by default
            deployment_delay_ms: 100,     // 100ms delay
            simulate_ports: true,
            default_ports: vec![PortMapping {
                container_port: 80,
                host_port: Some(8080),
                protocol: "tcp".to_string(),
            }],
        }
    }
}

impl MockEngine {
    /// Create a new mock engine with default configuration
    pub fn new() -> Self {
        Self {
            workloads: Arc::new(Mutex::new(HashMap::new())),
            config: MockConfig::default(),
        }
    }

    /// Create a new mock engine with custom configuration
    pub fn with_config(config: MockConfig) -> Self {
        Self {
            workloads: Arc::new(Mutex::new(HashMap::new())),
            config,
        }
    }

    /// Get the number of deployed workloads (for testing)
    pub fn workload_count(&self) -> usize {
        self.workloads.lock().unwrap().len()
    }

    /// Get a copy of all workloads (for testing)
    pub fn get_all_workloads(&self) -> Vec<MockWorkload> {
        self.workloads.lock().unwrap().values().cloned().collect()
    }

    /// Get workloads deployed by a specific local peer ID (for testing)
    pub fn get_workloads_by_peer(&self, local_peer_id: &str) -> Vec<MockWorkload> {
        self.workloads
            .lock()
            .unwrap()
            .values()
            .filter(|workload| {
                workload
                    .info
                    .metadata
                    .get("local_peer_id")
                    .map(|peer_id| peer_id == local_peer_id)
                    .unwrap_or(false)
            })
            .cloned()
            .collect()
    }

    /// Clear all workloads (for testing)
    pub fn clear_workloads(&self) {
        self.workloads.lock().unwrap().clear();
    }



    /// Get the deployment config for a workload (for testing verification)
    pub fn get_workload_config(&self, workload_id: &str) -> Option<DeploymentConfig> {
        self.workloads
            .lock()
            .unwrap()
            .get(workload_id)
            .map(|w| w.config.clone())
    }

    /// Simulate deployment delay
    async fn simulate_delay(&self) {
        if self.config.deployment_delay_ms > 0 {
            tokio::time::sleep(tokio::time::Duration::from_millis(
                self.config.deployment_delay_ms,
            ))
            .await;
        }
    }

    /// Check if deployment should succeed based on success rate
    fn should_deployment_succeed(&self) -> bool {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        rng.gen::<f64>() < self.config.deployment_success_rate
    }

    /// Generate a unique workload ID based on manifest name and ID
    fn generate_workload_id(&self, manifest_id: &str, manifest_content: &[u8]) -> String {
        let metadata = self.parse_manifest_metadata(manifest_content);
        let manifest_name = metadata
            .get("name")
            .map(|n| n.as_str())
            .unwrap_or("unnamed");
        format!("mock-{}-{}", manifest_name, manifest_id)
    }

    /// Generate a unique workload ID with peer ID for uniqueness
    fn generate_workload_id_with_peer(
        &self,
        manifest_id: &str,
        manifest_content: &[u8],
        peer_id: libp2p::PeerId,
    ) -> String {
        let metadata = self.parse_manifest_metadata(manifest_content);
        let manifest_name = metadata
            .get("name")
            .map(|n| n.as_str())
            .unwrap_or("unnamed");
        // Use last 12 chars of peer ID for better uniqueness (avoids common 12D3KooW prefix)
        let peer_str = peer_id.to_string();
        let peer_short = if peer_str.len() > 12 {
            peer_str[peer_str.len() - 12..].to_string()
        } else {
            peer_str
        };
        format!("mock-{}-{}-{}", manifest_name, manifest_id, peer_short)
    }

    /// Parse manifest content to extract metadata (simplified YAML parsing)
    fn parse_manifest_metadata(&self, manifest_content: &[u8]) -> HashMap<String, String> {
        let manifest_str = String::from_utf8_lossy(manifest_content);
        let mut metadata = HashMap::new();

        // Try to parse as JSON first (this is what we're getting from the CLI)
        if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(&manifest_str) {
            // Extract top-level fields
            if let Some(kind) = json_val.get("kind").and_then(|v| v.as_str()) {
                metadata.insert("kind".to_string(), kind.to_string());
            }
            
            if let Some(api_version) = json_val.get("apiVersion").and_then(|v| v.as_str()) {
                metadata.insert("apiVersion".to_string(), api_version.to_string());
            }
            
            // Extract name from metadata object
            if let Some(meta_obj) = json_val.get("metadata").and_then(|v| v.as_object()) {
                if let Some(name) = meta_obj.get("name").and_then(|v| v.as_str()) {
                    metadata.insert("name".to_string(), name.to_string());
                }
                
                if let Some(namespace) = meta_obj.get("namespace").and_then(|v| v.as_str()) {
                    metadata.insert("namespace".to_string(), namespace.to_string());
                }
            }
        } else {
            // Fallback to YAML parsing for older content
            for line in manifest_str.lines() {
                let line = line.trim();
                if line.starts_with("kind:") {
                    if let Some(value) = line.strip_prefix("kind:").map(|s| s.trim()) {
                        metadata.insert("kind".to_string(), value.to_string());
                    }
                } else if line.starts_with("apiVersion:") {
                    if let Some(value) = line.strip_prefix("apiVersion:").map(|s| s.trim()) {
                        metadata.insert("apiVersion".to_string(), value.to_string());
                    }
                } else if line.contains("name:") {
                    if let Some(name_part) = line.split("name:").nth(1) {
                        let value = name_part.trim();
                        if !value.is_empty() {
                            metadata.insert("name".to_string(), value.to_string());
                        }
                    }
                }
            }
        }

        metadata
    }

    /// Generate simulated port mappings
    fn generate_port_mappings(&self) -> Vec<PortMapping> {
        if self.config.simulate_ports {
            self.config.default_ports.clone()
        } else {
            Vec::new()
        }
    }
}

impl Default for MockEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl RuntimeEngine for MockEngine {
    fn name(&self) -> &str {
        "mock"
    }

    async fn is_available(&self) -> bool {
        // Mock engine is always available
        true
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn validate_manifest(&self, manifest_content: &[u8]) -> RuntimeResult<()> {
        let manifest_str = String::from_utf8_lossy(manifest_content);

        // Basic validation - check if it's not empty and looks like YAML/JSON
        if manifest_str.trim().is_empty() {
            return Err(RuntimeError::InvalidManifest("Empty manifest".to_string()));
        }

        // Try to parse as YAML to ensure it's valid
        if serde_yaml::from_str::<serde_yaml::Value>(&manifest_str).is_err() {
            return Err(RuntimeError::InvalidManifest(
                "Invalid YAML format".to_string(),
            ));
        }

        debug!("Mock engine: manifest validation passed");
        Ok(())
    }

    async fn deploy_workload(
        &self,
        manifest_id: &str,
        manifest_content: &[u8],
        config: &DeploymentConfig,
    ) -> RuntimeResult<WorkloadInfo> {
        info!(
            "Mock engine: deploying workload for manifest_id: {}",
            manifest_id
        );

        // Validate manifest first
        self.validate_manifest(manifest_content).await?;

        // Simulate deployment delay
        self.simulate_delay().await;

        // Check if deployment should succeed
        if !self.should_deployment_succeed() {
            return Err(RuntimeError::DeploymentFailed(
                "Mock deployment failure".to_string(),
            ));
        }

        // Generate unique workload ID
        let workload_id = self.generate_workload_id(manifest_id, manifest_content);

        // Parse manifest metadata
        let metadata = self.parse_manifest_metadata(manifest_content);

        // Generate port mappings
        let ports = self.generate_port_mappings();

        // Create workload info
        let now = SystemTime::now();
        let workload_info = WorkloadInfo {
            id: workload_id.clone(),
            manifest_id: manifest_id.to_string(),
            status: WorkloadStatus::Running,
            metadata,
            created_at: now,
            updated_at: now,
            ports,
        };

        // Store the workload
        let mock_workload = MockWorkload {
            info: workload_info.clone(),
            config: config.clone(),
        };

        {
            let mut workloads = self.workloads.lock().unwrap();
            workloads.insert(workload_id.clone(), mock_workload);
        }

        info!(
            "Mock engine: successfully deployed workload {} for manifest {}",
            workload_id, manifest_id
        );

        Ok(workload_info)
    }

    /// Deploy a workload with local peer ID tracking
    async fn deploy_workload_with_peer(
        &self,
        manifest_id: &str,
        manifest_content: &[u8],
        config: &DeploymentConfig,
        local_peer_id: libp2p::PeerId,
    ) -> RuntimeResult<WorkloadInfo> {
        // Simulate deployment failure based on configuration
        if !self.should_deployment_succeed() {
            return Err(RuntimeError::DeploymentFailed(
                "Mock deployment failure".to_string(),
            ));
        }

        // Generate unique workload ID with peer ID for uniqueness
        let workload_id = self.generate_workload_id_with_peer(manifest_id, manifest_content, local_peer_id);

        // Parse manifest metadata
        let mut metadata = self.parse_manifest_metadata(manifest_content);

        // Add local peer ID to metadata
        metadata.insert("local_peer_id".to_string(), local_peer_id.to_string());

        // Generate port mappings
        let ports = self.generate_port_mappings();

        // Create workload info
        let now = SystemTime::now();
        let workload_info = WorkloadInfo {
            id: workload_id.clone(),
            manifest_id: manifest_id.to_string(),
            status: WorkloadStatus::Running,
            metadata,
            created_at: now,
            updated_at: now,
            ports,
        };

        // Store the workload
        let mock_workload = MockWorkload {
            info: workload_info.clone(),
            config: config.clone(),
        };

        self.workloads
            .lock()
            .unwrap()
            .insert(workload_id, mock_workload);

        Ok(workload_info)
    }

    async fn get_workload_status(&self, workload_id: &str) -> RuntimeResult<WorkloadInfo> {
        debug!("Mock engine: getting status for workload: {}", workload_id);

        let workloads = self.workloads.lock().unwrap();
        match workloads.get(workload_id) {
            Some(workload) => {
                let mut info = workload.info.clone();
                // Update the timestamp to simulate status refresh
                info.updated_at = SystemTime::now();
                Ok(info)
            }
            None => Err(RuntimeError::WorkloadNotFound(workload_id.to_string())),
        }
    }

    async fn list_workloads(&self) -> RuntimeResult<Vec<WorkloadInfo>> {
        debug!("Mock engine: listing all workloads");

        let workloads = self.workloads.lock().unwrap();
        let mut workload_infos: Vec<WorkloadInfo> = workloads
            .values()
            .map(|w| {
                let mut info = w.info.clone();
                // Update timestamp to simulate fresh status
                info.updated_at = SystemTime::now();
                info
            })
            .collect();

        // Sort by creation time for consistent ordering
        workload_infos.sort_by(|a, b| a.created_at.cmp(&b.created_at));

        debug!("Mock engine: found {} workloads", workload_infos.len());
        Ok(workload_infos)
    }

    async fn remove_workload(&self, workload_id: &str) -> RuntimeResult<()> {
        info!("Mock engine: removing workload: {}", workload_id);

        let mut workloads = self.workloads.lock().unwrap();
        match workloads.remove(workload_id) {
            Some(_) => {
                info!("Mock engine: successfully removed workload {}", workload_id);
                Ok(())
            }
            None => {
                warn!(
                    "Mock engine: workload {} not found for removal",
                    workload_id
                );
                Err(RuntimeError::WorkloadNotFound(workload_id.to_string()))
            }
        }
    }

    async fn get_workload_logs(
        &self,
        workload_id: &str,
        tail: Option<usize>,
    ) -> RuntimeResult<String> {
        debug!("Mock engine: getting logs for workload: {}", workload_id);

        let workloads = self.workloads.lock().unwrap();
        match workloads.get(workload_id) {
            Some(workload) => {
                // Generate mock logs
                let lines = vec![
                    "2024-01-01T00:00:00Z [INFO] Mock workload started",
                    "2024-01-01T00:00:01Z [INFO] Initializing services",
                    "2024-01-01T00:00:02Z [INFO] Services ready",
                    "2024-01-01T00:00:03Z [INFO] Workload running normally",
                    "2024-01-01T00:00:04Z [DEBUG] Processing requests",
                    "2024-01-01T00:00:05Z [INFO] Health check passed",
                ];

                let log_content = if let Some(tail_lines) = tail {
                    lines
                        .iter()
                        .rev()
                        .take(tail_lines)
                        .rev()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    lines.join("\n")
                };

                debug!(
                    "Mock engine: returning {} bytes of logs for workload {}",
                    log_content.len(),
                    workload_id
                );

                Ok(format!(
                    "{}\n2024-01-01T00:00:06Z [INFO] Mock logs for workload {} (manifest_id: {})",
                    log_content, workload_id, workload.info.manifest_id
                ))
            }
            None => Err(RuntimeError::WorkloadNotFound(workload_id.to_string())),
        }
    }

    async fn export_manifest(&self, workload_id: &str) -> RuntimeResult<Vec<u8>> {
        debug!("Mock engine: exporting manifest for workload: {}", workload_id);

        let workloads = self.workloads.lock().unwrap();
        match workloads.get(workload_id) {
            Some(workload) => {
                // Generate a mock Kubernetes manifest based on stored metadata
                let metadata = &workload.info.metadata;
                let name = metadata.get("name").unwrap_or(&workload.info.id);
                let kind = metadata.get("kind").map(|s| s.as_str()).unwrap_or("Deployment");
                let api_version = metadata.get("apiVersion").map(|s| s.as_str()).unwrap_or("apps/v1");
                let namespace = metadata.get("namespace").map(|s| s.as_str()).unwrap_or("default");

                // Create a reconstructed manifest based on the workload info
                let mock_manifest = format!(
                    r#"apiVersion: {}
kind: {}
metadata:
  name: {}
  namespace: {}
  labels:
    beemesh.io/workload-id: {}
    beemesh.io/manifest-id: {}
    beemesh.io/generated: "true"
spec:
  replicas: {}
  selector:
    matchLabels:
      app: {}
  template:
    metadata:
      labels:
        app: {}
        beemesh.io/workload-id: {}
    spec:
      containers:
      - name: mock-container
        image: mock:latest
        ports:"#,
                    api_version,
                    kind,
                    name,
                    namespace,
                    workload.info.id,
                    workload.info.manifest_id,
                    workload.config.replicas,
                    name,
                    name,
                    workload.info.id
                );

                // Add port mappings if available
                let mut manifest_with_ports = mock_manifest;
                if !workload.info.ports.is_empty() {
                    for port in &workload.info.ports {
                        manifest_with_ports.push_str(&format!(
                            "\n        - containerPort: {}\n          protocol: {}",
                            port.container_port, port.protocol
                        ));
                    }
                } else {
                    manifest_with_ports.push_str("\n        - containerPort: 80\n          protocol: TCP");
                }

                // Add environment variables if available
                if !workload.config.env.is_empty() {
                    manifest_with_ports.push_str("\n        env:");
                    for (key, value) in &workload.config.env {
                        manifest_with_ports.push_str(&format!(
                            "\n        - name: {}\n          value: \"{}\"",
                            key, value
                        ));
                    }
                }

                // Add resource limits if available
                let resources = &workload.config.resources;
                if resources.cpu.is_some() || resources.memory.is_some() {
                    manifest_with_ports.push_str("\n        resources:");
                    if resources.cpu.is_some() || resources.memory.is_some() {
                        manifest_with_ports.push_str("\n          limits:");
                        if let Some(cpu) = resources.cpu {
                            manifest_with_ports.push_str(&format!("\n            cpu: \"{}\"", cpu));
                        }
                        if let Some(memory) = resources.memory {
                            manifest_with_ports.push_str(&format!("\n            memory: \"{}\"", memory));
                        }
                    }
                }

                info!(
                    "Mock engine: generated manifest for workload {} ({} bytes)",
                    workload_id,
                    manifest_with_ports.len()
                );

                Ok(manifest_with_ports.into_bytes())
            }
            None => Err(RuntimeError::WorkloadNotFound(workload_id.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_engine_creation() {
        let engine = MockEngine::new();
        assert_eq!(engine.name(), "mock");
        assert_eq!(engine.workload_count(), 0);
    }

    #[tokio::test]
    async fn test_mock_engine_availability() {
        let engine = MockEngine::new();
        assert!(engine.is_available().await);
    }

    #[tokio::test]
    async fn test_manifest_validation() {
        let engine = MockEngine::new();

        // Valid YAML manifest
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

        assert!(engine.validate_manifest(valid_manifest).await.is_ok());

        // Empty manifest
        let empty_manifest = b"";
        assert!(engine.validate_manifest(empty_manifest).await.is_err());

        // Invalid YAML
        let invalid_manifest = b"invalid: yaml: content: [[[";
        assert!(engine.validate_manifest(invalid_manifest).await.is_err());
    }

    #[tokio::test]
    async fn test_workload_deployment() {
        let engine = MockEngine::new();

        let manifest = br#"
apiVersion: v1
kind: Pod
metadata:
  name: test-pod
spec:
  containers:
  - name: nginx
    image: nginx:latest
"#;

        let config = DeploymentConfig::default();
        let result = engine
            .deploy_workload("test-manifest", manifest, &config)
            .await;

        assert!(result.is_ok());
        let workload_info = result.unwrap();
        assert_eq!(workload_info.manifest_id, "test-manifest");
        assert_eq!(workload_info.status, WorkloadStatus::Running);
        assert_eq!(engine.workload_count(), 1);

        // Verify workload is running
        let status = engine.get_workload_status(&workload_info.id).await;
        assert!(status.is_ok());
        assert_eq!(status.unwrap().status, WorkloadStatus::Running);
    }

    #[tokio::test]
    async fn test_workload_status() {
        let engine = MockEngine::new();
        let manifest = b"apiVersion: v1\nkind: Pod\nmetadata:\n  name: test";
        let config = DeploymentConfig::default();

        let workload_info = engine
            .deploy_workload("test-manifest", manifest, &config)
            .await
            .unwrap();

        // Get status
        let status = engine.get_workload_status(&workload_info.id).await;
        assert!(status.is_ok());
        let status_info = status.unwrap();
        assert_eq!(status_info.id, workload_info.id);
        assert_eq!(status_info.status, WorkloadStatus::Running);

        // Try to get status for non-existent workload
        let missing_status = engine.get_workload_status("non-existent").await;
        assert!(missing_status.is_err());
    }

    #[tokio::test]
    async fn test_list_workloads() {
        let engine = MockEngine::new();
        let manifest = b"apiVersion: v1\nkind: Pod\nmetadata:\n  name: test";
        let config = DeploymentConfig::default();

        // Initially empty
        let workloads = engine.list_workloads().await.unwrap();
        assert_eq!(workloads.len(), 0);

        // Deploy a workload
        engine
            .deploy_workload("test-manifest-1", manifest, &config)
            .await
            .unwrap();

        // Deploy another workload
        engine
            .deploy_workload("test-manifest-2", manifest, &config)
            .await
            .unwrap();

        // List should have 2 workloads
        let workloads = engine.list_workloads().await.unwrap();
        assert_eq!(workloads.len(), 2);
    }

    #[tokio::test]
    async fn test_remove_workload() {
        let engine = MockEngine::new();
        let manifest = b"apiVersion: v1\nkind: Pod\nmetadata:\n  name: test";
        let config = DeploymentConfig::default();

        let workload_info = engine
            .deploy_workload("test-manifest", manifest, &config)
            .await
            .unwrap();

        assert_eq!(engine.workload_count(), 1);

        // Remove the workload
        let result = engine.remove_workload(&workload_info.id).await;
        assert!(result.is_ok());
        assert_eq!(engine.workload_count(), 0);

        // Try to remove again should fail
        let result = engine.remove_workload(&workload_info.id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_workload_logs() {
        let engine = MockEngine::new();
        let manifest = b"apiVersion: v1\nkind: Pod\nmetadata:\n  name: test";
        let config = DeploymentConfig::default();

        let workload_info = engine
            .deploy_workload("test-manifest", manifest, &config)
            .await
            .unwrap();

        // Get logs
        let logs = engine.get_workload_logs(&workload_info.id, None).await;
        assert!(logs.is_ok());
        let log_content = logs.unwrap();
        assert!(log_content.contains("Mock workload started"));
        assert!(log_content.contains(&workload_info.id));

        // Get logs with tail
        let logs = engine.get_workload_logs(&workload_info.id, Some(2)).await;
        assert!(logs.is_ok());

        // Try to get logs for non-existent workload
        let logs = engine.get_workload_logs("non-existent", None).await;
        assert!(logs.is_err());
    }

    #[tokio::test]
    async fn test_mock_config() {
        let config = MockConfig {
            deployment_success_rate: 0.0, // Always fail
            deployment_delay_ms: 10,
            simulate_ports: false,
            default_ports: Vec::new(),
        };

        let engine = MockEngine::with_config(config);
        let manifest = b"apiVersion: v1\nkind: Pod\nmetadata:\n  name: test";
        let deploy_config = DeploymentConfig::default();

        // Should fail due to 0% success rate
        let result = engine
            .deploy_workload("test-manifest", manifest, &deploy_config)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_parse_manifest_metadata() {
        let engine = MockEngine::new();

        let manifest = b"apiVersion: v1\nkind: Pod\nmetadata:\n  name: test-pod";
        let metadata = engine.parse_manifest_metadata(manifest);

        assert_eq!(metadata.get("apiVersion"), Some(&"v1".to_string()));
        assert_eq!(metadata.get("kind"), Some(&"Pod".to_string()));
        assert_eq!(metadata.get("name"), Some(&"test-pod".to_string()));
    }

    #[tokio::test]
    async fn test_clear_workloads() {
        let engine = MockEngine::new();
        let manifest = b"apiVersion: v1\nkind: Pod\nmetadata:\n  name: test";
        let config = DeploymentConfig::default();

        // Deploy some workloads
        engine
            .deploy_workload("test-1", manifest, &config)
            .await
            .unwrap();
        engine
            .deploy_workload("test-2", manifest, &config)
            .await
            .unwrap();

        assert_eq!(engine.workload_count(), 2);

        // Clear all workloads
        engine.clear_workloads();
        assert_eq!(engine.workload_count(), 0);
    }
}
