use libp2p::request_response;

pub fn apply_inbound_failure(peer: libp2p::PeerId, error: request_response::InboundFailure) {
    super::failure_handlers::handle_apply_inbound_failure(peer, error);
}
