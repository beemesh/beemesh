use machineplane::messages::machine::{
    build_apply_request, build_bid, build_task_status_response, compute_manifest_id,
    compute_manifest_id_from_content, extract_manifest_name, root_as_apply_request, root_as_bid,
    root_as_task_status_response,
};

#[test]
fn apply_request_bincode_roundtrip() {
    let bytes = build_apply_request(3, "op-1", "{\"kind\":\"Pod\"}", "peer-a", "manifest-a");
    let parsed = root_as_apply_request(&bytes).expect("bincode decode");

    assert_eq!(parsed.replicas, 3);
    assert_eq!(parsed.operation_id, "op-1");
    assert_eq!(parsed.origin_peer, "peer-a");
    assert!(parsed.manifest_json.contains("Pod"));
    assert_eq!(parsed.manifest_id, "manifest-a");
}

#[test]
fn bid_roundtrip_preserves_scores() {
    let signature = vec![1, 2, 3, 4];
    let bytes = build_bid("task-123", "node-456", 0.9, 0.8, 0.7, 1111, &signature);
    let parsed = root_as_bid(&bytes).expect("bincode decode");

    assert_eq!(parsed.task_id, "task-123");
    assert_eq!(parsed.node_id, "node-456");
    assert_eq!(parsed.score, 0.9);
    assert_eq!(parsed.resource_fit_score, 0.8);
    assert_eq!(parsed.network_locality_score, 0.7);
    assert_eq!(parsed.signature, signature);
}

#[test]
fn task_status_response_roundtrip() {
    let assigned_peers = vec!["peer-a".to_string(), "peer-b".to_string()];
    let bytes = build_task_status_response("task-1", "running", &assigned_peers, Some("cid123"));

    let parsed = root_as_task_status_response(&bytes).expect("bincode decode");

    assert_eq!(parsed.task_id, "task-1");
    assert_eq!(parsed.state, "running");
    assert_eq!(parsed.assigned_peers, assigned_peers);
    assert_eq!(parsed.manifest_cid, "cid123");
}

#[test]
fn manifest_helpers_compute_expected_ids() {
    let manifest_json = br#"{"metadata":{"name":"demo"}}"#;
    let extracted = extract_manifest_name(manifest_json).expect("name available");
    assert_eq!(extracted, "demo");

    let hash_based = compute_manifest_id_from_content(manifest_json);
    assert!(!hash_based.is_empty());

    let versioned = compute_manifest_id("demo", 7);
    assert_eq!(versioned, "demo:7");
}
