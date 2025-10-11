use std::time::Duration;

/// Keep nonces for a short window to mitigate replay attacks.
/// Simple in-memory map: nonce_str -> Instant inserted_at.
///
/// This is intentionally small and simple. In production you may want an
/// LRU cache, sharded maps, or a persistent store for high-throughput nodes.

/// Signature verification is required unconditionally to ensure messages
/// are always authenticated and cannot be silently accepted.
pub fn require_signed_messages() -> bool {
    true
}

/// Verify a FlatBuffer envelope and check nonce for replay protection.
/// Returns (payload_bytes, pub_bytes, sig_bytes) on successful verification.
pub fn verify_envelope_and_check_nonce(
    envelope_bytes: &[u8],
) -> anyhow::Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    // Use standard 5 minute nonce window for backward compatibility
    let nonce_window = Duration::from_secs(300);

    // All envelopes must be FlatBuffers now
    crate::libp2p_beemesh::envelope::verify_flatbuffer_envelope(envelope_bytes, nonce_window)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crypto::{ensure_keypair_ephemeral, ensure_pqc_init, sign_envelope};

    #[test]
    fn test_verify_and_replay() {
        ensure_pqc_init().expect("pqc init");
        let (pubb, privb) = ensure_keypair_ephemeral().expect("keygen");

        let payload = b"test payload";
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
        let canonical = protocol::machine::build_envelope_canonical(
            payload,
            payload_type,
            &nonce,
            timestamp,
            alg,
            None,
        );
        let (sig_b64, pub_b64) = sign_envelope(&privb, &pubb, &canonical).expect("sign");

        // Build signed envelope
        let envelope_bytes = protocol::machine::build_envelope_signed(
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
        let (decoded, _pub, _sig) =
            verify_envelope_and_check_nonce(&envelope_bytes).expect("verify");
        assert_eq!(decoded, payload);

        // Replay should be rejected
        assert!(verify_envelope_and_check_nonce(&envelope_bytes).is_err());
    }
}
