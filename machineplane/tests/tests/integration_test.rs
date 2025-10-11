use log::info;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use tokio::time::sleep;

mod test_utils;
use test_utils::{make_test_cli, setup_cleanup_hook, start_nodes};

pub const TENANT: &str = "00000000-0000-0000-0000-000000000000";

// We will start machines directly in this process by calling `start_machine(cli).await`.

#[tokio::test]
async fn test_run_host_application() {
    // Setup cleanup hook and initialize logger
    setup_cleanup_hook();
    let _ = env_logger::try_init();

    // start three nodes using the reusable helper (first node runs REST+machine, others disabled APIs)
    // node_3000 gets fixed libp2p port 4001, node_3100 gets port 4002, both serve as bootstrap peers
    let cli1 = make_test_cli(3000, false, false, None, vec![], 4001, 0);
    let cli2 = make_test_cli(
        3100,
        true,
        true,
        None,
        vec!["/ip4/127.0.0.1/tcp/4001".to_string()],
        4002,
        0,
    );
    // node_3200 uses both nodes as bootstrap peers for resilience
    let bootstrap_peers = vec![
        "/ip4/127.0.0.1/tcp/4001".to_string(),
        "/ip4/127.0.0.1/tcp/4002".to_string(),
    ];
    let cli3 = make_test_cli(3200, true, true, None, bootstrap_peers, 0, 0);

    let mut guard = start_nodes(vec![cli1, cli2, cli3], Duration::from_secs(1)).await;

    // wait for the mesh to form (poll until peers appear or timeout)
    let verify_peers = wait_for_peers(Duration::from_secs(15)).await;
    let health = check_health().await;

    // Test the pubkey endpoint
    let kem_pubkey_result = check_pubkey("kem_pubkey").await;
    let signing_pubkey_result = check_pubkey("signing_pubkey").await;

    guard.cleanup().await;

    assert_eq!(health, "ok");
    assert!(
        verify_peers["peers"]
            .as_array()
            .expect("peers should be an array")
            .to_vec()
            .len()
            == 2,
        "Expected at least two peers in the mesh, got {:?}",
        verify_peers
    );
    assert!(
        kem_pubkey_result.is_empty() == false,
        "Expected kem_pubkey field in response, got: {}",
        kem_pubkey_result
    );
    assert!(
        signing_pubkey_result.is_empty() == false,
        "Expected signing_pubkey field in response, got: {}",
        signing_pubkey_result
    );
}

async fn check_health() -> String {
    tokio::time::timeout(
        Duration::from_secs(5),
        reqwest::get("http://localhost:3000/health"),
    )
    .await
    .unwrap()
    .unwrap()
    .text()
    .await
    .expect("failed to call health api")
}

async fn check_pubkey(url: &str) -> String {
    tokio::time::timeout(
        Duration::from_secs(5),
        reqwest::get(format!("http://localhost:3000/api/v1/{}", url)),
    )
    .await
    .unwrap()
    .unwrap()
    .text()
    .await
    .expect("failed to call pubkey endpoint")
}

async fn verify_peers() -> serde_json::Value {
    let url = "http://localhost:3000/debug/peers";
    let resp = tokio::time::timeout(Duration::from_secs(5), reqwest::get(url))
        .await
        .unwrap()
        .unwrap();
    let json = resp.json().await;
    info!("{:?}", json);
    let nodes: serde_json::Value = json.unwrap_or_default();
    nodes
}

async fn wait_for_peers(timeout: Duration) -> serde_json::Value {
    let start = tokio::time::Instant::now();
    loop {
        let nodes = verify_peers().await;
        if !nodes["peers"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(true)
        {
            return nodes;
        }
        if start.elapsed() > timeout {
            return nodes;
        }
        sleep(Duration::from_secs(1)).await;
    }
}
