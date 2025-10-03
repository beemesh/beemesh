use libp2p::{PeerId, Swarm};
use tokio::sync::mpsc;
use log::info;

use crate::libp2p_beemesh::behaviour::MyBehaviour;

/// Handle SendApplyRequest control message
pub async fn handle_send_apply_request(
    peer_id: PeerId,
    manifest: Vec<u8>,
    reply_tx: mpsc::UnboundedSender<Result<String, String>>,
    swarm: &mut Swarm<MyBehaviour>,
) {
    info!("libp2p: control SendApplyRequest received for peer={}", peer_id);

    // Create apply request FlatBuffer
    // If caller already provided a flatbuffer, forward it directly via request-response
    let request_id = swarm.behaviour_mut().apply_rr.send_request(&peer_id, manifest);
    info!("libp2p: sent apply request to peer={} request_id={:?}", peer_id, request_id);

    // For now, just send success immediately - proper response handling is done elsewhere
    let _ = reply_tx.send(Ok(format!("Apply request sent to {}", peer_id)));
}