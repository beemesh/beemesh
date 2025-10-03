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
        let mut fbb = flatbuffers::FlatBufferBuilder::new();
        let req_id_off = fbb.create_string(&orig_request_id);
        let node_id_off = fbb.create_string(&peer_id.to_string());
        let region_off = fbb.create_string("local");
        // capabilities vector
        let caps_vec = {
            let mut tmp: Vec<flatbuffers::WIPOffset<&str>> = Vec::new();
            tmp.push(fbb.create_string("default"));
            fbb.create_vector(&tmp)
        };
        let reply_args = protocol::machine::CapacityReplyArgs {
            request_id: Some(req_id_off),
            ok: true,
            node_id: Some(node_id_off),
            region: Some(region_off),
            capabilities: Some(caps_vec),
            cpu_available_milli: 1000u32,
            memory_available_bytes: 1024u64 * 1024 * 512,
            storage_available_bytes: 1024u64 * 1024 * 1024,
        };
        let reply_off = protocol::machine::CapacityReply::create(&mut fbb, &reply_args);
        protocol::machine::finish_capacity_reply_buffer(&mut fbb, reply_off);
        let finished = fbb.finished_data().to_vec();
        // Wrap the reply into a signed envelope: { payload: base64(payload), sig: "ml-dsa-65:<b64>", pubkey: <b64> }
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
        if let Some(senders) = pending_queries.get_mut(&request_part) {
            for tx in senders.iter() {
                let _ = tx.send(peer_id.to_string());
            }
        }
        return;
    }

    log::warn!("Received non-savvy message ({} bytes) from peer {} — ignoring", message.data.len(), peer_id);
}
