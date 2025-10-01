use libp2p::request_response;
use log::warn;

pub fn apply_inbound_failure(peer: libp2p::PeerId, error: request_response::InboundFailure) {
    warn!("libp2p: inbound request failure from peer={}: {:?}", peer, error);
}

