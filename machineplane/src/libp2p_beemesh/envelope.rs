use anyhow::Context;
use base64::Engine;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// Signed envelope bytes alongside the generated nonce/timestamp.
pub struct SignedEnvelope {
    pub bytes: Vec<u8>,
    pub nonce: String,
    pub timestamp: u64,
}

/// Configuration options for signing envelopes.
pub struct SignEnvelopeConfig<'a> {
    pub nonce: Option<&'a str>,
    pub timestamp: Option<u64>,
    pub peer_id: Option<&'a str>,
    pub kem_pub_b64: Option<&'a str>,
    pub algorithm: &'a str,
    pub signature_prefix: &'a str,
}

impl<'a> Default for SignEnvelopeConfig<'a> {
    fn default() -> Self {
        Self {
            nonce: None,
            timestamp: None,
            peer_id: None,
            kem_pub_b64: None,
            algorithm: "ml-dsa-65",
            signature_prefix: "ml-dsa-65",
        }
    }
}

/// Parsed components from a verified envelope.
#[derive(Clone, Debug)]
pub struct VerifiedEnvelopeParts {
    pub payload: Vec<u8>,
    pub pubkey: Vec<u8>,
    pub signature: Vec<u8>,
    pub timestamp_ms: u64,
    pub payload_type: String,
}

/// Sign `payload` using the node's persisted signing keypair.
pub fn sign_with_node_keys(
    payload: &[u8],
    payload_type: &str,
    config: SignEnvelopeConfig<'_>,
) -> anyhow::Result<SignedEnvelope> {
    let (pub_bytes, sk_bytes) = crate::crypto::ensure_keypair_on_disk()?;
    sign_with_existing_keypair(payload, payload_type, config, &pub_bytes, &sk_bytes)
}

/// Sign `payload` using a provided `(pub, secret)` keypair byte slices.
pub fn sign_with_existing_keypair(
    payload: &[u8],
    payload_type: &str,
    config: SignEnvelopeConfig<'_>,
    pub_bytes: &[u8],
    sk_bytes: &[u8],
) -> anyhow::Result<SignedEnvelope> {
    sign_internal(payload, payload_type, config, pub_bytes, sk_bytes)
}

fn sign_internal(
    payload: &[u8],
    payload_type: &str,
    config: SignEnvelopeConfig<'_>,
    pub_bytes: &[u8],
    sk_bytes: &[u8],
) -> anyhow::Result<SignedEnvelope> {
    let SignEnvelopeConfig {
        nonce,
        timestamp,
        peer_id,
        kem_pub_b64,
        algorithm,
        signature_prefix,
    } = config;

    let nonce_owned = nonce
        .map(|value| value.to_string())
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    let timestamp = timestamp.unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0u64)
    });

    let canonical = if let Some(peer) = peer_id {
        crate::protocol::machine::build_envelope_canonical_with_peer(
            payload,
            payload_type,
            &nonce_owned,
            timestamp,
            algorithm,
            peer,
            kem_pub_b64,
        )
    } else {
        crate::protocol::machine::build_envelope_canonical(
            payload,
            payload_type,
            &nonce_owned,
            timestamp,
            algorithm,
            kem_pub_b64,
        )
    };

    let (sig_b64, pub_b64) = crate::crypto::sign_envelope(sk_bytes, pub_bytes, &canonical)?;

    let bytes = if let Some(peer) = peer_id {
        crate::protocol::machine::build_envelope_signed_with_peer(
            payload,
            payload_type,
            &nonce_owned,
            timestamp,
            algorithm,
            signature_prefix,
            &sig_b64,
            &pub_b64,
            peer,
            kem_pub_b64,
        )
    } else {
        crate::protocol::machine::build_envelope_signed(
            payload,
            payload_type,
            &nonce_owned,
            timestamp,
            algorithm,
            signature_prefix,
            &sig_b64,
            &pub_b64,
            kem_pub_b64,
        )
    };

    Ok(SignedEnvelope {
        bytes,
        nonce: nonce_owned,
        timestamp,
    })
}

/// Accept a signature string potentially prefixed (e.g. "ml-dsa-65:<b64>") and return decoded bytes.
/// Keeps prefix-handling logic centralized.
pub fn normalize_and_decode_signature(sig_opt: Option<&str>) -> anyhow::Result<Vec<u8>> {
    crate::crypto::flatbuffer_envelope::normalize_and_decode_signature(sig_opt)
}

/// Check replay protection: ensure nonce is not seen in `nonce_window` and insert it.
/// Returns Err if duplicate or invalid.
pub fn check_and_insert_nonce(nonce_str: &str, nonce_window: Duration) -> anyhow::Result<()> {
    crate::crypto::flatbuffer_envelope::check_and_insert_nonce(nonce_str, nonce_window)
}

/// Check replay protection for a specific peer: ensure nonce is not seen in `nonce_window` and insert it.
/// Returns Err if duplicate or invalid.
pub fn check_and_insert_nonce_for_peer(
    nonce_str: &str,
    nonce_window: Duration,
    peer_id: &str,
) -> anyhow::Result<()> {
    crate::crypto::flatbuffer_envelope::check_and_insert_nonce_for_peer(
        nonce_str,
        nonce_window,
        peer_id,
    )
}

/// Verify a flatbuffer envelope. Reconstructs canonical bytes and verifies signature.
/// Returns verified envelope parts including payload bytes, signing key and timestamp.
pub fn verify_flatbuffer_envelope(
    fb_envelope_bytes: &[u8],
    nonce_window: Duration,
) -> anyhow::Result<VerifiedEnvelopeParts> {
    verify_flatbuffer_envelope_for_peer(fb_envelope_bytes, nonce_window, "global")
}

/// Verify a flatbuffer envelope for a specific peer. Reconstructs canonical bytes and verifies signature.
/// Returns verified envelope parts including payload bytes, signing key and timestamp.
pub fn verify_flatbuffer_envelope_for_peer(
    fb_envelope_bytes: &[u8],
    nonce_window: Duration,
    peer_id: &str,
) -> anyhow::Result<VerifiedEnvelopeParts> {
    let fb_env = crate::protocol::machine::root_as_envelope(fb_envelope_bytes)
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

    // Reconstruct canonical bytes using the same method as signing
    // Check if envelope has peer_id and kem_pubkey fields to match signing format
    let peer_id_opt = fb_env.peer_id();
    let kem_pubkey_opt = fb_env.kem_pubkey();

    let canonical = if peer_id_opt.is_some() {
        // Use peer-aware canonical form reconstruction
        crate::protocol::machine::build_envelope_canonical_with_peer(
            &payload_vec,
            payload_type,
            nonce,
            ts,
            alg,
            peer_id_opt.unwrap_or(""),
            kem_pubkey_opt,
        )
    } else {
        // Use standard canonical form reconstruction
        crate::protocol::machine::build_envelope_canonical(
            &payload_vec,
            payload_type,
            nonce,
            ts,
            alg,
            kem_pubkey_opt,
        )
    };

    if !nonce.is_empty() {
        check_and_insert_nonce_for_peer(nonce, nonce_window, peer_id)?;
    }

    crate::crypto::verify_envelope(&pub_bytes, &canonical, &sig_bytes)
        .context("flatbuffer signature verification failed")?;

    Ok(VerifiedEnvelopeParts {
        payload: payload_vec,
        pubkey: pub_bytes,
        signature: sig_bytes,
        timestamp_ms: ts,
        payload_type: payload_type.to_string(),
    })
}

/// Verify a flatbuffer envelope without nonce replay checking.
/// This is used when extracting tokens for re-signing, where the same envelope
/// may be processed multiple times legitimately.
/// Returns verified envelope parts.
pub fn verify_flatbuffer_envelope_skip_nonce_check(
    fb_envelope_bytes: &[u8],
) -> anyhow::Result<VerifiedEnvelopeParts> {
    let fb_env = crate::protocol::machine::root_as_envelope(fb_envelope_bytes)
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

    let canonical = crate::protocol::machine::build_envelope_canonical(
        &payload_vec,
        payload_type,
        nonce,
        ts,
        alg,
        None,
    );

    // Skip nonce replay check intentionally

    crate::crypto::verify_envelope(&pub_bytes, &canonical, &sig_bytes)
        .context("flatbuffer signature verification failed")?;

    Ok(VerifiedEnvelopeParts {
        payload: payload_vec,
        pubkey: pub_bytes,
        signature: sig_bytes,
        timestamp_ms: ts,
        payload_type: payload_type.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{ensure_keypair_ephemeral, ensure_pqc_init, sign_envelope};

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
        let fb_canonical = crate::protocol::machine::build_envelope_canonical(
            payload,
            payload_type,
            &nonce,
            timestamp,
            alg,
            None,
        );
        let (sig_b64, pub_b64) = sign_envelope(&privb, &pubb, &fb_canonical).expect("sign");

        // Build the final signed flatbuffer envelope (fb_envelope) using the original payload
        let fb_envelope = crate::protocol::machine::build_envelope_signed(
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
        let parts =
            verify_flatbuffer_envelope(&fb_envelope, Duration::from_secs(300)).expect("verify");

        assert_eq!(parts.payload, payload);

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
