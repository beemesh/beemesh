use std::time::Duration;
use tokio::time::sleep;
use log::info;
use std::env;
use std::fs;
use std::path::PathBuf;

mod test_utils;
use test_utils::{make_test_cli, start_nodes};

pub const TENANT: &str = "00000000-0000-0000-0000-000000000000";

// We will start machines directly in this process by calling `start_machine(cli).await`.

#[tokio::test]
async fn test_run_host_application() {
    // Initialize logger once (tests may run multiple times in same process)
    let _ = env_logger::try_init();

    // start three nodes using the reusable helper (first node runs REST+machine, others disabled APIs)
    let cli1 = make_test_cli(
        3000,
        false,
        false,
        None,
        None,
    );
    let cli2 = make_test_cli(
        3100,
        true,
        true,
        None,
        None,
    );
    let cli3 = make_test_cli(
        3200,
        true,
        true,
        None,
        None,
    );

    let mut guard = start_nodes(vec![cli1, cli2, cli3], Duration::from_secs(1)).await;

    // wait for the mesh to form (poll until peers appear or timeout)
    let verify_peers = wait_for_peers(Duration::from_secs(15)).await;
    let health = check_health().await;

    guard.cleanup().await;

    assert_eq!(health, "ok");
    assert!(verify_peers["peers"].as_array().expect("peers should be an array").to_vec().len() == 2, "Expected at least two peers in the mesh, got {:?}", verify_peers);

}

async fn check_health() -> String {
        tokio::time::timeout(
            Duration::from_secs(5),
            reqwest::get("http://localhost:3000/health")
        ).await.unwrap().unwrap().text().await.expect("failed to call health api")
}

async fn verify_peers() -> serde_json::Value {
    let url = format!("http://localhost:3000/tenant/{}/nodes", TENANT);
        let resp = tokio::time::timeout(
        Duration::from_secs(5),
        reqwest::get(url)
    ).await.unwrap().unwrap();
    let json = resp.json().await;
    info!("{:?}", json);
    let nodes: serde_json::Value = json.unwrap_or_default();
    nodes
}

async fn wait_for_peers(timeout: Duration) -> serde_json::Value {
    let start = tokio::time::Instant::now();
    loop {
        let nodes = verify_peers().await;
        if !nodes["peers"].as_array().map(|a| a.is_empty()).unwrap_or(true) {
            return nodes;
        }
        if start.elapsed() > timeout {
            return nodes;
        }
        sleep(Duration::from_secs(1)).await;
    }
}