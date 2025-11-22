use serial_test::serial;

use std::path::PathBuf;
use std::time::Duration;
use tokio::time::sleep;

#[path = "apply_common.rs"]
mod apply_common;
#[path = "kube_helpers.rs"]
mod kube_helpers;

use apply_common::{
    check_workload_deployment, get_peer_ids, setup_test_environment, start_fabric_nodes,
    wait_for_mesh_formation,
};
use kube_helpers::apply_manifest_via_kube_api;

#[serial]
#[tokio::test]
async fn test_disabled_nodes_do_not_schedule_workloads() {
    let (client, ports) = setup_test_environment().await;
    let mut guard = start_fabric_nodes(&[true, false, true]).await;

    sleep(Duration::from_secs(3)).await;
    let mesh_formed = wait_for_mesh_formation(&client, &ports, Duration::from_secs(15)).await;
    if !mesh_formed {
        log::warn!("Mesh formation incomplete, but proceeding with test");
    }

    let manifest_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/sample_manifests/nginx.yml");

    let original_content = tokio::fs::read_to_string(manifest_path.clone())
        .await
        .expect("Failed to read manifest for verification");

    let tender_id = apply_manifest_via_kube_api(&client, ports[0], &manifest_path)
        .await
        .expect("kubectl apply should succeed");

    sleep(Duration::from_secs(6)).await;

    let port_to_peer_id = get_peer_ids(&client, &ports).await;
    let (nodes_with_deployed_workloads, nodes_with_content_mismatch) = check_workload_deployment(
        &client,
        &ports,
        &tender_id,
        &original_content,
        &port_to_peer_id,
        false,
    )
    .await;

    assert_eq!(
        nodes_with_deployed_workloads.len(),
        1,
        "Expected a single node to host the workload"
    );
    assert!(
        !nodes_with_deployed_workloads.contains(&3000)
            && !nodes_with_deployed_workloads.contains(&3200),
        "Node with disabled scheduling should not receive workloads"
    );
    assert!(
        nodes_with_content_mismatch.is_empty(),
        "Deployed manifest content mismatch on nodes: {:?}",
        nodes_with_content_mismatch
    );

    guard.cleanup().await;
}

#[serial]
#[tokio::test]
async fn test_scheduling_fails_when_all_nodes_disabled() {
    let (client, ports) = setup_test_environment().await;
    let mut guard = start_fabric_nodes(&[true, true, true]).await;

    sleep(Duration::from_secs(3)).await;
    let mesh_formed = wait_for_mesh_formation(&client, &ports, Duration::from_secs(10)).await;
    if !mesh_formed {
        log::warn!("Mesh formation incomplete, but proceeding with test");
    }

    let manifest_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/sample_manifests/nginx.yml");

    let apply_result = apply_manifest_via_kube_api(&client, ports[0], &manifest_path).await;

    assert!(
        apply_result.is_err(),
        "kubectl apply should fail when no schedulable nodes are available"
    );

    let err_msg = apply_result.unwrap_err().to_string();
    assert!(
        err_msg.contains("HTTP 503") || err_msg.contains("Service Unavailable"),
        "Expected HTTP 503 when scheduling fails, got: {}",
        err_msg
    );

    guard.cleanup().await;
}
