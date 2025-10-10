use libp2p::request_response;

pub fn handshake_outbound_failure(peer: libp2p::PeerId, error: request_response::OutboundFailure) {
    super::failure_handlers::handle_handshake_outbound_failure(peer, error);
}
