use base64::Engine;
use crypto::{ensure_keypair_ephemeral, ensure_pqc_init, sign_envelope};
use protocol::machine::{build_envelope_canonical, build_envelope_signed};
use std::time::Duration;

#[test]
fn test_distribute_shares_flatbuffer_handling() {
    // Test that distribute_shares correctly processes FlatBuffer envelope payloads
    // - Create FlatBuffer signed envelope with shares data
    // - Verify envelope can be parsed and processed correctly

    ensure_pqc_init().expect("PQC initialization failed");
    let (pubb, privb) = ensure_keypair_ephemeral().expect("Failed to generate keypair");

    // Create shares data as raw bytes (would be encrypted key shares in reality)
    let shares_data = b"encrypted_share_data_for_manifest_123";
    let payload_type = "shares";
    let nonce = format!(
        "shares-test-{}",
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

    // Create FlatBuffer envelope for shares
    let canonical_bytes =
        build_envelope_canonical(shares_data, payload_type, &nonce, timestamp, alg, None);

    let (sig_b64, pub_b64) =
        sign_envelope(&privb, &pubb, &canonical_bytes).expect("Failed to sign shares envelope");

    let shares_envelope = build_envelope_signed(
        shares_data,
        payload_type,
        &nonce,
        timestamp,
        alg,
        "ml-dsa-65",
        &sig_b64,
        &pub_b64,
        None,
    );

    // Verify the envelope can be parsed back correctly
    let parsed_envelope = protocol::machine::root_as_envelope(&shares_envelope)
        .expect("Shares envelope should parse correctly");

    assert_eq!(parsed_envelope.payload_type().unwrap_or(""), payload_type);
    assert_eq!(parsed_envelope.nonce().unwrap_or(""), nonce);
    assert_eq!(parsed_envelope.alg().unwrap_or(""), alg);
    assert!(!parsed_envelope.sig().unwrap_or("").is_empty());
    assert!(!parsed_envelope.pubkey().unwrap_or("").is_empty());

    // Verify payload matches original
    let payload_bytes: Vec<u8> = parsed_envelope
        .payload()
        .unwrap_or_default()
        .iter()
        .collect();
    assert_eq!(payload_bytes, shares_data);
}

#[test]
fn test_distribute_capabilities_flatbuffer_handling() {
    // Test that distribute_capabilities correctly processes FlatBuffer capability envelopes
    ensure_pqc_init().expect("PQC initialization failed");
    let (pubb, privb) = ensure_keypair_ephemeral().expect("Failed to generate keypair");

    // Create capability token data
    let capability_data = b"capability_token_for_manifest_456";
    let payload_type = "capability";
    let nonce = format!(
        "capability-test-{}",
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

    // Create FlatBuffer envelope for capability
    let canonical_bytes =
        build_envelope_canonical(capability_data, payload_type, &nonce, timestamp, alg, None);

    let (sig_b64, pub_b64) =
        sign_envelope(&privb, &pubb, &canonical_bytes).expect("Failed to sign capability envelope");

    let capability_envelope = build_envelope_signed(
        capability_data,
        payload_type,
        &nonce,
        timestamp,
        alg,
        "ml-dsa-65",
        &sig_b64,
        &pub_b64,
        None,
    );

    // Verify the envelope is valid for capability distribution
    let verification_result = machine::libp2p_beemesh::envelope::verify_flatbuffer_envelope(
        &capability_envelope,
        Duration::from_secs(300),
    );

    assert!(
        verification_result.is_ok(),
        "Capability envelope should verify correctly: {:?}",
        verification_result.err()
    );

    let (payload_bytes, _pub_bytes, _sig_bytes) = verification_result.unwrap();
    assert_eq!(payload_bytes, capability_data);
}

#[test]
fn test_apply_request_flatbuffer_envelope() {
    // Test that apply requests use FlatBuffer envelopes correctly
    ensure_pqc_init().expect("PQC initialization failed");
    let (pubb, privb) = ensure_keypair_ephemeral().expect("Failed to generate keypair");

    // Create an apply request FlatBuffer
    let apply_request = protocol::machine::build_apply_request(
        3,                           // replicas
        "test-tenant",               // tenant
        "apply-op-123",              // operation_id
        "apiVersion: v1\nkind: Pod", // manifest_json (YAML as string)
        "origin-peer-id",            // origin_peer
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
    let (payload_bytes, _, _) = verification_result.unwrap();
    let parsed_apply = protocol::machine::root_as_apply_request(&payload_bytes)
        .expect("Should parse apply request from envelope payload");

    assert_eq!(parsed_apply.replicas(), 3);
    assert_eq!(parsed_apply.tenant().unwrap_or(""), "test-tenant");
    assert_eq!(parsed_apply.operation_id().unwrap_or(""), "apply-op-123");
    assert!(parsed_apply
        .manifest_json()
        .unwrap_or("")
        .contains("kind: Pod"));
}

#[test]
fn test_encrypted_manifest_envelope_handling() {
    // Test that encrypted manifests use FlatBuffer format correctly
    ensure_pqc_init().expect("PQC initialization failed");
    let (pubb, privb) = ensure_keypair_ephemeral().expect("Failed to generate keypair");

    // Create an encrypted manifest FlatBuffer
    let encrypted_manifest = protocol::machine::build_encrypted_manifest(
        "test-nonce-12345",                 // nonce
        "ZW5jcnlwdGVkX21hbmlmZXN0X2RhdGE=", // base64 encrypted payload
        "aes-gcm-256",                      // encryption_algorithm
        2,                                  // threshold
        3,                                  // total_shares
        Some("Pod"),                        // manifest_type
        &["app:test", "env:production"],    // labels
        1234567890,                         // encrypted_at
        Some("sha256:abcd1234efgh5678"),    // content_hash
    );

    let payload_type = "encrypted_manifest";
    let nonce = format!(
        "encrypted-test-{}",
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

    // Create signed envelope containing the encrypted manifest
    let canonical_bytes = build_envelope_canonical(
        &encrypted_manifest,
        payload_type,
        &nonce,
        timestamp,
        alg,
        None,
    );

    let (sig_b64, pub_b64) = sign_envelope(&privb, &pubb, &canonical_bytes)
        .expect("Failed to sign encrypted manifest envelope");

    let manifest_envelope = build_envelope_signed(
        &encrypted_manifest,
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
        &manifest_envelope,
        Duration::from_secs(300),
    );

    assert!(
        verification_result.is_ok(),
        "Encrypted manifest envelope should verify correctly: {:?}",
        verification_result.err()
    );

    // Verify we can extract and parse the encrypted manifest
    let (payload_bytes, _, _) = verification_result.unwrap();
    let parsed_manifest = protocol::machine::root_as_encrypted_manifest(&payload_bytes)
        .expect("Should parse encrypted manifest from envelope payload");

    assert_eq!(parsed_manifest.nonce().unwrap_or(""), "test-nonce-12345");
    assert_eq!(
        parsed_manifest.encryption_algorithm().unwrap_or(""),
        "aes-gcm-256"
    );
    assert_eq!(parsed_manifest.threshold(), 2);
    assert_eq!(parsed_manifest.total_shares(), 3);
    assert_eq!(parsed_manifest.manifest_type().unwrap_or(""), "Pod");
    assert_eq!(parsed_manifest.version(), 1);
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

    let (payload_bytes, _, _) = verification_result.unwrap();
    assert_eq!(payload_bytes, payload);
}
