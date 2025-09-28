use libp2p::{gossipsub, PeerId, Swarm};
use std::collections::HashMap as StdHashMap;
use tokio::sync::mpsc;

use crate::libp2p_beemesh::{behaviour::MyBehaviour};

mod query_capacity;
mod send_apply_request;

pub use query_capacity::handle_query_capacity_with_payload;
pub use send_apply_request::handle_send_apply_request;

/// Handle incoming control messages from other parts of the host (REST handlers)
pub async fn handle_control_message(
    msg: Libp2pControl,
    swarm: &mut Swarm<MyBehaviour>,
    topic: &gossipsub::IdentTopic,
    pending_queries: &mut StdHashMap<String, Vec<mpsc::UnboundedSender<String>>>,
) {
    match msg {
        Libp2pControl::QueryCapacityWithPayload { request_id, reply_tx, payload } => {
            handle_query_capacity_with_payload(request_id, reply_tx, payload, swarm, topic, pending_queries).await;
        }
        Libp2pControl::SendApplyRequest { peer_id, manifest, reply_tx } => {
            handle_send_apply_request(peer_id, manifest, reply_tx, swarm).await;
        }
        Libp2pControl::StoreAppliedManifest { manifest_data: _, reply_tx } => {
            // For now, just acknowledge - full implementation would require DHT manager integration
            let _ = reply_tx.send(Ok(()));
        }
        Libp2pControl::GetManifestFromDht { manifest_id: _, reply_tx } => {
            // For now, return None - full implementation would require DHT manager integration
            let _ = reply_tx.send(Ok(None));
        }
        Libp2pControl::BootstrapDht { reply_tx } => {
            // Bootstrap the Kademlia DHT
            let _ = swarm.behaviour_mut().kademlia.bootstrap();
            let _ = reply_tx.send(Ok(()));
        }
    }
}

/// Control messages sent from the rest API or other parts of the host to the libp2p task.
#[derive(Debug)]
pub enum Libp2pControl {
    QueryCapacityWithPayload {
        request_id: String,
        reply_tx: mpsc::UnboundedSender<String>,
        payload: Vec<u8>,
    },
    SendApplyRequest {
        peer_id: PeerId,
        manifest: serde_json::Value,
        reply_tx: mpsc::UnboundedSender<Result<String, String>>,
    },
    /// Store an applied manifest in the DHT after successful deployment
    StoreAppliedManifest {
        manifest_data: Vec<u8>,
        reply_tx: mpsc::UnboundedSender<Result<(), String>>,
    },
    /// Retrieve a manifest from the DHT by its ID
    GetManifestFromDht {
        manifest_id: String,
        reply_tx: mpsc::UnboundedSender<Result<Option<Vec<u8>>, String>>,
    },
    /// Bootstrap the DHT by connecting to known peers
    BootstrapDht {
        reply_tx: mpsc::UnboundedSender<Result<(), String>>,
    },
}
