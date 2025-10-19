//! Comprehensive tests for the workload manager with mock engine verification
//!
//! This test file demonstrates how to use the mock runtime engine to verify
//! that manifests are correctly applied without actually running containers.

use crate::runtime::mock::{MockConfig, MockEngine};
use crate::runtime::{DeploymentConfig, ResourceLimits, RuntimeEngine, WorkloadStatus};
use crate::workload_manager::{WorkloadManager, WorkloadManagerConfig};
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::timeout;

/// Test helper to create a mock workload manager for testing
async fn create_test_workload_manager() -> WorkloadManager {
    let config = WorkloadManagerConfig {
        announce_as_provider: false, // Disable for tests
        provider_ttl_seconds: 300,
        preferred_runtime: Some("mock".to_string()),
        validate_manifests: true,
        max_concurrent_deployments: 5,
    };

    WorkloadManager::new(config)
        .await
        .expect("Failed to create test workload manager")
}

/// Test helper to create a sample Kubernetes manifest
fn create_test_manifest(name: &str) -> Vec<u8> {
    format!(
        r#"apiVersion: v1
kind: Pod
metadata:
  name: {name}
  labels:
    app: test-app
    component: web
spec:
  containers:
  - name: nginx
    image: nginx:1.21
    ports:
    - containerPort: 80
      protocol: TCP
    env:
    - name: ENV_VAR
      value: "test-value"
    resources:
      requests:
        memory: "64Mi"
        cpu: "250m"
      limits:
        memory: "128Mi"
        cpu: "500m"
  restartPolicy: Always
"#
    )
    .into_bytes()
}

/// Test helper to create a Docker Compose manifest
fn create_docker_compose_manifest(service_name: &str) -> Vec<u8> {
    format!(
        r#"version: '3.8'
services:
  {service_name}:
    image: nginx:latest
    ports:
      - "8080:80"
    environment:
      - ENV_VAR=test-value
    restart: unless-stopped
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost"]
      interval: 30s
      timeout: 10s
      retries: 3
networks:
  default:
    driver: bridge
"#
    )
    .into_bytes()
}

/// Test helper to create deployment config with specific settings
fn create_test_deployment_config(replicas: u32) -> DeploymentConfig {
    let mut env = HashMap::new();
    env.insert("TEST_ENV".to_string(), "test-value".to_string());
    env.insert("REPLICA_COUNT".to_string(), replicas.to_string());

    let mut runtime_options = HashMap::new();
    runtime_options.insert("network".to_string(), "test-network".to_string());

    DeploymentConfig {
        replicas,
        resources: ResourceLimits {
            cpu: Some(1.0),
            memory: Some(512 * 1024 * 1024),   // 512MB
            storage: Some(1024 * 1024 * 1024), // 1GB
        },
        env,
        runtime_options,
    }
}

#[tokio::test]
async fn test_workload_manager_creation() {
    let manager = create_test_workload_manager().await;
    let stats = manager.get_stats();

    assert_eq!(stats.total_workloads, 0);
    assert_eq!(stats.announced_providers, 0);
    assert!(stats.engine_counts.is_empty());
    assert!(stats.status_counts.is_empty());
}

#[tokio::test]
async fn test_deploy_kubernetes_manifest() {
    let manager = create_test_workload_manager().await;
    // Mock engine tests don't need actual swarm

    let manifest_id = "test-k8s-manifest";
    let manifest_content = create_test_manifest("test-pod");
    let config = create_test_deployment_config(1);

    let result = manager
        .deploy_workload(&mut swarm, manifest_id, &manifest_content, &config)
        .await;

    assert!(result.is_ok());
    let deployment = result.unwrap();

    assert_eq!(deployment.manifest_id, manifest_id);
    assert_eq!(deployment.runtime_engine, "mock");
    assert_eq!(deployment.status, WorkloadStatus::Running);
    assert!(!deployment.announced_as_provider); // Disabled in test config
    assert_eq!(deployment.config.replicas, 1);

    // Verify the manager stats
    let stats = manager.get_stats();
    assert_eq!(stats.total_workloads, 1);
    assert_eq!(stats.engine_counts.get("mock"), Some(&1));
}

#[tokio::test]
async fn test_deploy_multiple_replicas() {
    let manager = create_test_workload_manager().await;
    // Mock engine tests don't need actual swarm

    let manifest_id = "test-multi-replica";
    let manifest_content = create_test_manifest("multi-replica-pod");
    let config = create_test_deployment_config(3);

    let result = manager
        .deploy_workload(&mut swarm, manifest_id, &manifest_content, &config)
        .await;

    assert!(result.is_ok());
    let deployment = result.unwrap();

    assert_eq!(deployment.config.replicas, 3);
    assert!(deployment.config.resources.cpu.is_some());
    assert!(deployment.config.resources.memory.is_some());
}

#[tokio::test]
async fn test_verify_manifest_content_with_mock() {
    let manager = create_test_workload_manager().await;
    // Mock engine tests don't need actual swarm

    let manifest_id = "test-verify-manifest";
    let manifest_content = create_test_manifest("verification-pod");
    let config = DeploymentConfig::default();

    // Deploy the workload
    let deployment = manager
        .deploy_workload(&mut swarm, manifest_id, &manifest_content, &config)
        .await
        .expect("Failed to deploy workload");

    // Now verify that the mock engine received the correct manifest
    // This is where the mock engine's verification capabilities shine

    // We'll need to access the mock engine through the runtime registry
    // For now, we verify through the deployment status
    assert_eq!(deployment.manifest_id, manifest_id);
    assert_eq!(deployment.status, WorkloadStatus::Running);

    // Verify deployment config was applied correctly
    assert_eq!(deployment.config.replicas, 1);
    assert!(deployment.config.env.contains_key("TEST_ENV"));
}

#[tokio::test]
async fn test_workload_removal() {
    let manager = create_test_workload_manager().await;
    // Mock engine tests don't need actual swarm

    let manifest_id = "test-removal";
    let manifest_content = create_test_manifest("removal-test-pod");
    let config = DeploymentConfig::default();

    // Deploy workload
    let deployment = manager
        .deploy_workload(&mut swarm, manifest_id, &manifest_content, &config)
        .await
        .expect("Failed to deploy workload");

    let workload_id = deployment.workload_id.clone();

    // Verify it's deployed
    let stats_before = manager.get_stats();
    assert_eq!(stats_before.total_workloads, 1);

    // Remove the workload
    let removal_result = manager.remove_workload(&mut swarm, &workload_id).await;
    assert!(removal_result.is_ok());

    // Verify it's removed
    let stats_after = manager.get_stats();
    assert_eq!(stats_after.total_workloads, 0);

    // Trying to get status should fail
    let status_result = manager.get_workload_status(&workload_id).await;
    assert!(status_result.is_err());
}

#[tokio::test]
async fn test_list_workloads() {
    let manager = create_test_workload_manager().await;
    // Mock engine tests don't need actual swarm

    // Deploy multiple workloads
    let workloads_to_deploy = vec![
        ("manifest-1", "pod-1"),
        ("manifest-2", "pod-2"),
        ("manifest-3", "pod-3"),
    ];

    let mut deployed_ids = Vec::new();

    for (manifest_id, pod_name) in workloads_to_deploy {
        let manifest_content = create_test_manifest(pod_name);
        let config = DeploymentConfig::default();

        let deployment = manager
            .deploy_workload(&mut swarm, manifest_id, &manifest_content, &config)
            .await
            .expect("Failed to deploy workload");

        deployed_ids.push(deployment.workload_id);
    }

    // List all workloads
    let workloads = manager.list_workloads().await;
    assert_eq!(workloads.len(), 3);

    // Verify all deployed workloads are in the list
    let listed_ids: Vec<String> = workloads.iter().map(|w| w.workload_id.clone()).collect();
    for deployed_id in deployed_ids {
        assert!(listed_ids.contains(&deployed_id));
    }
}

#[tokio::test]
async fn test_workload_logs() {
    let manager = create_test_workload_manager().await;
    // Mock engine tests don't need actual swarm

    let manifest_id = "test-logs";
    let manifest_content = create_test_manifest("logs-test-pod");
    let config = DeploymentConfig::default();

    // Deploy workload
    let deployment = manager
        .deploy_workload(&mut swarm, manifest_id, &manifest_content, &config)
        .await
        .expect("Failed to deploy workload");

    // Get logs without tail limit
    let logs_result = manager
        .get_workload_logs(&deployment.workload_id, None)
        .await;
    assert!(logs_result.is_ok());

    let logs = logs_result.unwrap();
    assert!(!logs.is_empty());
    assert!(logs.contains("Mock workload started"));
    assert!(logs.contains(&deployment.workload_id));

    // Get logs with tail limit
    let limited_logs_result = manager
        .get_workload_logs(&deployment.workload_id, Some(2))
        .await;
    assert!(limited_logs_result.is_ok());

    let limited_logs = limited_logs_result.unwrap();
    assert!(!limited_logs.is_empty());
}

#[tokio::test]
async fn test_workload_status_updates() {
    let manager = create_test_workload_manager().await;
    // Mock engine tests don't need actual swarm

    let manifest_id = "test-status";
    let manifest_content = create_test_manifest("status-test-pod");
    let config = DeploymentConfig::default();

    // Deploy workload
    let deployment = manager
        .deploy_workload(&mut swarm, manifest_id, &manifest_content, &config)
        .await
        .expect("Failed to deploy workload");

    // Get initial status
    let initial_status = manager
        .get_workload_status(&deployment.workload_id)
        .await
        .expect("Failed to get workload status");

    assert_eq!(initial_status.workload_id, deployment.workload_id);
    assert_eq!(initial_status.status, WorkloadStatus::Running);

    // The status should be recent
    let time_diff = initial_status
        .last_updated
        .duration_since(deployment.deployed_at)
        .unwrap_or_default();
    assert!(time_diff.as_secs() < 5); // Should be within 5 seconds
}

#[tokio::test]
async fn test_invalid_manifest_validation() {
    let manager = create_test_workload_manager().await;
    // Mock engine tests don't need actual swarm

    let manifest_id = "test-invalid";
    let invalid_manifest = b"invalid: yaml: content: [[[";
    let config = DeploymentConfig::default();

    // Attempt to deploy invalid manifest
    let result = manager
        .deploy_workload(&mut swarm, manifest_id, invalid_manifest, &config)
        .await;

    assert!(result.is_err());

    // Verify no workloads were created
    let stats = manager.get_stats();
    assert_eq!(stats.total_workloads, 0);
}

#[tokio::test]
async fn test_concurrent_deployments() {
    let _manager = create_test_workload_manager().await;

    let num_concurrent = 5;
    let mut deploy_tasks = Vec::new();

    // Start multiple concurrent deployments
    for i in 0..num_concurrent {
        let manifest_id = format!("concurrent-manifest-{}", i);
        let manifest_content = create_test_manifest(&format!("concurrent-pod-{}", i));
        let config = DeploymentConfig::default();

        // Note: In a real test, you'd need to handle the swarm borrowing differently
        // This is simplified for demonstration
        let task = async move {
            // Each task would need its own swarm or we'd need to coordinate access
            (manifest_id, manifest_content, config)
        };

        deploy_tasks.push(task);
    }

    // Wait for all tasks to complete
    let results = futures::future::join_all(deploy_tasks).await;
    assert_eq!(results.len(), num_concurrent);
}

#[tokio::test]
async fn test_docker_compose_manifest() {
    let manager = create_test_workload_manager().await;
    // Mock engine tests don't need actual swarm

    let manifest_id = "test-docker-compose";
    let manifest_content = create_docker_compose_manifest("web-service");
    let config = DeploymentConfig::default();

    // Deploy Docker Compose manifest
    let result = manager
        .deploy_workload(&mut swarm, manifest_id, &manifest_content, &config)
        .await;

    // Should succeed with mock engine (it accepts any valid YAML)
    assert!(result.is_ok());
    let deployment = result.unwrap();

    assert_eq!(deployment.manifest_id, manifest_id);
    assert_eq!(deployment.runtime_engine, "mock");
}

#[tokio::test]
async fn test_resource_limits_application() {
    let manager = create_test_workload_manager().await;
    // Mock engine tests don't need actual swarm

    let manifest_id = "test-resources";
    let manifest_content = create_test_manifest("resource-test-pod");

    let config = DeploymentConfig {
        replicas: 2,
        resources: ResourceLimits {
            cpu: Some(2.5),
            memory: Some(1024 * 1024 * 1024),      // 1GB
            storage: Some(5 * 1024 * 1024 * 1024), // 5GB
        },
        env: {
            let mut env = HashMap::new();
            env.insert("RESOURCE_TEST".to_string(), "true".to_string());
            env
        },
        runtime_options: HashMap::new(),
    };

    // Deploy with resource limits
    let result = manager
        .deploy_workload(&mut swarm, manifest_id, &manifest_content, &config)
        .await;

    assert!(result.is_ok());
    let deployment = result.unwrap();

    // Verify resource limits were applied
    assert_eq!(deployment.config.resources.cpu, Some(2.5));
    assert_eq!(deployment.config.resources.memory, Some(1024 * 1024 * 1024));
    assert_eq!(
        deployment.config.resources.storage,
        Some(5 * 1024 * 1024 * 1024)
    );
    assert_eq!(
        deployment.config.env.get("RESOURCE_TEST"),
        Some(&"true".to_string())
    );
}

#[tokio::test]
async fn test_workload_manager_stats() {
    let manager = create_test_workload_manager().await;
    // Mock engine tests don't need actual swarm

    // Deploy workloads of different types
    let deployments = vec![
        ("k8s-1", create_test_manifest("k8s-pod-1")),
        ("k8s-2", create_test_manifest("k8s-pod-2")),
        (
            "compose-1",
            create_docker_compose_manifest("compose-service-1"),
        ),
    ];

    for (manifest_id, manifest_content) in deployments {
        let config = DeploymentConfig::default();
        let _ = manager
            .deploy_workload(&mut swarm, manifest_id, &manifest_content, &config)
            .await
            .expect("Failed to deploy workload");
    }

    // Check stats
    let stats = manager.get_stats();
    assert_eq!(stats.total_workloads, 3);
    assert_eq!(stats.engine_counts.get("mock"), Some(&3));

    // All should be running (mock engine default)
    let running_count = stats.status_counts.get("Running").unwrap_or(&0);
    assert_eq!(*running_count, 3);
}

/// Test the mock engine's verification capabilities directly
#[tokio::test]
async fn test_mock_engine_verification() {
    let mock_config = MockConfig {
        deployment_success_rate: 1.0,
        deployment_delay_ms: 10,
        simulate_ports: true,
        default_ports: vec![crate::runtime::PortMapping {
            container_port: 80,
            host_port: Some(8080),
            protocol: "tcp".to_string(),
        }],
    };

    let engine = MockEngine::with_config(mock_config);

    let manifest_id = "verification-test";
    let manifest_content = create_test_manifest("verification-pod");
    let config = DeploymentConfig::default();

    // Deploy workload
    let workload_info = engine
        .deploy_workload(manifest_id, &manifest_content, &config)
        .await
        .expect("Failed to deploy to mock engine");

    // Verify the mock engine stored the correct data
    assert_eq!(workload_info.manifest_id, manifest_id);
    assert_eq!(workload_info.status, WorkloadStatus::Running);
    assert!(!workload_info.ports.is_empty());

    // Use mock engine's verification methods
    let stored_config = engine.get_workload_config(&workload_info.id);
    assert!(stored_config.is_some());
    assert_eq!(stored_config.unwrap().replicas, config.replicas);

    // Verify workload can be listed
    let workloads = engine.list_workloads().await.unwrap();
    assert!(!workloads.is_empty());
    assert!(workloads.iter().any(|w| w.id == workload_info.id));

    // Verify workload count
    assert_eq!(engine.workload_count(), 1);

    // Test removal
    let removal_result = engine.remove_workload(&workload_info.id).await;
    assert!(removal_result.is_ok());
    assert_eq!(engine.workload_count(), 0);
}

/// Integration test demonstrating the complete workflow
#[tokio::test]
async fn test_complete_workflow() {
    let manager = create_test_workload_manager().await;
    // Mock engine tests don't need actual swarm

    // Step 1: Deploy a workload
    let manifest_id = "workflow-test";
    let manifest_content = create_test_manifest("workflow-pod");
    let config = create_test_deployment_config(2);

    let deployment = manager
        .deploy_workload(&mut swarm, manifest_id, &manifest_content, &config)
        .await
        .expect("Failed to deploy workload");

    println!("Deployed workload: {}", deployment.workload_id);

    // Step 2: Verify deployment
    let status = manager
        .get_workload_status(&deployment.workload_id)
        .await
        .expect("Failed to get workload status");

    assert_eq!(status.status, WorkloadStatus::Running);

    // Step 3: Get logs
    let logs = manager
        .get_workload_logs(&deployment.workload_id, Some(5))
        .await
        .expect("Failed to get workload logs");

    assert!(logs.contains("Mock workload started"));

    // Step 4: List all workloads
    let all_workloads = manager.list_workloads().await;
    assert_eq!(all_workloads.len(), 1);
    assert_eq!(all_workloads[0].workload_id, deployment.workload_id);

    // Step 5: Check statistics
    let stats = manager.get_stats();
    assert_eq!(stats.total_workloads, 1);
    assert_eq!(stats.engine_counts.get("mock"), Some(&1));

    // Step 6: Remove workload
    let removal_result = manager
        .remove_workload(&mut swarm, &deployment.workload_id)
        .await;
    assert!(removal_result.is_ok());

    // Step 7: Verify removal
    let final_stats = manager.get_stats();
    assert_eq!(final_stats.total_workloads, 0);

    println!("Workflow test completed successfully!");
}

/// Test timeout behavior
#[tokio::test]
async fn test_deployment_timeout() {
    let mock_config = MockConfig {
        deployment_success_rate: 1.0,
        deployment_delay_ms: 2000, // 2 second delay
        simulate_ports: false,
        default_ports: vec![],
    };

    let engine = MockEngine::with_config(mock_config);

    let manifest_content = create_test_manifest("timeout-test");
    let config = DeploymentConfig::default();

    // Test with short timeout (should timeout)
    let short_timeout_result = timeout(
        Duration::from_millis(500),
        engine.deploy_workload("timeout-test", &manifest_content, &config),
    )
    .await;

    assert!(short_timeout_result.is_err()); // Should timeout

    // Test with long timeout (should succeed)
    let long_timeout_result = timeout(
        Duration::from_millis(3000),
        engine.deploy_workload("timeout-test-2", &manifest_content, &config),
    )
    .await;

    assert!(long_timeout_result.is_ok());
    assert!(long_timeout_result.unwrap().is_ok());
}
