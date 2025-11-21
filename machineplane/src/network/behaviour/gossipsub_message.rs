use crate::capacity::{CapacityCheckResult, ResourceRequest};
use crate::messages::constants::{SCHEDULER_EVENTS, SCHEDULER_PROPOSALS, SCHEDULER_TENDERS};
use crate::network::{capacity, utils};
use crate::run::get_global_capacity_verifier;
use libp2p::gossipsub;
use log::{debug, error, info, warn};
use std::sync::OnceLock;
use tokio::sync::mpsc;

// Global channel for scheduler messages
pub static SCHEDULER_INPUT_TX: OnceLock<
    mpsc::UnboundedSender<(libp2p::gossipsub::TopicHash, libp2p::gossipsub::Message)>,
> = OnceLock::new();

pub fn set_scheduler_input(
    tx: mpsc::UnboundedSender<(libp2p::gossipsub::TopicHash, libp2p::gossipsub::Message)>,
) {
    let _ = SCHEDULER_INPUT_TX.set(tx);
}

pub fn gossipsub_message(
    peer_id: libp2p::PeerId,
    message: gossipsub::Message,
    topic: gossipsub::TopicHash,
    swarm: &mut libp2p::Swarm<super::MyBehaviour>,
    pending_queries: &mut std::collections::HashMap<
        String,
        Vec<tokio::sync::mpsc::UnboundedSender<String>>,
    >,
) {
    debug!("received message from {}", peer_id);
    let payload = &message.data;

    // Check if this is a scheduler topic
    static SCHEDULER_TOPICS: OnceLock<[gossipsub::TopicHash; 3]> = OnceLock::new();

    let scheduler_topics = SCHEDULER_TOPICS.get_or_init(|| {
        [
            gossipsub::IdentTopic::new(SCHEDULER_TENDERS).hash(),
            gossipsub::IdentTopic::new(SCHEDULER_PROPOSALS).hash(),
            gossipsub::IdentTopic::new(SCHEDULER_EVENTS).hash(),
        ]
    });

    if scheduler_topics.contains(&topic) {
        if let Some(tx) = SCHEDULER_INPUT_TX.get() {
            if let Err(e) = tx.send((topic, message)) {
                error!("Failed to forward scheduler message: {}", e);
            }
        } else {
            warn!("Scheduler input channel not initialized, dropping message");
        }
        return;
    }

    // Then try CapacityReply
    if let Ok(cap_req) = crate::messages::machine::root_as_capacity_request(payload.as_slice()) {
        let orig_request_id = cap_req.request_id.clone();
        let responder_peer = swarm.local_peer_id().to_string();

        let manifest_id = match utils::extract_manifest_id_from_request_id(&orig_request_id) {
            Some(id) => id,
            None => {
                warn!(
                    "libp2p: capreq id={} missing manifest id, ignoring",
                    orig_request_id
                );
                return;
            }
        };

        let resource_request = ResourceRequest::new(
            Some(cap_req.cpu_milli),
            Some(cap_req.memory_bytes),
            Some(cap_req.storage_bytes),
            cap_req.replicas,
        );

        info!(
            "libp2p: received capreq id={} manifest_id={} from peer={} payload_bytes={}",
            orig_request_id,
            manifest_id,
            peer_id,
            message.data.len()
        );

        let verifier = get_global_capacity_verifier();

        // Perform synchronous capacity check using cached resources.
        let request_id_for_check = orig_request_id.clone();
        let verifier_for_check = verifier.clone();
        let resource_request_for_check = resource_request.clone();
        let check_handle = tokio::runtime::Handle::current();
        let check_result = std::thread::spawn(move || {
            check_handle.block_on(verifier_for_check.verify_capacity(&resource_request_for_check))
        })
        .join()
        .unwrap_or_else(|_| {
            warn!(
                "libp2p: capacity check thread panicked for request_id={}",
                request_id_for_check
            );
            CapacityCheckResult {
                has_capacity: false,
                rejection_reason: Some("internal error".to_string()),
                available_cpu_milli: 0,
                available_memory_bytes: 0,
                available_storage_bytes: 0,
            }
        });

        if !check_result.has_capacity {
            info!(
                "libp2p: capacity check failed for request_id={} manifest_id={} reason={:?}",
                orig_request_id, manifest_id, check_result.rejection_reason
            );
            return;
        }

        // Reserve resources for a short period to back the bid.
        let reserve_request_id = orig_request_id.clone();
        let reserve_manifest_id = manifest_id.clone();
        let verifier_for_reserve = verifier.clone();
        let resource_request_for_reserve = resource_request.clone();
        let reserve_handle = tokio::runtime::Handle::current();
        let reserve_outcome = std::thread::spawn(move || {
            reserve_handle.block_on(verifier_for_reserve.reserve_capacity(
                &reserve_request_id,
                Some(reserve_manifest_id.as_str()),
                &resource_request_for_reserve,
            ))
        })
        .join();

        match reserve_outcome {
            Ok(Ok(())) => {
                info!(
                    "libp2p: reserved resources for request_id={} manifest_id={}",
                    orig_request_id, manifest_id
                );
            }
            Ok(Err(err)) => {
                warn!(
                    "libp2p: failed to reserve resources for request_id={} manifest_id={}: {}",
                    orig_request_id, manifest_id, err
                );
                return;
            }
            Err(_) => {
                warn!(
                    "libp2p: reservation thread panicked for request_id={} manifest_id={}",
                    orig_request_id, manifest_id
                );
                return;
            }
        }

        let reply = capacity::compose_capacity_reply(
            "gossipsub",
            &orig_request_id,
            &responder_peer,
            |params| {
                params.ok = true;
                params.cpu_milli = check_result.available_cpu_milli;
                params.memory_bytes = check_result.available_memory_bytes;
                params.storage_bytes = check_result.available_storage_bytes;
            },
        );
        let payload_len = reply.payload.len();

        match capacity::publish_gossipsub_capacity_reply(
            &mut swarm.behaviour_mut().gossipsub,
            &topic,
            &reply,
        ) {
            Ok(_) => {
                info!(
                    "libp2p: published capreply for id={} manifest_id={} ({} bytes)",
                    orig_request_id, manifest_id, payload_len
                );
            }
            Err(e) => {
                error!(
                    "libp2p: failed to publish signed capacity reply id={} to {}: {:?}",
                    orig_request_id, peer_id, e
                );
            }
        }
        return;
    }

    if let Ok(cap_reply) = crate::messages::machine::root_as_capacity_reply(payload.as_slice()) {
        let request_part = cap_reply.request_id.clone();
        info!(
            "libp2p: received capreply for id={} from peer={}",
            request_part, peer_id
        );
        // KEM pubkey caching has been removed; keys are expected directly in capacity replies
        utils::notify_capacity_observers(pending_queries, &request_part, move || {
            peer_id.to_string()
        });
        return;
    }

    warn!(
        "gossipsub: Received unsupported message ({} bytes) from peer {}",
        payload.len(),
        peer_id
    );
}
