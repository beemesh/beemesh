use libp2p::{request_response, PeerId};

/// Handle manifest fetch request/response messages
pub fn manifest_fetch_message(
    message: request_response::Message<Vec<u8>, Vec<u8>>,
    peer: PeerId,
    swarm: &mut libp2p::Swarm<super::MyBehaviour>,
) {
    // Delegate to the main manifest fetch handler
    crate::libp2p_beemesh::manifest_fetch::manifest_fetch_message(message, peer, swarm);
}
