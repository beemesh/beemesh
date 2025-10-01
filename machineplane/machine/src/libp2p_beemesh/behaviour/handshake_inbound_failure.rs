use libp2p::request_response;
use log::warn;

pub fn handshake_inbound_failure(peer: libp2p::PeerId, error: request_response::InboundFailure) {
    warn!("libp2p: handshake inbound failure from peer={}: {:?}", peer, error);
}

