use libp2p::{PeerId, Swarm};
use tokio::sync::mpsc;
use log::info;

use base64::engine::general_purpose;
use base64::Engine as _;
use serde_json::json;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::libp2p_beemesh::behaviour::MyBehaviour;
use crate::libp2p_beemesh::security;
use crypto;

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

        // Also create and store a local capability token for this manifest so local
        // fetchers (and other components on this node) can present a verifiable
        // capability when requesting key shares. This mirrors the storage logic
        // used for remote peers but does not send the token over the network.
        if let Ok(apply_req) = protocol::machine::root_as_apply_request(&manifest) {
            // compute manifest id as in remote branch
            let manifest_id = if let (Some(tenant), Some(operation_id), Some(manifest_json)) = (
                apply_req.tenant(), apply_req.operation_id(), apply_req.manifest_json()
            ) {
                let mut hasher = DefaultHasher::new();
                tenant.hash(&mut hasher);
                operation_id.hash(&mut hasher);
                manifest_json.hash(&mut hasher);
                format!("{:x}", hasher.finish())
            } else {
                let mut hasher = DefaultHasher::new();
                manifest.hash(&mut hasher);
                format!("{:x}", hasher.finish())
            };

            // Build and sign capability envelope, then store signed bytes in keystore
            let token_obj = serde_json::json!({
                "task_id": manifest_id,
                "issuer": swarm.local_peer_id().to_string(),
                "required_quorum": 1,
                "caveats": { "authorized_peer": peer_id.to_string() },
                "ts": SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0u64)
            });
            let token_bytes = serde_json::to_vec(&token_obj).unwrap_or_default();

            if let Ok((pubb, privb)) = crypto::ensure_keypair_on_disk() {
                // envelope
                let mut envelope = serde_json::Map::new();
                envelope.insert("payload".to_string(), serde_json::Value::String(general_purpose::STANDARD.encode(&token_bytes)));
                envelope.insert("manifest_id".to_string(), serde_json::Value::String(manifest_id.clone()));
                envelope.insert("type".to_string(), serde_json::Value::String("capability".to_string()));
                let envelope_value = serde_json::Value::Object(envelope);
                let envelope_bytes = serde_json::to_vec(&envelope_value).unwrap_or_default();
                if let Ok((sig_b64, pub_b64)) = crypto::sign_envelope(&privb, &pubb, &envelope_bytes) {
                    let mut signed_env = envelope_value.as_object().cloned().unwrap_or_default();
                    signed_env.insert("sig".to_string(), serde_json::Value::String(format!("ml-dsa-65:{}", sig_b64)));
                    signed_env.insert("pubkey".to_string(), serde_json::Value::String(pub_b64));
                    let signed_bytes = serde_json::to_vec(&serde_json::Value::Object(signed_env)).unwrap_or_default();

                    if let Ok((blob, cid)) = crypto::encrypt_share_for_keystore(&signed_bytes) {
                        if let Ok(ks) = crate::libp2p_beemesh::open_keystore() {
                            let meta = format!("capability:{}", manifest_id);
                            if let Err(e) = ks.put(&cid, &blob, Some(&meta)) {
                                log::warn!("keystore put failed for capability cid {}: {:?}", cid, e);
                            } else {
                                let manifest_provider_cid = format!("manifest:{}", manifest_id);
                                let (reply_tx2, _rx) = tokio::sync::mpsc::unbounded_channel();
                                let ctrl = crate::libp2p_beemesh::control::Libp2pControl::AnnounceProvider { cid: manifest_provider_cid.clone(), ttl_ms: 3000, reply_tx: reply_tx2 };
                                crate::libp2p_beemesh::control::enqueue_control(ctrl);
                            }
                        }
                    }
                }
            }
        }

        // Now that the local capability (if any) has been stored, perform the self-apply
        // which may request key shares and will be able to find the capability in the keystore.
        crate::libp2p_beemesh::behaviour::apply_message::process_self_apply_request(
            &manifest, swarm
        );

        let _ = reply_tx.send(Ok(format!("Apply request handled locally for {}", peer_id)));
        return;
    }

    // For remote peers, use the normal RequestResponse protocol
    // Before sending the apply, create a capability token tied to this manifest and
    // send it to the target peer so they can store it in their keystore.
    // Compute a manifest_id deterministically when possible (follow same heuristic as apply_message)
    let manifest_id = if let Ok(apply_req) = protocol::machine::root_as_apply_request(&manifest) {
        if let (Some(tenant), Some(operation_id), Some(manifest_json)) = (
            apply_req.tenant(), apply_req.operation_id(), apply_req.manifest_json()
        ) {
            let mut hasher = DefaultHasher::new();
            tenant.hash(&mut hasher);
            operation_id.hash(&mut hasher);
            manifest_json.hash(&mut hasher);
            format!("{:x}", hasher.finish())
        } else {
            // Fallback to hashing raw bytes
            let mut hasher = DefaultHasher::new();
            manifest.hash(&mut hasher);
            format!("{:x}", hasher.finish())
        }
    } else {
        let mut hasher = DefaultHasher::new();
        manifest.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    };

    // Build a simple JSON capability token (signed). We use a JSON token to avoid
    // flatbuffers/flatbuffers-version issues in the workspace. The token contains
    // basic fields and caveats. It is signed by this node's signing key.
    let token_obj = json!({
        "task_id": manifest_id,
        "issuer": swarm.local_peer_id().to_string(),
        "required_quorum": 1,
        "caveats": { "authorized_peer": peer_id.to_string() },
        "ts": SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0u64)
    });
    let token_bytes = serde_json::to_vec(&token_obj).unwrap_or_default();

    // Sign the envelope (we sign the JSON token so recipients can verify origin)
    // sign_envelope expects (sk_bytes, pk_bytes, envelope_bytes) but the helper
    // `crypto::sign_envelope` in this repo takes (sk_bytes, pk_bytes, envelope_bytes)
    // The caller who has persistent keys should use ensure_keypair_on_disk() to get pub/priv
    match crypto::ensure_keypair_on_disk() {
        Ok((pubb, privb)) => {
            // Build an envelope JSON (without sig/pubkey fields) to sign deterministically
            let mut envelope = serde_json::Map::new();
            envelope.insert("payload".to_string(), serde_json::Value::String(general_purpose::STANDARD.encode(&token_bytes)));
            envelope.insert("manifest_id".to_string(), serde_json::Value::String(manifest_id.clone()));
            envelope.insert("type".to_string(), serde_json::Value::String("capability".to_string()));
            let envelope_value = serde_json::Value::Object(envelope);
            let envelope_bytes = serde_json::to_vec(&envelope_value).unwrap_or_default();
                if let Ok((sig_b64, pub_b64)) = crypto::sign_envelope(&privb, &pubb, &envelope_bytes) {
                // Attach signature fields
                let mut signed_env = envelope_value.as_object().cloned().unwrap_or_default();
                signed_env.insert("sig".to_string(), serde_json::Value::String(format!("ml-dsa-65:{}", sig_b64)));
                signed_env.insert("pubkey".to_string(), serde_json::Value::String(pub_b64));
                let signed_bytes = serde_json::to_vec(&serde_json::Value::Object(signed_env)).unwrap_or_default();

                // Try to KEM-encapsulate the signed envelope per-recipient so only the
                // intended peer can read it. Prefer using the KEM public key published by
                // peers in their capacity replies (included in scheduler/gossipsub flows).
                // If the peer's kem_pubkey is available in the behaviour peer metadata map,
                // use it to encapsulate and place the resulting blob into the
                // KeyShareRequest.capability FlatBuffer field. If not available, fall back
                // to sending the signed JSON envelope directly as before.
                let mut sent_blob = false;

                // Best-effort: try to locate the recipient's KEM public key from the
                // global cache populated by capacity replies.
                if let Ok(map) = crate::libp2p_beemesh::PEER_KEM_PUBKEYS.read() {
                    if let Some(peer_kem_bytes) = map.get(&peer_id) {
                        match crypto::encrypt_payload_for_recipient(&peer_kem_bytes, &signed_bytes) {
                            Ok(enc_blob) => {
                                let enc_b64 = base64::engine::general_purpose::STANDARD.encode(&enc_blob);
                                let keyshare_fb = protocol::machine::build_keyshare_request(&manifest_id, &enc_b64);
                                let _ = swarm.behaviour_mut().keyshare_rr.send_request(&peer_id, keyshare_fb);
                                sent_blob = true;
                            }
                            Err(e) => {
                                log::warn!("failed to encrypt payload for recipient {}: {:?} - falling back to plain envelope", peer_id, e);
                            }
                        }
                    }
                }

                if !sent_blob {
                    // Send the capability envelope to the peer via the keyshare request channel so
                    // their keyshare handler will store it in the keystore under metadata capability:<manifest_id>
                    // We use the existing protocol builder to put the signed JSON bytes (base64) into the capability field.
                    let signed_b64 = base64::engine::general_purpose::STANDARD.encode(&signed_bytes);
                    let keyshare_fb = protocol::machine::build_keyshare_request(&manifest_id, &signed_b64);
                    let _ = swarm.behaviour_mut().keyshare_rr.send_request(&peer_id, keyshare_fb);
                }

                // Also store a local copy in this node's keystore for auditing/owner purposes
                // Store the signed envelope bytes so local fetchers can present a verifiable
                // capability when requesting key shares. Previously we stored the raw token
                // which lacked signature fields and failed verification at fetch time.
                if let Ok((blob, cid)) = crypto::encrypt_share_for_keystore(&signed_bytes) {
                    if let Ok(ks) = crate::libp2p_beemesh::open_keystore() {
                        let meta = format!("capability:{}", manifest_id);
                        if let Err(e) = ks.put(&cid, &blob, Some(&meta)) {
                            log::warn!("keystore put failed for capability cid {}: {:?}", cid, e);
                        } else {
                            // Announce provider for manifest capability discovery
                            let manifest_provider_cid = format!("manifest:{}", manifest_id);
                            let (reply_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
                            let ctrl = crate::libp2p_beemesh::control::Libp2pControl::AnnounceProvider { cid: manifest_provider_cid.clone(), ttl_ms: 3000, reply_tx };
                            crate::libp2p_beemesh::control::enqueue_control(ctrl);
                        }
                    }
                }
            }
        }
        Err(e) => {
            log::warn!("could not load signing keypair to create capability token: {:?}", e);
        }
    }

    // Finally send the apply request itself
    let request_id = swarm.behaviour_mut().apply_rr.send_request(&peer_id, manifest);
    info!("libp2p: sent apply request to peer={} request_id={:?}", peer_id, request_id);

    // For now, just send success immediately - proper response handling is done elsewhere
    let _ = reply_tx.send(Ok(format!("Apply request sent to {}", peer_id)));
}