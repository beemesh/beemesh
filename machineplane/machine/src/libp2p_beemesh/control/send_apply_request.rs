use libp2p::{PeerId, Swarm};
use tokio::sync::mpsc;
use log::info;

use crate::libp2p_beemesh::behaviour::MyBehaviour;

/// Handle SendApplyRequest control message
pub async fn handle_send_apply_request(
    peer_id: PeerId,
    manifest: serde_json::Value,
    reply_tx: mpsc::UnboundedSender<Result<String, String>>,
    swarm: &mut Swarm<MyBehaviour>,
) {
    info!("libp2p: control SendApplyRequest received for peer={}", peer_id);

    // Create apply request FlatBuffer
    let operation_id = uuid::Uuid::new_v4().to_string();
    let manifest_json = manifest.to_string();
    let local_peer = swarm.local_peer_id().to_string();

    let apply_request = protocol::machine::build_apply_request(
        1, // replicas
        "default", // tenant
        &operation_id,
        &manifest_json,
        &local_peer,
    );

    // Send the request via request-response
    let request_id = swarm.behaviour_mut().apply_rr.send_request(&peer_id, apply_request);
    info!("libp2p: sent apply request to peer={} request_id={:?}", peer_id, request_id);

    // For now, just send success immediately - we'll handle proper response tracking later
    let _ = reply_tx.send(Ok(format!("Apply request sent to {}", peer_id)));
}