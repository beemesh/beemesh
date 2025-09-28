use std::time::Duration;
use tokio::process::Command;
use tokio::time::sleep;

struct NodeGuard {
    node1: Option<tokio::process::Child>,
    node2: Option<tokio::process::Child>,
}

impl NodeGuard {
    async fn cleanup(&mut self) {
        if let Some(mut n2) = self.node2.take() {
            let _ = n2.kill().await;
        }
        if let Some(mut n1) = self.node1.take() {
            let _ = n1.kill().await;
        }
    }
}

// Ensure processes are killed even if the test panics: Drop is called on unwind.
impl Drop for NodeGuard {
    fn drop(&mut self) {
        // Best-effort synchronous kill. tokio::process::Child exposes a synchronous kill().
        if let Some(mut n2) = self.node2.take() {
            let _ = n2.kill();
        }
        if let Some(mut n1) = self.node1.take() {
            let _ = n1.kill();
        }
    }
}

pub const TENANT: &str = "00000000-0000-0000-0000-000000000000";

#[tokio::test]
async fn test_run_host_application() {
    let node1 = Command::new("../target/debug/machine")
        .spawn()
        .expect("Failed to start machine");

    //sleep(Duration::from_secs(2)).await;

    let node2 = Command::new("../target/debug/machine")
        .args(&["--disable-rest-api", "--disable-machine-api"])
        .spawn()
        .expect("Failed to start machine");

    let mut guard = NodeGuard {
        node1: Some(node1),
        node2: Some(node2),
    };

    let _sleep = sleep(Duration::from_secs(5)).await;

    let health = check_health().await;
    let verify_peers = verify_peers().await;
    let _sleep = sleep(Duration::from_secs(1)).await;

    guard.cleanup().await;

    assert_eq!(health, "ok");
    println!("{}", verify_peers);
    assert!(!verify_peers["peers"].as_array().expect("peers should be an array").to_vec().is_empty(), "Expected at least one peer in the mesh, got {:?}", verify_peers);
}

async fn check_health() -> String {
        tokio::time::timeout(
            Duration::from_secs(5),
            reqwest::get("http://localhost:3000/health")
        ).await.unwrap().unwrap().text().await.expect("failed to call health api")
}

async fn verify_peers() -> serde_json::Value {
    let url = format!("http://localhost:3000/tenant/{TENANT}/nodes");
        let resp = tokio::time::timeout(
        Duration::from_secs(5),
        reqwest::get(url)
    ).await.unwrap().unwrap();
    let json = resp.json().await;
    println!("{:?}", json);   
    let nodes: serde_json::Value = json.unwrap_or_default();
    nodes
}