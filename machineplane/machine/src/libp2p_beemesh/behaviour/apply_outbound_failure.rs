use libp2p::request_response;

pub fn apply_outbound_failure(peer: libp2p::PeerId, error: request_response::OutboundFailure) {
    super::failure_handlers::handle_apply_outbound_failure(peer, error);
}
