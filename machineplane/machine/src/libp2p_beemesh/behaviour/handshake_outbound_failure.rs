use libp2p::request_response;

pub fn handshake_outbound_failure(peer: libp2p::PeerId, error: request_response::OutboundFailure) {
    println!("libp2p: handshake outbound failure to peer={}: {:?}", peer, error);
}

