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

    // Check if this is a self-send - handle locally instead of using RequestResponse
    if peer_id == *swarm.local_peer_id() {
        info!("libp2p: handling self-apply locally for peer {}", peer_id);
        
        // Handle the apply request locally without going through RequestResponse
        // This processes the manifest directly and triggers decryption
        crate::libp2p_beemesh::behaviour::apply_message::process_self_apply_request(
            &manifest, swarm
        );
        
        let _ = reply_tx.send(Ok(format!("Apply request handled locally for {}", peer_id)));
        return;
    }

    // For remote peers, use the normal RequestResponse protocol
    let request_id = swarm.behaviour_mut().apply_rr.send_request(&peer_id, manifest);
    info!("libp2p: sent apply request to peer={} request_id={:?}", peer_id, request_id);

    // For now, just send success immediately - proper response handling is done elsewhere
    let _ = reply_tx.send(Ok(format!("Apply request sent to {}", peer_id)));
}