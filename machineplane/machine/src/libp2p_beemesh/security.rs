use anyhow::Error;
use libp2p::PeerId;
use thiserror::Error;

/// Verified envelope metadata captured during signature validation.
#[derive(Debug)]
pub struct VerifiedEnvelope {
    pub payload: Vec<u8>,
    pub pubkey: Vec<u8>,
    pub signature: Vec<u8>,
}

/// Signature enforcement failure when signed messages are required.
#[derive(Debug, Error)]
pub enum EnvelopeRejection {
    #[error("signed message required: {0}")]
    SignatureRequired(#[from] Error),
}
/// Determine whether the node must enforce signed messages.
pub fn require_signed_messages() -> bool {
    crypto::envelope_validator::EnvelopeValidator::require_signed_messages()
}

/// Verify a FlatBuffer envelope and check nonce for replay protection.
/// Returns (payload_bytes, pub_bytes, sig_bytes) on successful verification.
pub fn verify_envelope_and_check_nonce(
    envelope_bytes: &[u8],
) -> anyhow::Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    verify_envelope_and_check_nonce_for_peer(envelope_bytes, "global")
}

/// Verify a FlatBuffer envelope and check nonce for replay protection for a specific peer.
/// Returns (payload_bytes, pub_bytes, sig_bytes) on successful verification.
pub fn verify_envelope_and_check_nonce_for_peer(
    envelope_bytes: &[u8],
    peer_id: &str,
) -> anyhow::Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    crate::libp2p_beemesh::envelope::verify_flatbuffer_envelope_for_peer(
        envelope_bytes,
        std::time::Duration::from_secs(300),
        peer_id,
    )
}

/// Verify the supplied bytes as a signed envelope originating from `peer`.
/// Any verification failure is promoted to an `EnvelopeRejection`.
pub fn verify_signed_payload_for_peer(
    message_bytes: &[u8],
    peer: &PeerId,
) -> Result<VerifiedEnvelope, EnvelopeRejection> {
    verify_envelope_and_check_nonce_for_peer(message_bytes, &peer.to_string())
        .map(|(payload, pubkey, signature)| VerifiedEnvelope {
            payload,
            pubkey,
            signature,
        })
        .map_err(EnvelopeRejection::SignatureRequired)
}
