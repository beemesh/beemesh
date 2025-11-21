use machine::messages::machine::{
    build_capacity_request_with_id, extract_manifest_name, root_as_apply_request,
    root_as_capacity_request,
};

#[test]
fn invalid_payloads_fail_to_decode() {
    let malformed = b"this is not bincode";
    assert!(root_as_apply_request(malformed).is_err());
    assert!(root_as_capacity_request(malformed).is_err());
}

#[test]
fn capacity_request_builder_roundtrip() {
    let bytes = build_capacity_request_with_id("req-123", 250, 512 * 1024 * 1024, 10_000, 2);
    let parsed = root_as_capacity_request(&bytes).expect("decode capacity request");

    assert_eq!(parsed.request_id, "req-123");
    assert_eq!(parsed.cpu_milli, 250);
    assert_eq!(parsed.memory_bytes, 512 * 1024 * 1024);
    assert_eq!(parsed.storage_bytes, 10_000);
    assert_eq!(parsed.replicas, 2);
}

#[test]
fn extract_manifest_name_handles_missing_fields() {
    assert!(extract_manifest_name(br"{}").is_none());
    assert!(extract_manifest_name(br"{\"metadata\":{}} ").is_none());
}
