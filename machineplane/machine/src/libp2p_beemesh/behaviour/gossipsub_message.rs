use super::message_verifier::verify_signed_message;
use crate::libp2p_beemesh::reply::{build_capacity_reply_with, warn_missing_kem};
use crate::libp2p_beemesh::utils;
use libp2p::gossipsub;
use log::{debug, error, info, warn};

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

    let verified = match verify_signed_message(&peer_id, &message.data, |err| {
        warn!("gossipsub: rejecting message from {}: {}", peer_id, err);
    }) {
        Some(envelope) => envelope,
        None => return,
    };
    let payload = verified.payload;

    // Then try CapacityReply
    if let Ok(cap_req) = protocol::machine::root_as_capacity_request(payload.as_slice()) {
        let orig_request_id = cap_req.request_id().unwrap_or("").to_string();
        let responder_peer = swarm.local_peer_id().to_string();
        info!(
            "libp2p: received capreq id={} from peer={} payload_bytes={}",
            orig_request_id,
            peer_id,
            payload.len()
        );
        let reply = build_capacity_reply_with(&orig_request_id, &responder_peer, |_| {});
        warn_missing_kem("gossipsub", &responder_peer, reply.kem_pub_b64.as_deref());
        let payload_len = reply.payload.len();

        match utils::sign_payload_default(&reply.payload, "capacity_reply", Some("capreply")) {
            Ok(signed_bytes) => {
                if let Err(e) = swarm
                    .behaviour_mut()
                    .gossipsub
                    .publish(topic.clone(), signed_bytes.as_slice())
                {
                    error!(
                        "libp2p: failed to publish signed capacity reply id={} to {}: {:?}",
                        orig_request_id, peer_id, e
                    );
                } else {
                    info!(
                        "libp2p: published capreply for id={} ({} bytes)",
                        orig_request_id, payload_len
                    );
                }
            }
            Err(e) => {
                error!(
                    "libp2p: failed to sign capacity reply for peer {} id={}: {:?}",
                    peer_id, orig_request_id, e
                );
            }
        }
        return;
    }

    if let Ok(cap_reply) = protocol::machine::root_as_capacity_reply(payload.as_slice()) {
        let request_part = cap_reply.request_id().unwrap_or("").to_string();
        info!(
            "libp2p: received capreply for id={} from peer={}",
            request_part, peer_id
        );
        // KEM pubkey caching has been removed - keys are now extracted directly from envelopes
        if let Some(senders) = pending_queries.get_mut(&request_part) {
            for tx in senders.iter() {
                let _ = tx.send(peer_id.to_string());
            }
        }
        return;
    }

    warn!(
        "gossipsub: Received unsupported message ({} bytes) from peer {}",
        payload.len(),
        peer_id
    );
}
