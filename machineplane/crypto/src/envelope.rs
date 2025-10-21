use crate::{ensure_keypair_on_disk, sign_envelope};
use anyhow::Context;
use protocol::machine::{build_envelope_canonical, build_envelope_signed};
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_SIG_ALG: &str = "ml-dsa-65";
const SIG_PREFIX: &str = "ml-dsa-65";

/// Result of signing a payload into a protocol envelope.
#[derive(Clone, Debug)]
pub struct SignedEnvelope {
    bytes: Vec<u8>,
    nonce: String,
    timestamp_ms: u64,
    signature_b64: String,
    public_key_b64: String,
}

impl SignedEnvelope {
    /// Borrow the serialized envelope bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Consume the struct and return the serialized envelope bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// Nonce associated with the envelope.
    pub fn nonce(&self) -> &str {
        &self.nonce
    }

    /// Millisecond timestamp embedded in the envelope.
    pub fn timestamp_ms(&self) -> u64 {
        self.timestamp_ms
    }

    /// Base64 signature included in the envelope.
    pub fn signature_b64(&self) -> &str {
        &self.signature_b64
    }

    /// Base64 public key included in the envelope.
    pub fn public_key_b64(&self) -> &str {
        &self.public_key_b64
    }
}

/// Sign payload bytes with the node signing key stored on disk, returning a serialized envelope.
pub fn sign_payload(
    payload: &[u8],
    payload_type: &str,
    kem_pub_b64: Option<&str>,
) -> anyhow::Result<SignedEnvelope> {
    let (pub_bytes, sk_bytes) = ensure_keypair_on_disk()?;
    sign_payload_with_keypair(payload, payload_type, &pub_bytes, &sk_bytes, kem_pub_b64)
}

/// Sign payload bytes with an explicit keypair, returning a serialized envelope and metadata.
pub fn sign_payload_with_keypair(
    payload: &[u8],
    payload_type: &str,
    public_key: &[u8],
    secret_key: &[u8],
    kem_pub_b64: Option<&str>,
) -> anyhow::Result<SignedEnvelope> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0u64);
    let nonce = format!("{}-{:016x}", payload_type, rand::random::<u64>());

    let canonical_bytes = build_envelope_canonical(
        payload,
        payload_type,
        &nonce,
        timestamp,
        DEFAULT_SIG_ALG,
        kem_pub_b64,
    );

    let (sig_b64, pub_b64) = sign_envelope(secret_key, public_key, &canonical_bytes)
        .with_context(|| format!("failed to sign envelope for payload_type={payload_type}"))?;

    let bytes = build_envelope_signed(
        payload,
        payload_type,
        &nonce,
        timestamp,
        DEFAULT_SIG_ALG,
        SIG_PREFIX,
        &sig_b64,
        &pub_b64,
        kem_pub_b64,
    );

    Ok(SignedEnvelope {
        bytes,
        nonce,
        timestamp_ms: timestamp,
        signature_b64: sig_b64,
        public_key_b64: pub_b64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ensure_keypair_ephemeral;

    #[test]
    fn sign_payload_with_explicit_keys() {
        let (pub_bytes, sk_bytes) = ensure_keypair_ephemeral().expect("keygen");
        let payload = b"hello";
        let signed = sign_payload_with_keypair(payload, "test", &pub_bytes, &sk_bytes, None)
            .expect("sign with explicit keypair");
        assert!(!signed.bytes().is_empty());
        assert!(signed.nonce().starts_with("test-"));
        assert!(!signed.signature_b64().is_empty());
        assert!(!signed.public_key_b64().is_empty());
    }

    #[test]
    fn sign_payload_with_default_keys() {
        std::env::set_var("BEEMESH_SIGNING_EPHEMERAL", "1");
        let payload = b"world";
        let signed = sign_payload(payload, "default", None).expect("sign with default keys");
        assert!(!signed.bytes().is_empty());
        assert!(signed.nonce().starts_with("default-"));
    }
}
