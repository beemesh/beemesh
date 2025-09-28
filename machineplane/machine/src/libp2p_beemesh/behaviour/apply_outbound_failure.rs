use libp2p::request_response;

pub fn apply_outbound_failure(peer: libp2p::PeerId, error: request_response::OutboundFailure) {
    println!("libp2p: outbound request failure to peer={}: {:?}", peer, error);
}

