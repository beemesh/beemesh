use crate::messages::constants::BEEMESH_FABRIC;
use crate::network::behaviour::{MyBehaviour, SCHEDULER_INPUT_TX};
use libp2p::{Swarm, gossipsub};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

type ManifestRequestSender = mpsc::UnboundedSender<Result<(), String>>;

/// Control messages sent from the rest API or other parts of the host to the libp2p task.
#[derive(Debug)]
pub enum Libp2pControl {
    PublishTender {
        payload: Vec<u8>,
        reply_tx: mpsc::UnboundedSender<Result<(), String>>,
    },
    /// Bootstrap the DHT by connecting to known peers
    BootstrapDht {
        reply_tx: mpsc::UnboundedSender<Result<(), String>>,
    },
    /// Get DHT peer information for debugging
    GetDhtPeers {
        reply_tx: mpsc::UnboundedSender<Result<serde_json::Value, String>>,
    },
    /// Get the local peer ID
    GetLocalPeerId {
        reply_tx: mpsc::UnboundedSender<libp2p::PeerId>,
    },
}

/// Handle incoming control messages from other parts of the host (REST handlers)
pub async fn handle_control_message(msg: Libp2pControl, swarm: &mut Swarm<MyBehaviour>) {
    match msg {
        Libp2pControl::PublishTender { payload, reply_tx } => {
            let topic = gossipsub::IdentTopic::new(BEEMESH_FABRIC);
            let local_peer_id = *swarm.local_peer_id();
            match swarm
                .behaviour_mut()
                .gossipsub
                .publish(topic.clone(), payload.clone())
            {
                Ok(_) => {
                    // Use per-node scheduler channel if available, otherwise fall back to global
                    let tx = crate::network::behaviour::get_scheduler_input_for_peer(&local_peer_id)
                        .or_else(|| SCHEDULER_INPUT_TX.get().cloned());
                    
                    if let Some(tx) = tx {
                        let _ = tx.send((
                            topic.hash(),
                            gossipsub::Message {
                                source: Some(local_peer_id),
                                data: payload,
                                sequence_number: None,
                                topic: topic.hash(),
                            },
                        ));
                    }

                    let _ = reply_tx.send(Ok(()));
                }
                Err(e) => {
                    let _ = reply_tx.send(Err(format!("Failed to publish tender: {}", e)));
                }
            }
        }

        Libp2pControl::BootstrapDht { reply_tx } => {
            let _ = swarm.behaviour_mut().kademlia.bootstrap();
            let _ = reply_tx.send(Ok(()));
        }
        Libp2pControl::GetDhtPeers { reply_tx } => {
            // Get DHT peer information for debugging
            let connected_peers: Vec<_> = swarm.connected_peers().cloned().collect();
            let local_peer_id = *swarm.local_peer_id();

            let dht_info = serde_json::json!({
                "local_peer_id": local_peer_id.to_string(),
                "connected_peers": connected_peers.iter().map(|p| p.to_string()).collect::<Vec<_>>(),
                "connected_count": connected_peers.len()
            });

            let _ = reply_tx.send(Ok(dht_info));
        }
        Libp2pControl::GetLocalPeerId { reply_tx } => {
            // Get the local peer ID
            let local_peer_id = *swarm.local_peer_id();
            let _ = reply_tx.send(local_peer_id);
        }
    }
}

/// Global store for tracking pending manifest distribution requests
static PENDING_MANIFEST_REQUESTS: Lazy<
    Mutex<HashMap<libp2p::request_response::OutboundRequestId, (ManifestRequestSender, Instant)>>,
> = Lazy::new(|| Mutex::new(HashMap::new()));

/// Get access to the pending manifest requests store
pub fn get_pending_manifest_requests() -> &'static Mutex<
    HashMap<libp2p::request_response::OutboundRequestId, (ManifestRequestSender, Instant)>,
> {
    &PENDING_MANIFEST_REQUESTS
}

/// Clean up timed-out manifest requests (older than 5 seconds)
pub fn cleanup_timed_out_manifest_requests() {
    let now = Instant::now();
    let timeout_duration = Duration::from_secs(5);

    let mut pending_requests = PENDING_MANIFEST_REQUESTS.lock().unwrap();
    let mut timed_out_requests = Vec::new();

    // Find timed-out requests
    for (request_id, (_, timestamp)) in pending_requests.iter() {
        if now.duration_since(*timestamp) > timeout_duration {
            timed_out_requests.push(*request_id);
        }
    }

    // Remove and notify timed-out requests
    for request_id in timed_out_requests {
        if let Some((reply_tx, _)) = pending_requests.remove(&request_id) {
            let _ = reply_tx.send(Err("Request timeout".to_string()));
        }
    }
}

// ============================================================================
// Control Message Handlers
// ============================================================================
