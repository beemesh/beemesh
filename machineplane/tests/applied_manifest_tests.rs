use machineplane::messages::machine::{build_applied_manifest, root_as_applied_manifest};
use machineplane::messages::types::{KeyValue, OperationType, SignatureScheme};

#[test]
fn test_applied_manifest_owner_fields_roundtrip() {
    let id = "id-123";
    let operation_id = "op-1";
    let origin_peer = "12D3KooW...";
    let owner_pub = vec![1u8, 2u8, 3u8];
    let signature = vec![9u8, 8u8, 7u8];
    let manifest_json = "{\"k\":\"v\"}";
    let manifest_kind = "Test";
    let labels: Vec<KeyValue> = vec![];
    let timestamp = 123456789u64;

    let buf = build_applied_manifest(
        id,
        operation_id,
        origin_peer,
        &owner_pub,
        &signature,
        SignatureScheme::None,
        manifest_json,
        manifest_kind,
        &labels,
        timestamp,
        OperationType::Apply,
        3600,
        "chash",
    );

    let parsed = root_as_applied_manifest(&buf).expect("parse");
    assert_eq!(parsed.id, id);
    assert_eq!(parsed.owner_pubkey, owner_pub);
    assert_eq!(parsed.signature, signature);
    assert_eq!(parsed.labels, labels);
}
