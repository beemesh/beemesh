use anyhow::Result;
use base64::prelude::*;
use crypto::{ensure_keypair_ephemeral, ensure_pqc_init};
use libp2p::PeerId;
use protocol::machine::{
    build_capability_token, build_capability_token_with_holder_signature, build_envelope_canonical,
    build_envelope_signed, build_keyshare_request, root_as_capability_token,
    root_as_key_share_request,
};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static TEST_MUTEX: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();

#[tokio::test]
async fn test_capability_holder_signature_end_to_end() -> Result<()> {
    let _guard = TEST_MUTEX
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    ensure_pqc_init()?;

    // Setup test data
    let manifest_id = "test_manifest_123";
    let issuer_peer_id = "12D3KooWIssuerPeerIdExample123";
    let _authorized_peer_id = "12D3KooWAuthorizedPeerExample123";
    let holder_peer_id = PeerId::random();

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    // Generate keypairs for issuer and holder
    let (issuer_pub, issuer_priv) = ensure_keypair_ephemeral()?;
    let (holder_pub, holder_priv) = ensure_keypair_ephemeral()?;

    println!("Test: Creating original capability token...");

    // 1. Create original capability token (issued by CLI/issuer)
    let original_token = build_capability_token(
        manifest_id,
        issuer_peer_id,
        &holder_peer_id.to_string(), // The holder is the authorized peer
        timestamp,
        timestamp + 300_000, // 5 minutes expiry
    );

    // 2. Create envelope for the original token
    let nonce_bytes: [u8; 16] = rand::random();
    let nonce_str = BASE64_STANDARD.encode(&nonce_bytes);

    let canonical = build_envelope_canonical(
        &original_token,
        "capability",
        &nonce_str,
        timestamp,
        "ml-dsa-65",
        None,
    );

    let (sig_b64, pub_b64) = crypto::sign_envelope(&issuer_priv, &issuer_pub, &canonical)?;

    let _signed_envelope = build_envelope_signed(
        &original_token,
        "capability",
        &nonce_str,
        timestamp,
        "ml-dsa-65",
        "ml-dsa-65",
        &sig_b64,
        &pub_b64,
        None,
    );

    println!("Test: Creating holder-signed capability token...");

    // 3. Holder adds their signature (simulating presentation)
    let presentation_nonce = format!("holder_sig_{}", rand::random::<u64>());
    let presentation_timestamp = timestamp + 1000; // Slightly later

    let presentation_context = crypto::create_capability_presentation_context(
        &original_token,
        &presentation_nonce,
        presentation_timestamp,
        manifest_id,
        "KeyShareRequest",
    );

    let (holder_sig_b64, holder_pub_b64) =
        crypto::sign_capability_presentation(&holder_priv, &holder_pub, &presentation_context)?;

    let holder_sig_bytes = BASE64_STANDARD.decode(holder_sig_b64)?;
    let holder_pub_bytes = BASE64_STANDARD.decode(holder_pub_b64)?;

    // 4. Create new capability token with holder signature
    let signed_token = build_capability_token_with_holder_signature(
        manifest_id,
        issuer_peer_id,
        &holder_peer_id.to_string(),
        timestamp,
        timestamp + 300_000,
        &holder_peer_id.to_string(),
        &holder_pub_bytes,
        &holder_sig_bytes,
        &presentation_nonce,
        presentation_timestamp,
    );

    // 5. Create new envelope for the holder-signed token
    let new_nonce_bytes: [u8; 16] = rand::random();
    let new_nonce_str = BASE64_STANDARD.encode(&new_nonce_bytes);

    let new_canonical = build_envelope_canonical(
        &signed_token,
        "capability",
        &new_nonce_str,
        presentation_timestamp,
        "ml-dsa-65",
        None,
    );

    let (new_sig_b64, new_pub_b64) =
        crypto::sign_envelope(&holder_priv, &holder_pub, &new_canonical)?;

    let final_envelope = build_envelope_signed(
        &signed_token,
        "capability",
        &new_nonce_str,
        presentation_timestamp,
        "ml-dsa-65",
        "ml-dsa-65",
        &new_sig_b64,
        &new_pub_b64,
        None,
    );

    println!("Test: Creating KeyShareRequest with holder-signed capability...");

    // 6. Create KeyShareRequest with holder-signed capability
    let capability_b64 = BASE64_STANDARD.encode(&final_envelope);
    let keyshare_request = build_keyshare_request(manifest_id, &capability_b64);

    println!("Test: Verifying KeyShareRequest parsing...");

    // 7. Verify the request can be parsed (simulating receiver side)
    let parsed_request = root_as_key_share_request(&keyshare_request)?;

    assert_eq!(parsed_request.manifest_id().unwrap(), manifest_id);
    assert!(parsed_request.capability().is_some());

    let received_capability_b64 = parsed_request.capability().unwrap();
    let received_capability_bytes = BASE64_STANDARD.decode(received_capability_b64)?;

    println!("Test: Verifying envelope signature...");

    // 8. Verify envelope signature
    let (token_bytes, _env_pub, _env_sig) =
        machine::libp2p_beemesh::envelope::verify_flatbuffer_envelope(
            &received_capability_bytes,
            std::time::Duration::from_secs(300),
        )?;

    println!("Test: Verifying capability token structure...");

    // 9. Verify capability token structure and holder signature
    let capability_token = root_as_capability_token(&token_bytes)?;

    // Check basic token fields
    let root_cap = capability_token.root_capability().unwrap();
    assert_eq!(root_cap.manifest_id().unwrap(), manifest_id);
    assert_eq!(root_cap.issuer_peer_id().unwrap(), issuer_peer_id);

    // Check signature chain exists and has holder signature
    let signature_chain = capability_token.signature_chain().unwrap();
    assert_eq!(signature_chain.len(), 1);

    let sig_entry = signature_chain.get(0);
    assert_eq!(
        sig_entry.signer_peer_id().unwrap(),
        holder_peer_id.to_string()
    );
    assert_eq!(sig_entry.presentation_nonce().unwrap(), presentation_nonce);
    assert_eq!(sig_entry.presentation_timestamp(), presentation_timestamp);

    println!("Test: Verifying holder signature...");

    // 10. Verify holder signature
    let stored_holder_pub: Vec<u8> = sig_entry.public_key().unwrap().iter().collect();
    let stored_holder_sig: Vec<u8> = sig_entry.signature().unwrap().iter().collect();

    // Reconstruct presentation context
    let reconstructed_context = crypto::create_capability_presentation_context(
        &original_token, // Use original token bytes
        &presentation_nonce,
        presentation_timestamp,
        manifest_id,
        "KeyShareRequest",
    );

    // Verify signature
    crypto::verify_capability_holder_signature(
        &stored_holder_pub,
        &reconstructed_context,
        &stored_holder_sig,
    )?;

    println!("Test: Verifying authorization...");

    // 11. Verify authorization (holder is authorized peer)
    let caveats = capability_token.caveats().unwrap();
    assert!(caveats.len() > 0);

    let caveat = caveats.get(0);
    assert_eq!(caveat.condition_type().unwrap(), "authorized_peer");

    let authorized_bytes: Vec<u8> = caveat.value().unwrap().iter().collect();
    let authorized_peer = std::str::from_utf8(&authorized_bytes)?;
    assert_eq!(authorized_peer, holder_peer_id.to_string());

    println!("Test: All verifications passed!");

    // 12. Test failure case - wrong signer should fail
    let (wrong_pub, wrong_priv) = ensure_keypair_ephemeral()?;
    let wrong_context = crypto::create_capability_presentation_context(
        &original_token,
        "wrong_nonce",
        presentation_timestamp,
        manifest_id,
        "KeyShareRequest",
    );

    let (wrong_sig_b64, _) =
        crypto::sign_capability_presentation(&wrong_priv, &wrong_pub, &wrong_context)?;
    let wrong_sig_bytes = BASE64_STANDARD.decode(wrong_sig_b64)?;

    // This should fail
    assert!(crypto::verify_capability_holder_signature(
        &stored_holder_pub,
        &wrong_context,
        &wrong_sig_bytes,
    )
    .is_err());

    println!("Test: Negative test cases passed!");

    Ok(())
}

#[tokio::test]
async fn test_capability_replay_attack_prevention() -> Result<()> {
    let _guard = TEST_MUTEX
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    ensure_pqc_init()?;

    let manifest_id = "replay_test_manifest";
    let holder_peer_id = PeerId::random();
    let (holder_pub, holder_priv) = ensure_keypair_ephemeral()?;

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    // Create original token
    let original_token = build_capability_token(
        manifest_id,
        "issuer",
        &holder_peer_id.to_string(),
        timestamp,
        timestamp + 300_000,
    );

    println!("Test: Creating two different presentation contexts (different nonces)...");

    // Create first presentation
    let nonce_1 = "presentation_nonce_1";
    let context_1 = crypto::create_capability_presentation_context(
        &original_token,
        nonce_1,
        timestamp,
        manifest_id,
        "KeyShareRequest",
    );

    let (sig_1_b64, _) =
        crypto::sign_capability_presentation(&holder_priv, &holder_pub, &context_1)?;
    let sig_1_bytes = BASE64_STANDARD.decode(sig_1_b64)?;

    // Create second presentation with different nonce
    let nonce_2 = "presentation_nonce_2";
    let context_2 = crypto::create_capability_presentation_context(
        &original_token,
        nonce_2,
        timestamp,
        manifest_id,
        "KeyShareRequest",
    );

    println!("Test: Verifying signature 1 works with context 1...");
    // Signature 1 should work with context 1
    assert!(
        crypto::verify_capability_holder_signature(&holder_pub, &context_1, &sig_1_bytes,).is_ok()
    );

    println!("Test: Verifying signature 1 fails with context 2 (replay prevention)...");
    // Signature 1 should NOT work with context 2 (prevents replay)
    assert!(
        crypto::verify_capability_holder_signature(&holder_pub, &context_2, &sig_1_bytes,).is_err()
    );

    println!("Test: Replay attack prevention verified!");

    Ok(())
}
