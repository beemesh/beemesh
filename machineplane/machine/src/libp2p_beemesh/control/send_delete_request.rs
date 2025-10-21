use libp2p::{PeerId, Swarm};
use log::info;
use tokio::sync::mpsc;

use crate::libp2p_beemesh::behaviour::MyBehaviour;

/// Handle sending a delete request to a specific peer via libp2p request-response
pub async fn handle_send_delete_request(
    peer_id: PeerId,
    delete_request: Vec<u8>,
    reply_tx: mpsc::UnboundedSender<Result<String, String>>,
    swarm: &mut Swarm<MyBehaviour>,
) {
    info!(
        "handle_send_delete_request: sending delete request to peer {}",
        peer_id
    );

    // Send the delete request via request-response protocol
    let request_id = swarm
        .behaviour_mut()
        .delete_rr
        .send_request(&peer_id, delete_request);

    info!(
        "handle_send_delete_request: delete request sent to peer {} with request_id {:?}",
        peer_id, request_id
    );

    // For now, send immediate success response
    // In a complete implementation, we would track the request_id and wait for the actual response
    // from the peer, but that requires more complex request tracking infrastructure
    let _ = reply_tx.send(Ok("Delete request sent".to_string()));
}
