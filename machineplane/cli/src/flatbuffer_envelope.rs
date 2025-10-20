use anyhow::Result;
use base64::Engine;
use crypto::sign_envelope;
use protocol::machine::{
    build_envelope_canonical, build_envelope_canonical_with_peer, build_envelope_signed_with_peer,
    root_as_envelope,
};

/// Helper for building flatbuffer Envelopes instead of JSON
pub struct FlatbufferEnvelopeBuilder {
    peer_id: String,
    public_key: String,
}

impl FlatbufferEnvelopeBuilder {
    #[allow(dead_code)]
    pub fn with_keys(peer_id: String, public_key: String) -> Self {
        Self {
            peer_id,
            public_key,
        }
    }

    /// Build a signed manifest envelope containing encrypted manifest payload
    #[allow(dead_code)]
    pub fn build_manifest_envelope(
        &mut self,
        ciphertext: &[u8],
        _nonce_bytes: &[u8],
        _n: usize,
        _k: usize,
        _count: usize,
        sk_bytes: &[u8],
        pk_bytes: &[u8],
    ) -> Result<Vec<u8>> {
        // Simply use the ciphertext directly in an envelope
        // No need for EncryptedManifest flatbuffer structure
        let payload_bytes = ciphertext;

        let envelope_nonce: [u8; 16] = rand::random();
        let nonce_str = base64::engine::general_purpose::STANDARD.encode(&envelope_nonce);
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        // Create canonical bytes for signing
        let canonical_bytes = build_envelope_canonical(
            &payload_bytes,
            "manifest",
            &nonce_str,
            ts,
            "ml-dsa-65",
            None,
        );

        // Sign the canonical bytes
        let (sig_b64, _pub_b64) = sign_envelope(sk_bytes, pk_bytes, &canonical_bytes)?;

        // Create signed envelope directly
        Ok(build_envelope_signed_with_peer(
            &payload_bytes,
            "manifest",
            &nonce_str,
            ts,
            "ml-dsa-65",
            "ml-dsa-65",
            &sig_b64,
            &self.public_key,
            &self.peer_id,
            None,
        ))
    }

    /// Build a simple envelope with just payload and type
    #[allow(dead_code)]
    pub fn build_simple_envelope(
        &mut self,
        payload_type: &str,
        payload_content: &str,
    ) -> Result<Vec<u8>> {
        let payload_bytes = payload_content.as_bytes();

        let envelope_nonce: [u8; 16] = rand::random();
        let nonce_str = base64::engine::general_purpose::STANDARD.encode(&envelope_nonce);
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        Ok(build_envelope_canonical_with_peer(
            payload_bytes,
            payload_type,
            &nonce_str,
            ts,
            "ml-dsa-65",
            &self.peer_id,
            None,
        ))
    }

    /// Build a shares envelope containing encrypted share data
    #[allow(dead_code)]

    /// Add signature and pubkey to an existing envelope
    #[allow(dead_code)]
    pub fn sign_envelope(
        &self,
        envelope_bytes: &[u8],
        sk_bytes: &[u8],
        pk_bytes: &[u8],
    ) -> Result<Vec<u8>> {
        // Sign flatbuffer envelope
        // Fall back to flatbuffer handling - reconstruct canonical bytes for signing (same as verification)
        let envelope = root_as_envelope(envelope_bytes)
            .map_err(|e| anyhow::anyhow!("Failed to parse envelope: {}", e))?;

        // Build signed envelope using protocol helper
        let payload_bytes = envelope.payload().map(|v| v.bytes()).unwrap_or(&[]);

        // Reconstruct canonical bytes using the same method as verification
        let canonical_bytes = protocol::machine::build_envelope_canonical(
            payload_bytes,
            envelope.payload_type().unwrap_or(""),
            envelope.nonce().unwrap_or(""),
            envelope.ts(),
            envelope.alg().unwrap_or(""),
            None,
        );

        let (sig_b64, _pub_b64) = sign_envelope(sk_bytes, pk_bytes, &canonical_bytes)?;

        Ok(build_envelope_signed_with_peer(
            payload_bytes,
            envelope.payload_type().unwrap_or(""),
            envelope.nonce().unwrap_or(""),
            envelope.ts(),
            "ml-dsa-65",
            "ml-dsa-65",
            &sig_b64,
            &self.public_key, // Use the stored public key instead of pub_b64
            &self.peer_id,
            None,
        ))
    }
}
