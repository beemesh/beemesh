use libp2p::request_response;
use log::warn;

pub fn apply_outbound_failure(peer: libp2p::PeerId, error: request_response::OutboundFailure) {
    warn!("libp2p: outbound request failure to peer={}: {:?}", peer, error);
}

