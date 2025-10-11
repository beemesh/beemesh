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
    pub fn with_keys(peer_id: String, public_key: String) -> Self {
        Self {
            peer_id,
            public_key,
        }
    }

    /// Build a manifest envelope containing encrypted manifest payload
    pub fn build_manifest_envelope(
        &mut self,
        ciphertext: &[u8],
        nonce_bytes: &[u8],
        n: usize,
        k: usize,
        _count: usize,
    ) -> Result<Vec<u8>> {
        // Create an EncryptedManifest flatbuffer instead of JSON
        let nonce_b64 = base64::engine::general_purpose::STANDARD.encode(nonce_bytes);
        let payload_b64 = base64::engine::general_purpose::STANDARD.encode(ciphertext);
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let encrypted_manifest_fb = protocol::machine::build_encrypted_manifest(
            &nonce_b64,
            &payload_b64,
            "aes-256-gcm",      // encryption algorithm
            k as u32,           // threshold (k)
            n as u32,           // total_shares (n)
            Some("kubernetes"), // manifest_type
            &[],                // labels (empty)
            ts,                 // encrypted_at
            None,               // content_hash
        );
        eprintln!(
            "CLI: Created EncryptedManifest flatbuffer len={}, first_20_bytes={:02x?}",
            encrypted_manifest_fb.len(),
            &encrypted_manifest_fb[..std::cmp::min(20, encrypted_manifest_fb.len())]
        );

        // Test if the created flatbuffer is valid
        match protocol::machine::root_as_encrypted_manifest(&encrypted_manifest_fb) {
            Ok(test_manifest) => {
                eprintln!(
                    "CLI: EncryptedManifest validation PASSED - nonce={}, payload_len={}",
                    test_manifest.nonce().unwrap_or(""),
                    test_manifest.payload().unwrap_or("").len()
                );
            }
            Err(e) => {
                eprintln!("CLI: EncryptedManifest validation FAILED: {}", e);
            }
        }

        let payload_bytes = encrypted_manifest_fb;

        let envelope_nonce: [u8; 16] = rand::random();
        let nonce_str = base64::engine::general_purpose::STANDARD.encode(&envelope_nonce);
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        Ok(build_envelope_canonical(
            &payload_bytes,
            "manifest",
            &nonce_str,
            ts,
            "ml-dsa-65",
            None,
        ))
    }

    /// Build a simple envelope with just payload and type
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
    pub fn build_shares_envelope(
        &mut self,
        shares_b64: &[String],
        n: usize,
        k: usize,
        count: usize,
        manifest_id: &str,
    ) -> Result<Vec<u8>> {
        // Use flatbuffer instead of JSON for shares data
        let shares_fb = protocol::machine::build_key_shares(
            shares_b64,
            n as u32,
            k as u32,
            count as u32,
            manifest_id,
        );

        let envelope_nonce: [u8; 16] = rand::random();
        let nonce_str = base64::engine::general_purpose::STANDARD.encode(&envelope_nonce);
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        Ok(build_envelope_canonical_with_peer(
            &shares_fb,
            "shares",
            &nonce_str,
            ts,
            "ml-dsa-65",
            &self.peer_id,
            None,
        ))
    }

    /// Build a capability token envelope - creates flatbuffer format for machine REST API
    pub fn build_capability_envelope(
        &mut self,
        manifest_id: &str,
        peer_id: &str,
        ts: u64,
    ) -> Result<Vec<u8>> {
        // Create capability token using flatbuffer (not JSON)
        let expires_at = ts + 300_000; // 5 minutes from now
        let token_bytes = protocol::machine::build_capability_token(
            manifest_id,
            "cli",   // issuer
            peer_id, // authorized_peer
            ts,      // issued_at
            expires_at,
        );

        let envelope_nonce: [u8; 16] = rand::random();
        let nonce_str = base64::engine::general_purpose::STANDARD.encode(&envelope_nonce);

        // Create flatbuffer envelope (not JSON) for machine REST API compatibility
        Ok(build_envelope_canonical_with_peer(
            &token_bytes,
            "capability",
            &nonce_str,
            ts,
            "ml-dsa-65",
            &self.peer_id,
            None,
        ))
    }

    /// Add signature and pubkey to an existing envelope
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
