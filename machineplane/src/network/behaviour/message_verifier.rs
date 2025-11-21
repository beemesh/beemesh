use crate::network::security::{
    EnvelopeRejection, VerifiedEnvelope, verify_signed_payload_for_peer,
};
use libp2p::PeerId;

/// Verify an incoming signed message from a peer and invoke `on_error` when verification fails.
/// Returns the verified envelope on success, otherwise `None`.
pub fn verify_signed_message<F>(
    peer: &PeerId,
    message_bytes: &[u8],
    on_error: F,
) -> Option<VerifiedEnvelope>
where
    F: FnOnce(&EnvelopeRejection),
{
    match verify_signed_payload_for_peer(message_bytes, peer) {
        Ok(envelope) => Some(envelope),
        Err(err) => {
            on_error(&err);
            None
        }
    }
}
