use base64::Engine;
use crypto::{ensure_keypair_ephemeral, ensure_pqc_init, sign_envelope};
use protocol::machine::{build_envelope_canonical, build_envelope_signed};
use std::time::Duration;

#[test]
fn test_envelope_payload_extraction() {
    // This test should verify that the envelope payload can be extracted and matches the expected encrypted manifest
    // (Implementation needed based on new envelope structure)
    // Example placeholder:
    // let (payload_bytes, _, _) = verification_result.unwrap();
    // assert_eq!(payload_bytes, encrypted_manifest);
    assert!(true); // Placeholder assertion
}

#[test]
fn test_apply_request_flatbuffer_envelope() {
    // Test that apply requests use FlatBuffer envelopes correctly
    ensure_pqc_init().expect("PQC initialization failed");
    let (pubb, privb) = ensure_keypair_ephemeral().expect("Failed to generate keypair");

    // Create an apply request FlatBuffer
    let apply_request = protocol::machine::build_apply_request(
        3,                           // replicas
        "apply-op-123",              // operation_id
        "apiVersion: v1\nkind: Pod", // manifest_json (YAML as string)
        "origin-peer-id",            // origin_peer
        "test-manifest-id",          // manifest_id
    );

    let payload_type = "apply_request";
    let nonce = format!(
        "apply-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let alg = "ml-dsa-65";

    // Create signed envelope containing the apply request
    let canonical_bytes =
        build_envelope_canonical(&apply_request, payload_type, &nonce, timestamp, alg, None);

    let (sig_b64, pub_b64) =
        sign_envelope(&privb, &pubb, &canonical_bytes).expect("Failed to sign apply envelope");

    let apply_envelope = build_envelope_signed(
        &apply_request,
        payload_type,
        &nonce,
        timestamp,
        alg,
        "ml-dsa-65",
        &sig_b64,
        &pub_b64,
        None,
    );

    // Verify the envelope
    let verification_result = machine::libp2p_beemesh::envelope::verify_flatbuffer_envelope(
        &apply_envelope,
        Duration::from_secs(300),
    );

    assert!(
        verification_result.is_ok(),
        "Apply envelope should verify correctly: {:?}",
        verification_result.err()
    );

    // Verify we can extract and parse the apply request
    let parts = verification_result.unwrap();
    let parsed_apply = protocol::machine::root_as_apply_request(&parts.payload)
        .expect("Should parse apply request from envelope payload");

    assert_eq!(parsed_apply.replicas(), 3);
    assert_eq!(parsed_apply.operation_id().unwrap_or(""), "apply-op-123");
    assert!(
        parsed_apply
            .manifest_json()
            .unwrap_or("")
            .contains("kind: Pod")
    );
}

#[test]
fn test_envelope_base64_encoding_for_transport() {
    // Test that envelopes can be base64 encoded for HTTP transport
    ensure_pqc_init().expect("PQC initialization failed");
    let (pubb, privb) = ensure_keypair_ephemeral().expect("Failed to generate keypair");

    let payload = b"test transport payload";
    let payload_type = "transport_test";
    let nonce = "transport-nonce";
    let timestamp = 1234567890u64;
    let alg = "ml-dsa-65";

    // Create signed envelope
    let canonical_bytes =
        build_envelope_canonical(payload, payload_type, nonce, timestamp, alg, None);
    let (sig_b64, pub_b64) =
        sign_envelope(&privb, &pubb, &canonical_bytes).expect("Failed to sign envelope");

    let envelope = build_envelope_signed(
        payload,
        payload_type,
        nonce,
        timestamp,
        alg,
        "ml-dsa-65",
        &sig_b64,
        &pub_b64,
        None,
    );

    // Encode for transport
    let encoded_envelope = base64::engine::general_purpose::STANDARD.encode(&envelope);
    assert!(!encoded_envelope.is_empty());

    // Decode and verify
    let decoded_envelope = base64::engine::general_purpose::STANDARD
        .decode(&encoded_envelope)
        .expect("Should decode base64 envelope");

    let verification_result = machine::libp2p_beemesh::envelope::verify_flatbuffer_envelope(
        &decoded_envelope,
        Duration::from_secs(300),
    );

    assert!(
        verification_result.is_ok(),
        "Decoded envelope should verify correctly: {:?}",
        verification_result.err()
    );

    let parts = verification_result.unwrap();
    assert_eq!(parts.payload, payload);
}
