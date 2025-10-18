use anyhow::Context;
use base64::Engine;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

static NONCE_STORE: OnceLock<Mutex<HashMap<String, HashMap<String, Instant>>>> = OnceLock::new();

fn nonce_store() -> &'static Mutex<HashMap<String, HashMap<String, Instant>>> {
    NONCE_STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Accept a signature string potentially prefixed (e.g. "ml-dsa-65:<b64>") and return decoded bytes.
/// Keeps prefix-handling logic centralized.
pub fn normalize_and_decode_signature(sig_opt: Option<&str>) -> anyhow::Result<Vec<u8>> {
    let sig_str = sig_opt.unwrap_or("");
    let b64_part = if let Some(idx) = sig_str.find(':') {
        &sig_str[idx + 1..]
    } else {
        sig_str
    };
    base64::engine::general_purpose::STANDARD
        .decode(b64_part)
        .context("failed to base64-decode signature")
}

/// Check replay protection: ensure nonce is not seen in `nonce_window` and insert it.
/// Returns Err if duplicate or invalid.
pub fn check_and_insert_nonce(nonce_str: &str, nonce_window: Duration) -> anyhow::Result<()> {
    check_and_insert_nonce_for_peer(nonce_str, nonce_window, "global")
}

/// Check replay protection for a specific peer: ensure nonce is not seen in `nonce_window` and insert it.
/// Returns Err if duplicate or invalid.
pub fn check_and_insert_nonce_for_peer(
    nonce_str: &str,
    nonce_window: Duration,
    peer_id: &str,
) -> anyhow::Result<()> {
    if nonce_str.is_empty() {
        return Err(anyhow::anyhow!("nonce cannot be empty"));
    }

    let now = Instant::now();
    let mut store = nonce_store().lock().unwrap();

    // Get or create peer-specific nonce store
    let peer_store = store
        .entry(peer_id.to_string())
        .or_insert_with(HashMap::new);

    // prune old nonces for this peer
    peer_store.retain(|_, &mut t| now.duration_since(t) <= nonce_window);

    if peer_store.contains_key(nonce_str) {
        return Err(anyhow::anyhow!("replay detected: nonce already seen"));
    }
    peer_store.insert(nonce_str.to_string(), now);
    Ok(())
}

/// Verify a flatbuffer envelope. Reconstructs canonical bytes and verifies signature.
/// Returns payload bytes, pub bytes, sig bytes.
pub fn verify_flatbuffer_envelope(
    fb_envelope_bytes: &[u8],
    nonce_window: Duration,
) -> anyhow::Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    verify_flatbuffer_envelope_for_peer(fb_envelope_bytes, nonce_window, "global")
}

/// Verify a flatbuffer envelope for a specific peer. Reconstructs canonical bytes and verifies signature.
/// Returns payload bytes, pub bytes, sig bytes.
pub fn verify_flatbuffer_envelope_for_peer(
    fb_envelope_bytes: &[u8],
    nonce_window: Duration,
    peer_id: &str,
) -> anyhow::Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    let fb_env = protocol::machine::root_as_envelope(fb_envelope_bytes)
        .context("failed to parse flatbuffer envelope")?;

    let sig_str = fb_env.sig().unwrap_or("");
    let pub_str = fb_env.pubkey().unwrap_or("");

    let sig_bytes = normalize_and_decode_signature(Some(sig_str))?;
    let pub_bytes = base64::engine::general_purpose::STANDARD
        .decode(pub_str)
        .context("failed to base64-decode pubkey")?;

    // Reconstruct canonical bytes using the same method as signing
    let payload_vec = fb_env
        .payload()
        .map(|b| b.iter().collect::<Vec<u8>>())
        .unwrap_or_default();
    let payload_type = fb_env.payload_type().unwrap_or("");
    let nonce = fb_env.nonce().unwrap_or("");
    let ts = fb_env.ts();
    let alg = fb_env.alg().unwrap_or("");

    let canonical = protocol::machine::build_envelope_canonical(
        &payload_vec,
        payload_type,
        nonce,
        ts,
        alg,
        None,
    );

    if !nonce.is_empty() {
        check_and_insert_nonce_for_peer(nonce, nonce_window, peer_id)?;
    }

    crypto::verify_envelope(&pub_bytes, &canonical, &sig_bytes)
        .context("flatbuffer signature verification failed")?;

    Ok((payload_vec, pub_bytes, sig_bytes))
}

/// Verify a flatbuffer envelope without nonce replay checking.
/// This is used when extracting tokens for re-signing, where the same envelope
/// may be processed multiple times legitimately.
/// Returns payload bytes, pub bytes, sig bytes.
pub fn verify_flatbuffer_envelope_skip_nonce_check(
    fb_envelope_bytes: &[u8],
) -> anyhow::Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    let fb_env = protocol::machine::root_as_envelope(fb_envelope_bytes)
        .context("failed to parse flatbuffer envelope")?;

    let sig_str = fb_env.sig().unwrap_or("");
    let pub_str = fb_env.pubkey().unwrap_or("");

    let sig_bytes = normalize_and_decode_signature(Some(sig_str))?;
    let pub_bytes = base64::engine::general_purpose::STANDARD
        .decode(pub_str)
        .context("failed to base64-decode pubkey")?;

    // Reconstruct canonical bytes using the same method as signing
    let payload_vec = fb_env
        .payload()
        .map(|b| b.iter().collect::<Vec<u8>>())
        .unwrap_or_default();
    let payload_type = fb_env.payload_type().unwrap_or("");
    let nonce = fb_env.nonce().unwrap_or("");
    let ts = fb_env.ts();
    let alg = fb_env.alg().unwrap_or("");

    let canonical = protocol::machine::build_envelope_canonical(
        &payload_vec,
        payload_type,
        nonce,
        ts,
        alg,
        None,
    );

    // Skip nonce replay check - this is intentional for token extraction

    crypto::verify_envelope(&pub_bytes, &canonical, &sig_bytes)
        .context("flatbuffer signature verification failed")?;

    Ok((payload_vec, pub_bytes, sig_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crypto::{ensure_keypair_ephemeral, ensure_pqc_init, sign_envelope};
    use std::time::Duration;

    #[test]
    fn test_build_and_verify_flatbuffer_envelope_roundtrip() {
        ensure_pqc_init().unwrap();
        let (pubb, privb) = ensure_keypair_ephemeral().expect("keypair");

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

        // Create and sign flatbuffer envelope
        let fb_canonical = protocol::machine::build_envelope_canonical(
            payload,
            payload_type,
            &nonce,
            timestamp,
            alg,
            None,
        );
        let (sig_b64, pub_b64) = sign_envelope(&privb, &pubb, &fb_canonical).expect("sign");

        // Build the final signed flatbuffer envelope (fb_envelope) using the original payload
        let fb_envelope = protocol::machine::build_envelope_signed(
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

        // Verify the envelope using the fb_envelope and ensure the extracted payload matches
        let (payload_bytes, _pub, _sig) =
            verify_flatbuffer_envelope(&fb_envelope, Duration::from_secs(300)).expect("verify");

        assert_eq!(payload_bytes, payload);

        // Replay should fail when verifying the same envelope again
        assert!(verify_flatbuffer_envelope(&fb_envelope, Duration::from_secs(300)).is_err());
    }

    #[test]
    fn test_nonce_replay_protection() {
        let nonce = "test-nonce-unique";
        let window = Duration::from_secs(60);

        // First use should succeed
        assert!(check_and_insert_nonce(nonce, window).is_ok());

        // Second use should fail (replay)
        assert!(check_and_insert_nonce(nonce, window).is_err());
    }

    #[test]
    fn test_signature_prefix_handling() {
        // Test with prefix
        let sig_with_prefix = "ml-dsa-65:dGVzdA=="; // "test" in base64
        let decoded = normalize_and_decode_signature(Some(sig_with_prefix)).unwrap();
        assert_eq!(decoded, b"test");

        // Test without prefix
        let sig_without_prefix = "dGVzdA==";
        let decoded = normalize_and_decode_signature(Some(sig_without_prefix)).unwrap();
        assert_eq!(decoded, b"test");
    }
}
