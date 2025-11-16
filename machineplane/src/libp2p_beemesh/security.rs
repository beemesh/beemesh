use anyhow::Error;
use libp2p::PeerId;
use thiserror::Error;

/// Verified envelope metadata captured during signature validation.
#[derive(Debug)]
pub struct VerifiedEnvelope {
    pub payload: Vec<u8>,
    pub pubkey: Vec<u8>,
    pub signature: Vec<u8>,
    pub timestamp_ms: u64,
    pub payload_type: String,
}

/// Signature enforcement failure when signed messages are required.
#[derive(Debug, Error)]
pub enum EnvelopeRejection {
    #[error("signed message required: {0}")]
    SignatureRequired(#[from] Error),
}
/// Determine whether the node must enforce signed messages.
pub fn require_signed_messages() -> bool {
    crate::crypto::envelope_validator::EnvelopeValidator::require_signed_messages()
}

/// Verify a FlatBuffer envelope and check nonce for replay protection.
/// Returns the parsed envelope parts on successful verification.
pub fn verify_envelope_and_check_nonce(
    envelope_bytes: &[u8],
) -> anyhow::Result<crate::libp2p_beemesh::envelope::VerifiedEnvelopeParts> {
    verify_envelope_and_check_nonce_for_peer(envelope_bytes, "global")
}

/// Verify a FlatBuffer envelope and check nonce for replay protection for a specific peer.
/// Returns the parsed envelope parts on successful verification.
pub fn verify_envelope_and_check_nonce_for_peer(
    envelope_bytes: &[u8],
    peer_id: &str,
) -> anyhow::Result<crate::libp2p_beemesh::envelope::VerifiedEnvelopeParts> {
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
        .map(|parts| VerifiedEnvelope {
            payload: parts.payload,
            pubkey: parts.pubkey,
            signature: parts.signature,
            timestamp_ms: parts.timestamp_ms,
            payload_type: parts.payload_type,
        })
        .map_err(EnvelopeRejection::SignatureRequired)
}
