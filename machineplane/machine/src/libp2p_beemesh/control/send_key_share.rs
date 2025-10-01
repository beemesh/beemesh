use libp2p::{PeerId, Swarm};
use tokio::sync::mpsc;
use log::info;

use crate::libp2p_beemesh::behaviour::MyBehaviour;

pub async fn handle_send_key_share(
    peer_id: PeerId,
    share_payload: serde_json::Value,
    reply_tx: mpsc::UnboundedSender<Result<(), String>>,
    swarm: &mut Swarm<MyBehaviour>,
) {
    info!("libp2p: control SendKeyShare for peer={}", peer_id);

    // Deliver the share payload using the dedicated keyshare request-response behaviour.
    let request_bytes = serde_json::to_vec(&share_payload).unwrap_or_default();
    let request_id = swarm.behaviour_mut().keyshare_rr.send_request(&peer_id, request_bytes);
    info!("libp2p: sent keyshare to peer={} request_id={:?}", peer_id, request_id);

    let _ = reply_tx.send(Ok(()));
}
