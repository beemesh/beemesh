use crate::libp2p_beemesh::envelope::verify_flatbuffer_envelope_for_peer;
use anyhow::Error;
use libp2p::PeerId;
use thiserror::Error;

/// Outcome of verifying an incoming payload. Either it was fully verified,
/// or signature validation failed but we can fall back to the raw payload when
/// signatures are optional.
#[derive(Debug)]
pub enum PayloadVerification {
    Verified(VerifiedEnvelope),
    Unsigned { payload: Vec<u8>, reason: String },
}

impl PayloadVerification {
    /// Returns the verification failure reason if the payload was accepted
    /// without a signature.
    pub fn reason(&self) -> Option<&str> {
        match self {
            PayloadVerification::Unsigned { reason, .. } => Some(reason.as_str()),
            _ => None,
        }
    }

    /// Consume the verification result and return the payload bytes.
    pub fn into_payload(self) -> Vec<u8> {
        match self {
            PayloadVerification::Verified(envelope) => envelope.payload,
            PayloadVerification::Unsigned { payload, .. } => payload,
        }
    }

    /// Consume the verification result and return payload bytes alongside the
    /// signer public key when present.
    pub fn into_payload_and_pubkey(self) -> (Vec<u8>, Option<Vec<u8>>) {
        match self {
            PayloadVerification::Verified(envelope) => (envelope.payload, Some(envelope.pubkey)),
            PayloadVerification::Unsigned { payload, .. } => (payload, None),
        }
    }
}

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

/// Attempt to verify the supplied bytes as a signed envelope originating from `peer`.
/// When signatures are optional, verification failures degrade gracefully by
/// returning the raw payload along with an explanatory reason. If signatures
/// are required, the failure is promoted to an `EnvelopeRejection`.
pub fn verify_or_passthrough_for_peer(
    message_bytes: &[u8],
    peer: &PeerId,
) -> Result<PayloadVerification, EnvelopeRejection> {
    match verify_envelope_and_check_nonce_for_peer(message_bytes, &peer.to_string()) {
        Ok((payload, pubkey, signature)) => Ok(PayloadVerification::Verified(VerifiedEnvelope {
            payload,
            pubkey,
            signature,
        })),
        Err(err) => {
            if require_signed_messages() {
                Err(EnvelopeRejection::SignatureRequired(err))
            } else {
                Ok(PayloadVerification::Unsigned {
                    payload: message_bytes.to_vec(),
                    reason: format!("{:?}", err),
                })
            }
        }
    }
}
