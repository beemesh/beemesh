use libp2p::request_response;

pub fn handshake_inbound_failure(peer: libp2p::PeerId, error: request_response::InboundFailure) {
    println!("libp2p: handshake inbound failure from peer={}: {:?}", peer, error);
}

