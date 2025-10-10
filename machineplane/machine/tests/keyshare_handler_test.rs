use base64::Engine;
use crypto::{ensure_keypair_ephemeral, ensure_pqc_init, sign_envelope};
use protocol::machine::{
    build_envelope_canonical, build_envelope_signed, build_keyshare_request,
    root_as_key_share_request,
};
use std::time::Duration;

// Mock swarm and peer setup for testing
struct MockSwarm;
impl MockSwarm {
    fn new() -> Self {
        Self
    }
}

struct MockChannel;

#[tokio::test]
async fn test_keyshare_simple_envelope_handling() {
    // Test that simple flatbuffer envelopes are accepted and stored
    // - Create signed envelope with "keyshare" type
    // - Send via keyshare handler
    // - Verify stored in keystore with correct CID

    ensure_pqc_init().expect("PQC initialization failed");
    let (pubb, privb) = ensure_keypair_ephemeral().expect("Failed to generate keypair");

    let payload = b"simple keyshare test data";
    let payload_type = "keyshare";
    let nonce = "simple-keyshare-nonce";
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let alg = "ml-dsa-65";

    // Create signed envelope with "keyshare" type
    let canonical_bytes = build_envelope_canonical(payload, payload_type, nonce, timestamp, alg);

    let (sig_b64, pub_b64) =
        sign_envelope(&privb, &pubb, &canonical_bytes).expect("signing failed");

    let signed_envelope = build_envelope_signed(
        payload,
        payload_type,
        nonce,
        timestamp,
        alg,
        "ml-dsa-65",
        &sig_b64,
        &pub_b64,
    );

    // Verify the envelope can be parsed correctly
    let parsed_env = protocol::machine::root_as_envelope(&signed_envelope)
        .expect("envelope should parse correctly");

    assert_eq!(parsed_env.payload_type().unwrap_or(""), "keyshare");
    assert!(!parsed_env.sig().unwrap_or("").is_empty());

    // Verify signature verification works
    let verification_result = machine::libp2p_beemesh::envelope::verify_flatbuffer_envelope(
        &signed_envelope,
        Duration::from_secs(300),
    );

    assert!(
        verification_result.is_ok(),
        "Keyshare envelope should verify correctly: {:?}",
        verification_result.err()
    );

    // Note: We can't easily test the keystore storage without setting up the full libp2p infrastructure,
    // but we can verify that the envelope format is correct and would be accepted by the handler
}

#[test]
fn test_keyshare_format_rejection() {
    // Test that invalid formats are properly rejected
    // - Send malformed flatbuffer (should be rejected)
    // - Send unsigned envelope (should be rejected)

    ensure_pqc_init().expect("PQC initialization failed");

    // Test 1: Malformed flatbuffer should be rejected
    let malformed_bytes = b"this is not a valid flatbuffer";
    let malformed_result = protocol::machine::root_as_envelope(malformed_bytes);
    assert!(
        malformed_result.is_err(),
        "Malformed flatbuffer should not parse"
    );

    // Test 2: Unsigned envelope should be rejected
    let unsigned_envelope = build_envelope_canonical(
        b"unsigned payload",
        "keyshare",
        "unsigned-nonce",
        1234567890,
        "ml-dsa-65",
    );

    // Try to verify unsigned envelope - should fail
    let unsigned_result = machine::libp2p_beemesh::envelope::verify_flatbuffer_envelope(
        &unsigned_envelope,
        Duration::from_secs(300),
    );

    assert!(
        unsigned_result.is_err(),
        "Unsigned envelope should fail verification"
    );

    // Test 3: Invalid signature should be rejected
    let (pubb, privb) = ensure_keypair_ephemeral().expect("Failed to generate keypair");
    let payload = b"test payload";
    let canonical = build_envelope_canonical(
        payload,
        "keyshare",
        "invalid-sig-test",
        1234567890,
        "ml-dsa-65",
    );

    let (sig_b64, pub_b64) = sign_envelope(&privb, &pubb, &canonical).expect("signing failed");

    // Create envelope with tampered signature
    let invalid_envelope = build_envelope_signed(
        payload,
        "keyshare",
        "invalid-sig-test",
        1234567890,
        "ml-dsa-65",
        "ml-dsa-65",
        "dGFtcGVyZWRzaWc=", // "tamperedsig" in base64 - invalid signature
        &pub_b64,
    );

    let invalid_result = machine::libp2p_beemesh::envelope::verify_flatbuffer_envelope(
        &invalid_envelope,
        Duration::from_secs(300),
    );

    assert!(
        invalid_result.is_err(),
        "Invalid signature should fail verification"
    );
}

#[test]
fn test_keyshare_kem_vs_simple_paths() {
    // Test both KEM-encapsulated and simple envelope paths work
    // - Test KEM-encapsulated KeyShareRequest path
    // - Test simple envelope path
    // - Verify both store correctly in keystore

    ensure_pqc_init().expect("PQC initialization failed");
    let (pubb, privb) = ensure_keypair_ephemeral().expect("Failed to generate keypair");

    // Test 1: Simple envelope path
    let simple_payload = b"simple path test data";
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    let simple_canonical = build_envelope_canonical(
        simple_payload,
        "keyshare",
        "simple-path-nonce",
        timestamp,
        "ml-dsa-65",
    );

    let (sig_b64, pub_b64) =
        sign_envelope(&privb, &pubb, &simple_canonical).expect("signing failed");

    let simple_envelope = build_envelope_signed(
        simple_payload,
        "keyshare",
        "simple-path-nonce",
        timestamp,
        "ml-dsa-65",
        "ml-dsa-65",
        &sig_b64,
        &pub_b64,
    );

    // Verify simple envelope path works
    let simple_result = machine::libp2p_beemesh::envelope::verify_flatbuffer_envelope(
        &simple_envelope,
        Duration::from_secs(300),
    );

    assert!(
        simple_result.is_ok(),
        "Simple envelope should verify correctly"
    );

    // Test 2: KEM-encapsulated path (KeyShareRequest)
    // Create a KeyShareRequest flatbuffer
    let manifest_id = "test-manifest-id";
    let capability_data = "test-capability-data";

    let kem_request = build_keyshare_request(manifest_id, capability_data);

    // Wrap in envelope and sign
    let kem_canonical = build_envelope_canonical(
        &kem_request,
        "keyshare",
        "kem-path-nonce",
        timestamp + 1,
        "ml-dsa-65",
    );

    let (kem_sig_b64, kem_pub_b64) =
        sign_envelope(&privb, &pubb, &kem_canonical).expect("KEM signing failed");

    let kem_envelope = build_envelope_signed(
        &kem_request,
        "keyshare",
        "kem-path-nonce",
        timestamp + 1,
        "ml-dsa-65",
        "ml-dsa-65",
        &kem_sig_b64,
        &kem_pub_b64,
    );

    // Verify KEM envelope path works
    let kem_result = machine::libp2p_beemesh::envelope::verify_flatbuffer_envelope(
        &kem_envelope,
        Duration::from_secs(300),
    );

    assert!(kem_result.is_ok(), "KEM envelope should verify correctly");

    // Verify the KEM payload can be parsed as KeyShareRequest
    let kem_env = protocol::machine::root_as_envelope(&kem_envelope).expect("parse KEM envelope");
    let payload_bytes = kem_env.payload().expect("KEM envelope should have payload");
    let payload_vec: Vec<u8> = payload_bytes.iter().collect();

    let keyshare_request = root_as_key_share_request(&payload_vec);
    assert!(
        keyshare_request.is_ok(),
        "KEM payload should parse as KeyShareRequest"
    );
}

#[test]
fn test_keyshare_capability_verification() {
    // Test that capability envelopes are handled correctly
    // Capabilities should use flatbuffer format but be treated specially

    ensure_pqc_init().expect("PQC initialization failed");
    let (pubb, privb) = ensure_keypair_ephemeral().expect("Failed to generate keypair");

    let capability_payload = b"capability token data";
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    let capability_canonical = build_envelope_canonical(
        capability_payload,
        "capability",
        "capability-nonce",
        timestamp,
        "ml-dsa-65",
    );

    let (sig_b64, pub_b64) =
        sign_envelope(&privb, &pubb, &capability_canonical).expect("signing failed");

    let capability_envelope = build_envelope_signed(
        capability_payload,
        "capability",
        "capability-nonce",
        timestamp,
        "ml-dsa-65",
        "ml-dsa-65",
        &sig_b64,
        &pub_b64,
    );

    // Capabilities are typically base64 encoded for transport
    let capability_b64 = base64::engine::general_purpose::STANDARD.encode(&capability_envelope);

    // Decode and verify
    let decoded_capability = base64::engine::general_purpose::STANDARD
        .decode(&capability_b64)
        .expect("capability should decode from base64");

    // Verify the decoded capability envelope
    let capability_verification = machine::libp2p_beemesh::envelope::verify_flatbuffer_envelope(
        &decoded_capability,
        Duration::from_secs(300),
    );

    assert!(
        capability_verification.is_ok(),
        "Capability envelope should verify correctly: {:?}",
        capability_verification.err()
    );

    // Verify it's recognized as a capability
    let capability_env =
        protocol::machine::root_as_envelope(&decoded_capability).expect("parse capability");
    assert_eq!(
        capability_env.payload_type().unwrap_or(""),
        "capability",
        "Should be recognized as capability type"
    );
}

#[test]
fn test_keyshare_envelope_with_encryption() {
    // Test the complete flow: envelope creation, signing, verification, and storage preparation
    ensure_pqc_init().expect("PQC initialization failed");
    let (pubb, privb) = ensure_keypair_ephemeral().expect("Failed to generate keypair");

    let payload = b"encrypted keyshare test data";
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    let canonical = build_envelope_canonical(
        payload,
        "keyshare",
        "encryption-test-nonce",
        timestamp,
        "ml-dsa-65",
    );

    let (sig_b64, pub_b64) = sign_envelope(&privb, &pubb, &canonical).expect("signing failed");

    let signed_envelope = build_envelope_signed(
        payload,
        "keyshare",
        "encryption-test-nonce",
        timestamp,
        "ml-dsa-65",
        "ml-dsa-65",
        &sig_b64,
        &pub_b64,
    );

    // Verify envelope and extract payload for storage
    let (verified_payload, _pub_bytes, _sig_bytes) =
        machine::libp2p_beemesh::envelope::verify_flatbuffer_envelope(
            &signed_envelope,
            Duration::from_secs(300),
        )
        .expect("envelope should verify");

    // Test that the payload can be prepared for keystore encryption
    let encryption_result = crypto::encrypt_share_for_keystore(&verified_payload);
    assert!(
        encryption_result.is_ok(),
        "Should be able to encrypt verified payload for keystore: {:?}",
        encryption_result.err()
    );

    if let Ok((encrypted_blob, cid)) = encryption_result {
        assert!(
            !encrypted_blob.is_empty(),
            "Encrypted blob should not be empty"
        );
        assert!(!cid.is_empty(), "CID should not be empty");

        // Test that we can decrypt it back - need KEM private key for this
        let (_kem_pub, kem_priv) = crypto::ensure_kem_keypair_on_disk().expect("KEM keypair");
        let decryption_result = crypto::decrypt_share_from_blob(&encrypted_blob, &kem_priv);
        assert!(
            decryption_result.is_ok(),
            "Should be able to decrypt the blob back"
        );

        if let Ok(decrypted_data) = decryption_result {
            assert_eq!(
                decrypted_data, verified_payload,
                "Decrypted data should match original payload"
            );
        }
    }
}

#[test]
fn test_keyshare_signature_prefix_handling() {
    // Test that signature prefixes are handled correctly
    ensure_pqc_init().expect("PQC initialization failed");
    let (pubb, privb) = ensure_keypair_ephemeral().expect("Failed to generate keypair");

    let payload = b"prefix test payload";
    let canonical = build_envelope_canonical(
        payload,
        "keyshare",
        "prefix-test-nonce",
        1234567890,
        "ml-dsa-65",
    );

    let (sig_b64, pub_b64) = sign_envelope(&privb, &pubb, &canonical).expect("signing failed");

    // Test with explicit prefix
    let envelope_with_prefix = build_envelope_signed(
        payload,
        "keyshare",
        "prefix-test-nonce",
        1234567890,
        "ml-dsa-65",
        "ml-dsa-65", // explicit prefix
        &sig_b64,
        &pub_b64,
    );

    let result_with_prefix = machine::libp2p_beemesh::envelope::verify_flatbuffer_envelope(
        &envelope_with_prefix,
        Duration::from_secs(300),
    );

    assert!(
        result_with_prefix.is_ok(),
        "Envelope with explicit prefix should verify"
    );

    // Verify the signature field includes the prefix
    let env = protocol::machine::root_as_envelope(&envelope_with_prefix).expect("parse envelope");
    let sig_field = env.sig().unwrap_or("");
    assert!(
        sig_field.contains("ml-dsa-65:"),
        "Signature field should contain prefix: {}",
        sig_field
    );
}

#[test]
fn test_keyshare_encrypted_manifest_handling() {
    // Test handling of FlatBuffer EncryptedManifest format
    ensure_pqc_init().expect("PQC initialization failed");

    let nonce = "test-nonce-12345";
    let encrypted_payload = "dGVzdCBlbmNyeXB0ZWQgcGF5bG9hZA=="; // base64 encoded test data
    let encryption_algorithm = "aes-gcm-256";
    let threshold = 2u32;
    let total_shares = 3u32;
    let manifest_type = Some("Pod");
    let labels = vec!["app:test", "env:dev"];
    let encrypted_at = 1234567890u64;
    let content_hash = Some("sha256:abcd1234");

    // Build encrypted manifest
    let encrypted_manifest = protocol::machine::build_encrypted_manifest(
        nonce,
        encrypted_payload,
        encryption_algorithm,
        threshold,
        total_shares,
        manifest_type,
        &labels,
        encrypted_at,
        content_hash,
    );

    // Verify it can be parsed back correctly
    let parsed_manifest = protocol::machine::root_as_encrypted_manifest(&encrypted_manifest)
        .expect("should parse encrypted manifest");

    assert_eq!(parsed_manifest.nonce().unwrap_or(""), nonce);
    assert_eq!(parsed_manifest.payload().unwrap_or(""), encrypted_payload);
    assert_eq!(
        parsed_manifest.encryption_algorithm().unwrap_or(""),
        encryption_algorithm
    );
    assert_eq!(parsed_manifest.threshold(), threshold);
    assert_eq!(parsed_manifest.total_shares(), total_shares);
    assert_eq!(parsed_manifest.manifest_type().unwrap_or(""), "Pod");
    assert_eq!(parsed_manifest.encrypted_at(), encrypted_at);
    assert_eq!(
        parsed_manifest.content_hash().unwrap_or(""),
        "sha256:abcd1234"
    );
    assert_eq!(parsed_manifest.version(), 1);

    // Check labels
    if let Some(labels_fb) = parsed_manifest.labels() {
        let parsed_labels: Vec<&str> = (0..labels_fb.len()).map(|i| labels_fb.get(i)).collect();
        assert_eq!(parsed_labels, labels);
    } else {
        panic!("Labels should be present");
    }
}

#[test]
fn test_keyshare_replay_protection() {
    // Test nonce-based replay protection
    ensure_pqc_init().expect("PQC initialization failed");
    let (pubb, privb) = ensure_keypair_ephemeral().expect("Failed to generate keypair");

    let payload = b"replay protection test";
    let nonce = format!(
        "replay-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    let canonical = build_envelope_canonical(payload, "keyshare", &nonce, timestamp, "ml-dsa-65");

    let (sig_b64, pub_b64) = sign_envelope(&privb, &pubb, &canonical).expect("signing failed");

    let envelope = build_envelope_signed(
        payload,
        "keyshare",
        &nonce,
        timestamp,
        "ml-dsa-65",
        "ml-dsa-65",
        &sig_b64,
        &pub_b64,
    );

    // First verification should succeed
    let first_result = machine::libp2p_beemesh::envelope::verify_flatbuffer_envelope(
        &envelope,
        Duration::from_secs(300),
    );
    assert!(first_result.is_ok(), "First verification should succeed");

    // Second verification with same nonce should fail (replay protection)
    let second_result = machine::libp2p_beemesh::envelope::verify_flatbuffer_envelope(
        &envelope,
        Duration::from_secs(300),
    );
    assert!(
        second_result.is_err(),
        "Replay should be detected and rejected"
    );
}
