use log::{debug, info, warn};
use libp2p::gossipsub;
use base64::Engine;
use crate::libp2p_beemesh::NODE_KEYPAIR;
use crate::libp2p_beemesh::security::verify_envelope_and_check_nonce;

pub fn gossipsub_message(
    peer_id: libp2p::PeerId,
    message: gossipsub::Message,
    topic: gossipsub::TopicHash,
    swarm: &mut libp2p::Swarm<super::MyBehaviour>,
    pending_queries: &mut std::collections::HashMap<String, Vec<tokio::sync::mpsc::UnboundedSender<String>>>,
) {
    log::debug!("received message");
    // First try CapacityRequest
    if let Ok(cap_req) = protocol::machine::root_as_capacity_request(&message.data) {
        let orig_request_id = cap_req.request_id().unwrap_or("").to_string();
        log::info!("libp2p: received capreq id={} from peer={} payload_bytes={}", orig_request_id, peer_id, message.data.len());
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

        // Wrap the reply into a signed envelope and publish
        let envelope_bytes = if let Some((pk_bytes, sk_bytes)) = NODE_KEYPAIR.get().and_then(|o| o.as_ref()) {
            match crypto::sign_envelope(sk_bytes, pk_bytes, &finished) {
                Ok((sig_b64, pub_b64)) => {
                    let env = serde_json::json!({
                        "payload": base64::engine::general_purpose::STANDARD.encode(&finished),
                        "sig": format!("ml-dsa-65:{}", sig_b64),
                        "pubkey": pub_b64,
                    });
                    env.to_string().into_bytes()
                }
                Err(_) => finished.clone(),
            }
        } else {
            finished.clone()
        };
        let _ = swarm.behaviour_mut().gossipsub.publish(topic.clone(), envelope_bytes.as_slice());
        log::info!("libp2p: published capreply for id={} ({} bytes)", orig_request_id, finished.len());
        return;
    }

    // First, try to parse/verify an envelope (JSON or FlatBuffer). Keep ownership
    // of the decoded payload bytes alive for downstream parsing.
    let mut owned_payload: Option<Vec<u8>> = None;
    let mut _owner_pubkey: Option<String> = None;
    let mut _owner_sig: Option<String> = None;

    // Try JSON envelope path first
    if let Ok(text) = std::str::from_utf8(&message.data) {
        if let Ok(env_val) = serde_json::from_str::<serde_json::Value>(text) {
            match verify_envelope_and_check_nonce(&env_val) {
                Ok((inner_bytes, pubkey_s, sig_s)) => {
                    owned_payload = Some(inner_bytes);
                    _owner_pubkey = Some(pubkey_s);
                    _owner_sig = Some(sig_s);
                }
                Err(e) => {
                    log::warn!("gossipsub: JSON envelope verification failed from {}: {:?}", peer_id, e);
                    return;
                }
            }
        }
    }

    // If not JSON-enveloped, the message might itself be a signed FlatBuffer Envelope
    if owned_payload.is_none() {
        // Try to parse message.data as a flatbuffer Envelope (direct bytes)
    if let Ok(_fb_env) = protocol::machine::root_as_envelope(&message.data) {
            // Reconstruct canonical bytes and verify via security helper which also checks replay
            // For convenience, we encode the flatbuffer bytes as base64 String and call the helper
            let b64 = base64::engine::general_purpose::STANDARD.encode(&message.data);
            let val = serde_json::Value::String(b64);
            match verify_envelope_and_check_nonce(&val) {
                Ok((inner_bytes, pubkey_s, sig_s)) => {
                    owned_payload = Some(inner_bytes);
                    _owner_pubkey = Some(pubkey_s);
                    _owner_sig = Some(sig_s);
                }
                Err(e) => {
                    log::warn!("gossipsub: flatbuffer envelope verification failed from {}: {:?}", peer_id, e);
                    return;
                }
            }
        }
    }

    let effective_data: &[u8] = match owned_payload.as_ref() {
        Some(b) => b.as_slice(),
        None => &message.data,
    };

    // Then try CapacityReply
    if let Ok(cap_reply) = protocol::machine::root_as_capacity_reply(effective_data) {
        let request_part = cap_reply.request_id().unwrap_or("").to_string();
    log::info!("libp2p: received capreply for id={} from peer={}", request_part, peer_id);
        // If the reply contains a kem_pubkey, decode and insert into behaviour cache
        if let Some(kem_b64) = cap_reply.kem_pubkey() {
            match base64::engine::general_purpose::STANDARD.decode(kem_b64) {
                Ok(kem_bytes) => {
                    let mut map = crate::libp2p_beemesh::PEER_KEM_PUBKEYS.write().unwrap();
                    map.insert(peer_id.clone(), kem_bytes);
                }
                Err(e) => {
                    log::warn!("failed to decode kem_pubkey from {}: {:?}", peer_id, e);
                }
            }
        }
        if let Some(senders) = pending_queries.get_mut(&request_part) {
            for tx in senders.iter() {
                let _ = tx.send(peer_id.to_string());
            }
        }
        return;
    }

    log::warn!("Received non-savvy message ({} bytes) from peer {} — ignoring", message.data.len(), peer_id);
}
