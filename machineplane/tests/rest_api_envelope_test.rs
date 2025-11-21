use machineplane::messages::machine::{
    build_delete_request, build_delete_response, root_as_delete_request, root_as_delete_response,
};

#[test]
fn delete_request_roundtrip() {
    let bytes = build_delete_request("manifest-1", "op-9", "origin-x", true);
    let parsed = root_as_delete_request(&bytes).expect("bincode decode");

    assert_eq!(parsed.manifest_id, "manifest-1");
    assert_eq!(parsed.operation_id, "op-9");
    assert_eq!(parsed.origin_peer, "origin-x");
    assert!(parsed.force);
}

#[test]
fn delete_response_roundtrip() {
    let removed = vec!["workload-a".to_string(), "workload-b".to_string()];
    let bytes = build_delete_response(true, "op-9", "ok", "manifest-1", &removed);
    let parsed = root_as_delete_response(&bytes).expect("bincode decode");

    assert!(parsed.ok);
    assert_eq!(parsed.operation_id, "op-9");
    assert_eq!(parsed.message, "ok");
    assert_eq!(parsed.manifest_id, "manifest-1");
    assert_eq!(parsed.removed_workloads, removed);
}
