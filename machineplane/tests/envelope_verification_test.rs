use base64::Engine;
use machine::crypto::{ensure_keypair_ephemeral, ensure_pqc_init, sign_envelope};
use machine::libp2p_beemesh::envelope::verify_flatbuffer_envelope;
use machine::protocol::machine::{build_envelope_canonical, build_envelope_signed};
use std::time::Duration;

#[test]
fn test_flatbuffer_envelope_roundtrip() {
    // Test that FlatBuffer envelopes signed with canonical bytes verify correctly
    // - Create FlatBuffer envelope with payload
    // - Sign canonical bytes
    // - Verify using verify_flatbuffer_envelope()

    ensure_pqc_init().expect("PQC initialization failed");
    let (pubb, privb) = ensure_keypair_ephemeral().expect("Failed to generate keypair");

    let payload = b"test payload data";
    let payload_type = "test";
    let nonce = format!(
        "test-nonce-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let timestamp = 1234567890u64;
    let alg = "ml-dsa-65";

    // Create canonical bytes for signing
    let canonical_bytes =
        build_envelope_canonical(payload, payload_type, &nonce, timestamp, alg, None);

    // Sign the canonical bytes
    let (sig_b64, pub_b64) =
        sign_envelope(&privb, &pubb, &canonical_bytes).expect("Failed to sign envelope");

    // Build signed FlatBuffer envelope
    let signed_envelope = build_envelope_signed(
        payload,
        payload_type,
        &nonce,
        timestamp,
        alg,
        "ml-dsa-65",
        &sig_b64,
        &pub_b64,
        None,
    );

    // Verify using verify_flatbuffer_envelope()
    let result = verify_flatbuffer_envelope(&signed_envelope, Duration::from_secs(300));

    assert!(
        result.is_ok(),
        "FlatBuffer envelope verification failed: {:?}",
        result.err()
    );

    let parts = result.unwrap();
    assert_eq!(parts.payload, payload, "Payload should match original");
}

#[test]
fn test_flatbuffer_envelope_invalid_signature() {
    // Test that invalid signatures are rejected
    ensure_pqc_init().expect("PQC initialization failed");
    let (pubb, _privb) = ensure_keypair_ephemeral().expect("Failed to generate keypair");

    let payload = b"test payload data";
    let payload_type = "test";
    let nonce = "invalid-sig-test";
    let timestamp = 1234567890u64;
    let alg = "ml-dsa-65";

    // Create envelope with invalid signature
    let invalid_envelope = build_envelope_signed(
        payload,
        payload_type,
        nonce,
        timestamp,
        alg,
        "ml-dsa-65",
        "aW52YWxpZC1zaWduYXR1cmU=", // "invalid-signature" in base64
        &base64::engine::general_purpose::STANDARD.encode(pubb),
        None,
    );

    // Verification should fail
    let result = verify_flatbuffer_envelope(&invalid_envelope, Duration::from_secs(300));
    assert!(
        result.is_err(),
        "Invalid signature should fail verification"
    );
}

#[test]
fn test_flatbuffer_envelope_replay_protection() {
    // Test nonce-based replay protection
    ensure_pqc_init().expect("PQC initialization failed");
    let (pubb, privb) = ensure_keypair_ephemeral().expect("Failed to generate keypair");

    let payload = b"replay test payload";
    let payload_type = "test";
    let nonce = format!(
        "replay-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let timestamp = 1234567890u64;
    let alg = "ml-dsa-65";

    // Create canonical bytes and sign
    let canonical_bytes =
        build_envelope_canonical(payload, payload_type, &nonce, timestamp, alg, None);
    let (sig_b64, pub_b64) =
        sign_envelope(&privb, &pubb, &canonical_bytes).expect("Failed to sign envelope");

    // Build signed envelope
    let envelope = build_envelope_signed(
        payload,
        payload_type,
        &nonce,
        timestamp,
        alg,
        "ml-dsa-65",
        &sig_b64,
        &pub_b64,
        None,
    );

    // First verification should succeed
    let first_result = verify_flatbuffer_envelope(&envelope, Duration::from_secs(300));
    assert!(
        first_result.is_ok(),
        "First verification should succeed: {:?}",
        first_result.err()
    );

    // Second verification with same nonce should fail (replay protection)
    let second_result = verify_flatbuffer_envelope(&envelope, Duration::from_secs(300));
    assert!(
        second_result.is_err(),
        "Replay should be detected and rejected"
    );
}

#[test]
fn test_flatbuffer_envelope_signature_prefix_handling() {
    // Test that signature prefixes are handled correctly
    ensure_pqc_init().expect("PQC initialization failed");
    let (pubb, privb) = ensure_keypair_ephemeral().expect("Failed to generate keypair");

    let payload = b"prefix test payload";
    let payload_type = "test";
    let nonce = "prefix-test-nonce";
    let timestamp = 1234567890u64;
    let alg = "ml-dsa-65";

    // Create canonical bytes and sign
    let canonical_bytes =
        build_envelope_canonical(payload, payload_type, nonce, timestamp, alg, None);
    let (sig_b64, pub_b64) =
        sign_envelope(&privb, &pubb, &canonical_bytes).expect("Failed to sign envelope");

    // Build envelope with explicit algorithm prefix
    let envelope_with_prefix = build_envelope_signed(
        payload,
        payload_type,
        nonce,
        timestamp,
        alg,
        "ml-dsa-65", // explicit prefix
        &sig_b64,
        &pub_b64,
        None,
    );

    // Verification should succeed
    let result = verify_flatbuffer_envelope(&envelope_with_prefix, Duration::from_secs(300));
    assert!(
        result.is_ok(),
        "Envelope with explicit prefix should verify: {:?}",
        result.err()
    );

    // Verify the signature field contains the prefix
    let env = machine::protocol::machine::root_as_envelope(&envelope_with_prefix)
        .expect("should parse envelope");
    let sig_field = env.sig().unwrap_or("");
    assert!(
        sig_field.contains("ml-dsa-65:"),
        "Signature field should contain prefix: {}",
        sig_field
    );
}

#[test]
fn test_flatbuffer_envelope_empty_payload() {
    // Test handling of empty payloads
    ensure_pqc_init().expect("PQC initialization failed");
    let (pubb, privb) = ensure_keypair_ephemeral().expect("Failed to generate keypair");

    let payload = b"";
    let payload_type = "empty";
    let nonce = "empty-payload-test";
    let timestamp = 1234567890u64;
    let alg = "ml-dsa-65";

    // Create canonical bytes and sign
    let canonical_bytes =
        build_envelope_canonical(payload, payload_type, nonce, timestamp, alg, None);
    let (sig_b64, pub_b64) =
        sign_envelope(&privb, &pubb, &canonical_bytes).expect("Failed to sign envelope");

    // Build envelope
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

    // Verification should succeed even with empty payload
    let result = verify_flatbuffer_envelope(&envelope, Duration::from_secs(300));
    assert!(
        result.is_ok(),
        "Empty payload should verify correctly: {:?}",
        result.err()
    );

    let parts = result.unwrap();
    assert_eq!(parts.payload, payload, "Empty payload should be preserved");
}

#[test]
fn test_flatbuffer_envelope_large_payload() {
    // Test handling of larger payloads
    ensure_pqc_init().expect("PQC initialization failed");
    let (pubb, privb) = ensure_keypair_ephemeral().expect("Failed to generate keypair");

    // Create a larger payload (1MB)
    let large_payload = vec![0x42u8; 1024 * 1024];
    let payload_type = "large";
    let nonce = "large-payload-test";
    let timestamp = 1234567890u64;
    let alg = "ml-dsa-65";

    // Create canonical bytes and sign
    let canonical_bytes =
        build_envelope_canonical(&large_payload, payload_type, nonce, timestamp, alg, None);
    let (sig_b64, pub_b64) =
        sign_envelope(&privb, &pubb, &canonical_bytes).expect("Failed to sign envelope");

    // Build envelope
    let envelope = build_envelope_signed(
        &large_payload,
        payload_type,
        nonce,
        timestamp,
        alg,
        "ml-dsa-65",
        &sig_b64,
        &pub_b64,
        None,
    );

    // Verification should succeed
    let result = verify_flatbuffer_envelope(&envelope, Duration::from_secs(300));
    assert!(
        result.is_ok(),
        "Large payload should verify correctly: {:?}",
        result.err()
    );

    let parts = result.unwrap();
    assert_eq!(
        parts.payload, large_payload,
        "Large payload should be preserved"
    );
}

#[test]
fn test_flatbuffer_envelope_malformed_data() {
    // Test handling of malformed FlatBuffer data
    let malformed_data = b"this is not a valid flatbuffer";

    let result = verify_flatbuffer_envelope(malformed_data, Duration::from_secs(300));
    assert!(result.is_err(), "Malformed data should fail to parse");
}
