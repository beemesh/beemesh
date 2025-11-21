use libp2p::request_response;
use log::warn;

/// Direction of the failure (inbound or outbound)
#[derive(Debug, Clone, Copy)]
pub enum FailureDirection {
    Inbound,
    Outbound,
}

impl std::fmt::Display for FailureDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FailureDirection::Inbound => write!(f, "inbound"),
            FailureDirection::Outbound => write!(f, "outbound"),
        }
    }
}

/// Generic failure handler for any protocol and direction
pub fn handle_failure<E: std::fmt::Debug>(
    protocol: &str,
    direction: FailureDirection,
    peer: libp2p::PeerId,
    error: E,
) {
    warn!(
        "{} {} failure with peer {}: {:?}",
        protocol, direction, peer, error
    );
}

/// Specialized handler for apply protocol inbound failures
pub fn handle_apply_inbound_failure(peer: libp2p::PeerId, error: request_response::InboundFailure) {
    handle_failure("apply", FailureDirection::Inbound, peer, error);
}

/// Specialized handler for apply protocol outbound failures
pub fn handle_apply_outbound_failure(
    peer: libp2p::PeerId,
    error: request_response::OutboundFailure,
) {
    handle_failure("apply", FailureDirection::Outbound, peer, error);
}

/// Specialized handler for handshake protocol inbound failures
pub fn handle_handshake_inbound_failure(
    peer: libp2p::PeerId,
    error: request_response::InboundFailure,
) {
    handle_failure("handshake", FailureDirection::Inbound, peer, error);
}

/// Specialized handler for handshake protocol outbound failures
pub fn handle_handshake_outbound_failure(
    peer: libp2p::PeerId,
    error: request_response::OutboundFailure,
) {
    handle_failure("handshake", FailureDirection::Outbound, peer, error);
}
