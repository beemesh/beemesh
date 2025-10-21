use crate::libp2p_beemesh::envelope::{sign_with_existing_keypair, SignEnvelopeConfig};
use crate::libp2p_beemesh::NODE_KEYPAIR;
use base64::Engine;
use libp2p::gossipsub;
use log::warn;
use protocol::machine::fb_envelope_extract_sig_pub;

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
    log::debug!("received message");
    // First try CapacityRequest
    if let Ok(cap_req) = protocol::machine::root_as_capacity_request(&message.data) {
        let orig_request_id = cap_req.request_id().unwrap_or("").to_string();
        log::info!(
            "libp2p: received capreq id={} from peer={} payload_bytes={}",
            orig_request_id,
            peer_id,
            message.data.len()
        );
        // Build a capacity reply and publish it (include request_id inside the reply)
        // Build capacity reply via helper and include our local KEM pubkey if available
        let kem_b64 = match crypto::ensure_kem_keypair_on_disk() {
            Ok((pubb, _)) => Some(base64::engine::general_purpose::STANDARD.encode(&pubb)),
            Err(_) => None,
        };
        let finished = protocol::machine::build_capacity_reply(
            true,
            1000u32,
            1024u64 * 1024 * 512,
            1024u64 * 1024 * 1024,
            &orig_request_id,
            &peer_id.to_string(),
            "local",
            kem_b64.as_deref(),
            &["default"],
        );

        // Wrap the reply into a signed FlatBuffer Envelope and publish
        let envelope_bytes =
            if let Some((pub_bytes, sk_bytes)) = NODE_KEYPAIR.get().and_then(|o| o.as_ref()) {
                let nonce = uuid::Uuid::new_v4().to_string();
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0u64);
                let sign_cfg = SignEnvelopeConfig {
                    nonce: Some(&nonce),
                    timestamp: Some(ts),
                    ..Default::default()
                };

                match sign_with_existing_keypair(
                    &finished,
                    "capacity_reply",
                    sign_cfg,
                    pub_bytes,
                    sk_bytes,
                ) {
                    Ok(signed) => Some(signed.bytes),
                    Err(e) => {
                        warn!("failed to sign finished message: {:?}", e);
                        None
                    }
                }
            } else {
                // If no node keypair present, publish plaintext flatbuffer reply
                Some(finished.clone())
            };
        if let Some(env) = envelope_bytes {
            let _ = swarm
                .behaviour_mut()
                .gossipsub
                .publish(topic.clone(), env.as_slice());
        }
        log::info!(
            "libp2p: published capreply for id={} ({} bytes)",
            orig_request_id,
            finished.len()
        );
        return;
    }

    // Prepare payload holder
    let mut owned_payload: Option<Vec<u8>> = None;

    // Only accept flatbuffer Envelope payloads now. Reject JSON envelopes.
    if let Ok(fb_env) = protocol::machine::root_as_envelope(&message.data) {
        if let Some((sig_bytes, pub_bytes)) = fb_envelope_extract_sig_pub(&message.data) {
            // Extract payload from envelope
            let inner_bytes: Vec<u8> = fb_env
                .payload()
                .map(|p| p.iter().collect())
                .unwrap_or_default();

            // Verify signature via crypto helper
            if let Err(e) = crypto::verify_envelope(&pub_bytes, &inner_bytes, &sig_bytes) {
                log::warn!(
                    "gossipsub: envelope verification failed from {}: {:?}",
                    peer_id,
                    e
                );
                return;
            }
            // Check nonce replay protection
            if let Some(nonce) = fb_env.nonce() {
                if let Err(e) = crate::libp2p_beemesh::envelope::check_and_insert_nonce(
                    nonce,
                    std::time::Duration::from_secs(300),
                ) {
                    log::warn!(
                        "gossipsub: envelope nonce rejected from {}: {:?}",
                        peer_id,
                        e
                    );
                    return;
                }
            }
            owned_payload = Some(inner_bytes);
        } else {
            log::warn!(
                "gossipsub: flatbuffer envelope signature extraction failed from {}",
                peer_id
            );
            return;
        }
    }

    // Determine effective data (inner payload if envelope present)
    let effective_data: &[u8] = match owned_payload.as_ref() {
        Some(b) => b.as_slice(),
        None => &message.data,
    };

    // Then try CapacityReply
    if let Ok(cap_reply) = protocol::machine::root_as_capacity_reply(effective_data) {
        let request_part = cap_reply.request_id().unwrap_or("").to_string();
        log::info!(
            "libp2p: received capreply for id={} from peer={}",
            request_part,
            peer_id
        );
        // KEM pubkey caching has been removed - keys are now extracted directly from envelopes
        if let Some(senders) = pending_queries.get_mut(&request_part) {
            for tx in senders.iter() {
                let _ = tx.send(peer_id.to_string());
            }
        }
        return;
    }

    log::warn!(
        "Received non-savvy message ({} bytes) from peer {} — ignoring",
        message.data.len(),
        peer_id
    );
}
