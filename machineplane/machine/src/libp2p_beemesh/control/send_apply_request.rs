use libp2p::{PeerId, Swarm};
use log::{debug, info};
use tokio::sync::mpsc;

use base64::Engine;
use flatbuffers::FlatBufferBuilder;
use protocol::machine::CaveatArgs;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::libp2p_beemesh::behaviour::MyBehaviour;
use crypto;

/// Handle SendApplyRequest control message
pub async fn handle_send_apply_request(
    peer_id: PeerId,
    manifest: Vec<u8>,
    reply_tx: mpsc::UnboundedSender<Result<String, String>>,
    swarm: &mut Swarm<MyBehaviour>,
) {
    info!(
        "libp2p: control SendApplyRequest received for peer={}",
        peer_id
    );

    // Check if this is a self-send - handle locally instead of using RequestResponse
    if peer_id == *swarm.local_peer_id() {
        debug!("libp2p: handling self-apply locally for peer {}", peer_id);

        // Now that the local capability (if any) has been stored, perform the self-apply
        // which may request key shares and will be able to find the capability in the keystore.
        // Note: process_self_apply_request expects raw flatbuffer bytes, not signed envelopes
        crate::libp2p_beemesh::behaviour::apply_message::process_self_apply_request(
            &manifest,
            swarm,
            *swarm.local_peer_id(),
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
            apply_req.tenant(),
            apply_req.operation_id(),
            apply_req.manifest_json(),
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

    // Build a flatbuffer CapabilityToken for the remote peer and sign it.
    let mut signed_bytes_for_remote: Option<Vec<u8>> = None;
    if let Ok((pubb, privb)) = crypto::ensure_keypair_on_disk() {
        let mut fbb = FlatBufferBuilder::with_capacity(256);
        let task_off = fbb.create_string(&manifest_id);
        let issuer_peer_off = fbb.create_string(&swarm.local_peer_id().to_string());
        let peer_bytes_vec = fbb.create_vector(peer_id.to_string().as_bytes());
        let condition_type_str = fbb.create_string("authorized_peer");
        let caveat_off = protocol::machine::Caveat::create(
            &mut fbb,
            &CaveatArgs {
                condition_type: Some(condition_type_str),
                value: Some(peer_bytes_vec),
            },
        );
        let caves_vec = fbb.create_vector(&[caveat_off]);

        let mut cap_args = protocol::machine::CapabilityArgs::default();
        cap_args.manifest_id = Some(task_off);
        cap_args.required_quorum = 3;
        cap_args.issued_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0u64);
        cap_args.expires_at = cap_args.issued_at + 3600 * 1000;
        cap_args.issuer_peer_id = Some(issuer_peer_off);
        // type_ can be left None
        let cap_off = protocol::machine::Capability::create(&mut fbb, &cap_args);

        let mut token_args = protocol::machine::CapabilityTokenArgs::default();
        token_args.root_capability = Some(cap_off);
        if let Some(caves) = Some(caves_vec) {
            token_args.caveats = Some(caves);
        }
        let token_off = protocol::machine::CapabilityToken::create(&mut fbb, &token_args);
        fbb.finish(token_off, None);
        let token_bytes = fbb.finished_data().to_vec();

        if let Ok((sig_b64, pub_b64)) = crypto::sign_envelope(&privb, &pubb, &token_bytes) {
            let nonce = uuid::Uuid::new_v4().to_string();
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0u64);
            let envelope_fb = protocol::machine::build_envelope_signed(
                &token_bytes,
                "capability",
                &nonce,
                ts,
                "ml-dsa-65",
                "ml-dsa-65",
                &sig_b64,
                &pub_b64,
                None,
            );
            signed_bytes_for_remote = Some(envelope_fb);
        }
    }

    // If we have a signed flatbuffer envelope, attempt KEM-encapsulate it per-recipient; otherwise fall back to sending the capability token bytes base64
    if let Some(signed_envelope_fb) = signed_bytes_for_remote.as_ref() {
        // Try to KEM-encapsulate the signed envelope per-recipient
        let mut sent_blob = false;
        if let Ok(map) = crate::libp2p_beemesh::PEER_KEM_PUBKEYS.read() {
            if let Some(peer_kem_bytes) = map.get(&peer_id) {
                match crypto::encrypt_payload_for_recipient(&peer_kem_bytes, &signed_envelope_fb) {
                    Ok(enc_blob) => {
                        let _enc_b64 = base64::engine::general_purpose::STANDARD.encode(&enc_blob);
                        // Keyshare functionality has been deprecated/removed
                        log::warn!("keyshare functionality deprecated - skipping keyshare request");
                        sent_blob = true;
                    }
                    Err(e) => {
                        log::warn!("failed to encrypt payload for recipient {}: {:?} - falling back to plain envelope", peer_id, e);
                    }
                }
            }
        }
        // Also store a local copy encrypted for keystore
        if let Ok((blob, cid)) = crypto::encrypt_share_for_keystore(&signed_envelope_fb) {
            if let Ok(ks) = crate::libp2p_beemesh::open_keystore() {
                let meta = format!("capability:{}", manifest_id);
                if let Err(e) = ks.put(&cid, &blob, Some(&meta)) {
                    log::warn!("keystore put failed for capability cid {}: {:?}", cid, e);
                } else {
                    let manifest_provider_cid = format!("manifest:{}", manifest_id);
                    let (reply_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
                    let ctrl = crate::libp2p_beemesh::control::Libp2pControl::AnnounceProvider {
                        cid: manifest_provider_cid.clone(),
                        ttl_ms: 3000,
                        reply_tx,
                    };
                    crate::libp2p_beemesh::control::enqueue_control(ctrl);
                }
            }
        }
    }

    // Sign the apply request in an envelope before sending
    let signed_apply_request = if let Ok((pubb, privb)) = crypto::ensure_keypair_on_disk() {
        let nonce = uuid::Uuid::new_v4().to_string();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0u64);
        let payload_type = "apply_request";
        let alg = "ml-dsa-65";

        // Create canonical bytes and sign
        let canonical_bytes = protocol::machine::build_envelope_canonical(
            &manifest,
            payload_type,
            &nonce,
            timestamp,
            alg,
            None,
        );

        match crypto::sign_envelope(&privb, &pubb, &canonical_bytes) {
            Ok((sig_b64, pub_b64)) => {
                // Build signed envelope
                protocol::machine::build_envelope_signed(
                    &manifest,
                    payload_type,
                    &nonce,
                    timestamp,
                    alg,
                    "ml-dsa-65",
                    &sig_b64,
                    &pub_b64,
                    None,
                )
            }
            Err(e) => {
                log::warn!("failed to sign apply request for peer {}: {:?}", peer_id, e);
                manifest // fallback to unsigned
            }
        }
    } else {
        log::warn!(
            "failed to load keypair for signing apply request to peer {}",
            peer_id
        );
        manifest // fallback to unsigned
    };

    // Finally send the (now signed) apply request
    let request_id = swarm
        .behaviour_mut()
        .apply_rr
        .send_request(&peer_id, signed_apply_request);
    info!(
        "libp2p: sent apply request to peer={} request_id={:?}",
        peer_id, request_id
    );

    // For now, just send success immediately - proper response handling is done elsewhere
    let _ = reply_tx.send(Ok(format!("Apply request sent to {}", peer_id)));
}
