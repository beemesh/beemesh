use base64::Engine;
use libp2p::request_response;
use log::{debug, error, info, warn};
/// Handle inbound key-share request-response messages.
/// Accept only flatbuffer-encoded KeyShareRequest / Envelope formats. JSON paths have been removed.
pub fn keyshare_message(
    message: request_response::Message<Vec<u8>, Vec<u8>>,
    peer: libp2p::PeerId,
    swarm: &mut libp2p::Swarm<super::MyBehaviour>,
    _local_peer: libp2p::PeerId,
) {
    match message {
        request_response::Message::Request {
            request, channel, ..
        } => {
            debug!(
                "libp2p: received keyshare request from peer={} request_size={}",
                peer,
                request.len()
            );

            // First try to parse as FlatBuffer KeyShareRequest (fetch request)
            if let Ok(kreq) = protocol::machine::root_as_key_share_request(&request) {
                // It's a fetch request: respond with the locally stored share if available
                let manifest_id = kreq.manifest_id().unwrap_or("");
                debug!(
                    "libp2p: keyshare fetch request for manifest_id={} from {}",
                    manifest_id, peer
                );

                // Enforce capability verification: require that the requester included a valid
                // capability token in the 'capability' field of the request (base64-encoded).
                // If missing or invalid, reject the fetch.
                if let Some(cap_b64) = kreq.capability() {
                    match base64::engine::general_purpose::STANDARD.decode(cap_b64) {
                        Ok(cap_bytes) => {
                            // Treat capability bytes as a flatbuffer envelope (not JSON).
                            match crate::libp2p_beemesh::envelope::verify_flatbuffer_envelope(
                                &cap_bytes,
                                std::time::Duration::from_secs(300),
                            ) {
                                Ok((token_bytes, _pub, _sig)) => {
                                    // Verify holder signature in the capability token
                                    match verify_capability_holder_signature(
                                        &token_bytes,
                                        manifest_id,
                                        &peer,
                                    ) {
                                        Ok(()) => {
                                            log::debug!(
                                                "libp2p: received valid capability blob with holder signature len={} from {}",
                                                cap_bytes.len(),
                                                peer
                                            );
                                        }
                                        Err(e) => {
                                            warn!("capability holder signature verification failed from {}: {:?}", peer, e);
                                            let resp = protocol::machine::build_keyshare_response(
                                                false,
                                                "fetch",
                                                "invalid_holder_signature",
                                            );
                                            let _ = swarm
                                                .behaviour_mut()
                                                .keyshare_rr
                                                .send_response(channel, resp);
                                            return;
                                        }
                                    }
                                    // Do not Store the raw capability blob in the keystore because it can only be used by the holder.
                                    // if let Ok((blob_enc, cid)) =
                                    //     crypto::encrypt_share_for_keystore(&cap_bytes)
                                    // {
                                    //     if let Ok(ks) = crate::libp2p_beemesh::open_keystore() {
                                    //         let meta = format!("capability:{}", manifest_id);
                                    //         if let Err(e) = ks.put(&cid, &blob_enc, Some(&meta)) {
                                    //             log::warn!("keystore put failed for capability cid {}: {:?}", cid, e);
                                    //         } else {
                                    //             log::debug!("libp2p: stored received capability for manifest_id={} cid={}", manifest_id, cid);
                                    //             // Announce provider for discovered capability holders
                                    //             let manifest_provider_cid =
                                    //                 format!("manifest:{}", manifest_id);
                                    //             let (reply_tx, _rx) =
                                    //                 tokio::sync::mpsc::unbounded_channel();
                                    //             let ctrl = crate::libp2p_beemesh::control::Libp2pControl::AnnounceProvider { cid: manifest_provider_cid.clone(), ttl_ms: 3000, reply_tx };
                                    //             crate::libp2p_beemesh::control::enqueue_control(
                                    //                 ctrl,
                                    //             );
                                    //         }
                                    //     }
                                    // }

                                    // Capability verified; continue to fetch
                                }
                                Err(e) => {
                                    error!("capability verification failed from {}: {:?}", peer, e);
                                    let resp = protocol::machine::build_keyshare_response(
                                        false,
                                        "fetch",
                                        "invalid_capability",
                                    );
                                    let _ = swarm
                                        .behaviour_mut()
                                        .keyshare_rr
                                        .send_response(channel, resp);
                                    return;
                                }
                            }
                        }
                        Err(e) => {
                            error!("failed to base64-decode capability from {}: {:?}", peer, e);
                            let resp = protocol::machine::build_keyshare_response(
                                false,
                                "fetch",
                                "invalid_capability",
                            );
                            let _ = swarm
                                .behaviour_mut()
                                .keyshare_rr
                                .send_response(channel, resp);
                            return;
                        }
                    }
                } else {
                    debug!(
                        "keyshare fetch missing capability from {} - rejecting",
                        peer
                    );
                    let resp = protocol::machine::build_keyshare_response(
                        false,
                        "fetch",
                        "missing_capability",
                    );
                    let _ = swarm
                        .behaviour_mut()
                        .keyshare_rr
                        .send_response(channel, resp);
                    return;
                }

                // Search keystore for a share with this manifest_id
                match crate::libp2p_beemesh::open_keystore() {
                    Ok(ks) => {
                        debug!(
                            "libp2p: keyshare fetch searching keystore for manifest_id={}",
                            manifest_id
                        );
                        // Use the keystore's metadata field to find the CID for this manifest_id
                        match ks.find_cid_for_manifest(manifest_id) {
                            Ok(Some(cid)) => {
                                // Found the CID, now get the blob and decrypt it
                                if let Ok(Some(blob)) = ks.get(&cid) {
                                    match crypto::ensure_kem_keypair_on_disk() {
                                        Ok((_pubb, privb)) => {
                                            match crypto::decrypt_share_from_blob(&blob, &privb) {
                                                Ok(plain) => {
                                                    let b64 = base64::engine::general_purpose::STANDARD.encode(&plain);
                                                    debug!("libp2p: keyshare fetch found and returning share for manifest_id={} from cid={}", manifest_id, cid);
                                                    let resp = protocol::machine::build_keyshare_response(true, "fetch", &b64);
                                                    let _ = swarm.behaviour_mut().keyshare_rr.send_response(channel, resp);
                                                    return;
                                                }
                                                Err(e) => {
                                                    warn!("failed to decrypt stored share blob for cid {}: {:?}", cid, e);
                                                }
                                            }
                                        }
                                        Err(e) => warn!("could not open kem keypair to decrypt share blob: {:?}", e),
                                    }
                                }
                            }
                            Ok(None) => {
                                debug!(
                                    "libp2p: keyshare fetch no share found for manifest_id={}",
                                    manifest_id
                                );
                            }
                            Err(e) => {
                                debug!("libp2p: keyshare fetch error querying keystore: {:?}", e);
                            }
                        }
                    }
                    Err(e) => warn!("could not open keystore for fetch reply: {:?}", e),
                }

                // Not found
                debug!(
                    "libp2p: keyshare fetch no share found for manifest_id={}",
                    manifest_id
                );
                let resp = protocol::machine::build_keyshare_response(false, "fetch", "not_found");
                let _ = swarm
                    .behaviour_mut()
                    .keyshare_rr
                    .send_response(channel, resp);
                return;
            }

            // If the incoming request is a recipient-blob (versioned v0x02), try to decapsulate first
            let maybe_preprocessed = if !request.is_empty() && request[0] == 0x02u8 {
                // We received an encapsulated blob. Attempt to decapsulate using our on-disk KEM private key.
                match crypto::ensure_kem_keypair_on_disk() {
                    Ok((_pubb, privb)) => {
                        match crypto::decrypt_payload_from_recipient_blob(&request, &privb) {
                            Ok(inner_bytes) => Ok(inner_bytes),
                            Err(e) => Err(anyhow::anyhow!(
                                "failed to decapsulate recipient blob: {:?}",
                                e
                            )),
                        }
                    }
                    Err(e) => Err(anyhow::anyhow!("could not load kem keypair: {:?}", e)),
                }
            } else {
                // Not an encapsulated blob - use raw request bytes
                Ok(request.clone())
            };

            match maybe_preprocessed {
                Ok(effective_bytes) => {
                    // Only accept flatbuffer-style envelopes/requests now. Reject any JSON payloads.

                    // First try to parse as a simple flatbuffer envelope (for share distribution)
                    if let Ok(env) = protocol::machine::root_as_envelope(&effective_bytes) {
                        // Verify the envelope signature
                        match crate::libp2p_beemesh::envelope::verify_flatbuffer_envelope(
                            &effective_bytes,
                            std::time::Duration::from_secs(300),
                        ) {
                            Ok((payload_bytes, _pub, _sig)) => {
                                // For simple share distribution, just store the payload directly
                                match crypto::encrypt_share_for_keystore(&payload_bytes) {
                                    Ok((blob, cid)) => {
                                        match crate::libp2p_beemesh::open_keystore() {
                                            Ok(ks) => {
                                                warn!(
                                                    "attempting keystore.put for cid={} size={}",
                                                    cid,
                                                    blob.len()
                                                );
                                                // Use manifest_id from envelope type or a default meta
                                                let payload_type = env.payload_type().unwrap_or("");

                                                warn!("libp2p: keyshare processing envelope payload_type={}", payload_type);
                                                if payload_type == "capability" {
                                                    // Handle capability tokens
                                                    let store_meta = if let Ok(capability_token) =
                                                        protocol::machine::root_as_capability_token(
                                                            &payload_bytes,
                                                        ) {
                                                        if let Some(root_capability) =
                                                            capability_token.root_capability()
                                                        {
                                                            if let Some(manifest_id) =
                                                                root_capability.manifest_id()
                                                            {
                                                                let meta = format!(
                                                                    "capability:{}",
                                                                    manifest_id
                                                                );
                                                                warn!("libp2p: keyshare extracted capability manifest_id={}, storing with metadata: {}", manifest_id, meta);
                                                                Some(meta)
                                                            } else {
                                                                warn!("libp2p: keyshare capability token has no manifest_id, storing with no metadata");
                                                                None
                                                            }
                                                        } else {
                                                            warn!("libp2p: keyshare capability token has no root_capability, storing with no metadata");
                                                            None
                                                        }
                                                    } else {
                                                        warn!("libp2p: keyshare failed to parse capability token, storing with no metadata");
                                                        None
                                                    };

                                                    if let Err(e) =
                                                        ks.put(&cid, &blob, store_meta.as_deref())
                                                    {
                                                        warn!(
                                                            "keystore put failed for cid {}: {:?}",
                                                            cid, e
                                                        );
                                                    } else {
                                                        debug!("keystore: stored simple keyshare cid={} type={}", cid, payload_type);
                                                        // Always announce the CID itself
                                                        let (reply_tx, _rx) =
                                                            tokio::sync::mpsc::unbounded_channel();
                                                        let ctrl = crate::libp2p_beemesh::control::Libp2pControl::AnnounceProvider { cid: cid.clone(), ttl_ms: 3000, reply_tx };
                                                        crate::libp2p_beemesh::control::enqueue_control(
                                                            ctrl,
                                                        );
                                                    }
                                                } else if payload_type == "keyshare" {
                                                    warn!("libp2p: keyshare processing keyshare payload");
                                                    // Handle keyshares - extract individual raw share bytes from KeyShares flatbuffer
                                                    let mut shares_stored = false;

                                                    // Try to parse as KeyShares flatbuffer and extract individual shares
                                                    // First try base64 decode if it's UTF-8
                                                    let mut keyshares_bytes_opt: Option<Vec<u8>> =
                                                        None;
                                                    if let Ok(payload_str) =
                                                        std::str::from_utf8(&payload_bytes)
                                                    {
                                                        if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(payload_str) {
                                                            keyshares_bytes_opt = Some(decoded);
                                                        }
                                                    }

                                                    let keyshares_result =
                                                        if let Some(ref keyshares_bytes) =
                                                            keyshares_bytes_opt
                                                        {
                                                            protocol::machine::root_as_key_shares(
                                                                keyshares_bytes,
                                                            )
                                                        } else {
                                                            protocol::machine::root_as_key_shares(
                                                                &payload_bytes,
                                                            )
                                                        };

                                                    if let Ok(key_shares) = keyshares_result {
                                                        let manifest_id = key_shares
                                                            .manifest_id()
                                                            .map(|id| id.to_string());
                                                        if let Some(shares_vector) =
                                                            key_shares.shares()
                                                        {
                                                            for i in 0..shares_vector.len() {
                                                                let share_b64 =
                                                                    shares_vector.get(i);
                                                                if let Ok(raw_share_bytes) =
                                                                    base64::engine::general_purpose::STANDARD.decode(share_b64)
                                                                {
                                                                    debug!("libp2p: keyshare extracting raw share {} len={}", i, raw_share_bytes.len());

                                                                    // Store this individual raw share
                                                                    if let Ok((share_blob, share_cid)) =
                                                                        crypto::encrypt_share_for_keystore(&raw_share_bytes)
                                                                    {
                                                                        if let Err(e) = ks.put(
                                                                            &share_cid,
                                                                            &share_blob,
                                                                            manifest_id.as_deref(),
                                                                        ) {
                                                                            warn!("libp2p: keyshare keystore put failed for share cid {}: {:?}", share_cid, e);
                                                                        } else {
                                                                            debug!("libp2p: keyshare keystore SUCCESS stored raw share {} cid={} for manifest_id={:?}", i, share_cid, manifest_id);
                                                                            shares_stored = true;

                                                                            // Announce this node as a provider for the manifest
                                                                            if let Some(ref mid) = manifest_id {
                                                                                let manifest_provider_cid = format!("manifest:{}", mid);
                                                                                warn!("libp2p: keyshare announcing provider for manifest_provider_cid={}", manifest_provider_cid);
                                                                                let (announce_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
                                                                                let ctrl = crate::libp2p_beemesh::control::Libp2pControl::AnnounceProvider {
                                                                                    cid: manifest_provider_cid,
                                                                                    ttl_ms: 3000,
                                                                                    reply_tx: announce_tx,
                                                                                };
                                                                                crate::libp2p_beemesh::control::enqueue_control(ctrl);
                                                                            }
                                                                        }
                                                                    } else {
                                                                        warn!("libp2p: keyshare encrypt_share_for_keystore failed for share {}", i);
                                                                    }
                                                                } else {
                                                                    warn!("libp2p: keyshare failed to decode share {} base64", i);
                                                                }
                                                            }
                                                        } else {
                                                            warn!("libp2p: keyshare KeyShares flatbuffer has no shares vector");
                                                        }
                                                    } else {
                                                        warn!("libp2p: keyshare failed to parse payload as KeyShares flatbuffer");
                                                    }

                                                    if !shares_stored {
                                                        warn!("libp2p: keyshare no shares were extracted and stored");
                                                    }
                                                } else {
                                                    // Unknown payload type - store as-is with basic metadata
                                                    let store_meta = if payload_type.is_empty() {
                                                        None
                                                    } else {
                                                        Some(payload_type.to_string())
                                                    };

                                                    if let Err(e) =
                                                        ks.put(&cid, &blob, store_meta.as_deref())
                                                    {
                                                        warn!(
                                                            "keystore put failed for cid {}: {:?}",
                                                            cid, e
                                                        );
                                                    } else {
                                                        debug!("keystore: stored simple keyshare cid={} type={}", cid, payload_type);
                                                        // Always announce the CID itself
                                                        let (reply_tx, _rx) =
                                                            tokio::sync::mpsc::unbounded_channel();
                                                        let ctrl = crate::libp2p_beemesh::control::Libp2pControl::AnnounceProvider { cid: cid.clone(), ttl_ms: 3000, reply_tx };
                                                        crate::libp2p_beemesh::control::enqueue_control(
                                                            ctrl,
                                                        );
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                warn!("could not open keystore: {:?}", e);
                                            }
                                        }
                                        let resp = protocol::machine::build_keyshare_response(
                                            true,
                                            "keyshare_op",
                                            "stored simple share",
                                        );
                                        let _ = swarm
                                            .behaviour_mut()
                                            .keyshare_rr
                                            .send_response(channel, resp);
                                    }
                                    Err(e) => {
                                        warn!("failed to encrypt share for keystore: {:?}", e);
                                        let resp = protocol::machine::build_keyshare_response(
                                            false,
                                            "keyshare_op",
                                            "encryption for storage failed",
                                        );
                                        let _ = swarm
                                            .behaviour_mut()
                                            .keyshare_rr
                                            .send_response(channel, resp);
                                    }
                                }
                                return;
                            }
                            Err(e) => {
                                warn!("simple envelope verification failed from {}: {:?}", peer, e);
                                // Fall through to try KEM-based processing
                            }
                        }
                    }

                    // Try flatbuffer KeyShareRequest encoded directly in effective_bytes.
                    if let Ok(kreq) = protocol::machine::root_as_key_share_request(&effective_bytes)
                    {
                        // Treat this as a keyshare upload: client provided KeyShareRequest as flatbuffer
                        // Validate envelope/signature fields if present inside flatbuffer
                        match crate::libp2p_beemesh::envelope::verify_flatbuffer_envelope(
                            &effective_bytes,
                            std::time::Duration::from_secs(300),
                        ) {
                            Ok((_payload_bytes, _pub, _sig)) => {
                                // The payload_bytes here are the canonical bytes; proceed to decapsulate and store as before
                                match crypto::ensure_kem_keypair_on_disk() {
                                    Ok((_pubb, privb)) => {
                                        match crypto::decapsulate_share(&privb, &_payload_bytes) {
                                            Ok(shared_secret) => {
                                                info!("libp2p: successfully decapsulated shared secret for peer={}", peer);
                                                drop(shared_secret);

                                                // Encrypt the provided payload and store in the local keystore.
                                                match crypto::encrypt_share_for_keystore(
                                                    &_payload_bytes,
                                                ) {
                                                    Ok((blob, cid)) => {
                                                        match crate::libp2p_beemesh::open_keystore()
                                                        {
                                                            Ok(ks) => {
                                                                warn!("attempting keystore.put for cid={} size={}", cid, blob.len());
                                                                // Use manifest_id from flatbuffer if provided.
                                                                let metadata = kreq
                                                                    .manifest_id()
                                                                    .map(|s| s.to_string());
                                                                let store_meta = metadata.clone();

                                                                if let Err(e) = ks.put(
                                                                    &cid,
                                                                    &blob,
                                                                    store_meta.as_deref(),
                                                                ) {
                                                                    warn!("keystore put failed for cid {}: {:?}", cid, e);
                                                                } else {
                                                                    if let Some(ref sm) = store_meta
                                                                    {
                                                                        info!("keystore: stored keyshare cid={} meta={}", cid, sm);
                                                                        if sm.starts_with(
                                                                            "capability:",
                                                                        ) {
                                                                            if let Some(
                                                                                manifest_id_val,
                                                                            ) = sm.strip_prefix(
                                                                                "capability:",
                                                                            ) {
                                                                                let manifest_provider_cid = format!(
                                                                                    "manifest:{}",
                                                                                    manifest_id_val
                                                                                );
                                                                                let (reply_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
                                                                                let ctrl = crate::libp2p_beemesh::control::Libp2pControl::AnnounceProvider { cid: manifest_provider_cid.clone(), ttl_ms: 3000, reply_tx };
                                                                                crate::libp2p_beemesh::control::enqueue_control(ctrl);
                                                                            }
                                                                        }
                                                                    }
                                                                    // Always announce the CID itself
                                                                    let (reply_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
                                                                    let ctrl = crate::libp2p_beemesh::control::Libp2pControl::AnnounceProvider { cid: cid.clone(), ttl_ms: 3000, reply_tx };
                                                                    crate::libp2p_beemesh::control::enqueue_control(ctrl);
                                                                }
                                                            }
                                                            Err(e) => {
                                                                warn!(
                                                                    "could not open keystore: {:?}",
                                                                    e
                                                                );
                                                            }
                                                        }
                                                        let resp = protocol::machine::build_keyshare_response(
                                                            true,
                                                            "keyshare_op",
                                                            "decapsulated, stored and announced",
                                                        );
                                                        let _ = swarm
                                                            .behaviour_mut()
                                                            .keyshare_rr
                                                            .send_response(channel, resp);
                                                    }
                                                    Err(e) => {
                                                        warn!("failed to encrypt share for keystore: {:?}", e);
                                                        let resp = protocol::machine::build_keyshare_response(
                                                            false,
                                                            "keyshare_op",
                                                            "encryption for storage failed",
                                                        );
                                                        let _ = swarm
                                                            .behaviour_mut()
                                                            .keyshare_rr
                                                            .send_response(channel, resp);
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                warn!("libp2p: failed to decapsulate share from {}: {:?}", peer, e);
                                                let resp =
                                                    protocol::machine::build_keyshare_response(
                                                        false,
                                                        "keyshare_op",
                                                        "decapsulation failed",
                                                    );
                                                let _ = swarm
                                                    .behaviour_mut()
                                                    .keyshare_rr
                                                    .send_response(channel, resp);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        warn!(
                                            "libp2p: could not read or create kem keypair: {:?}",
                                            e
                                        );
                                        let resp = protocol::machine::build_keyshare_response(
                                            false,
                                            "keyshare_op",
                                            "server kem key unavailable",
                                        );
                                        let _ = swarm
                                            .behaviour_mut()
                                            .keyshare_rr
                                            .send_response(channel, resp);
                                    }
                                }
                            }
                            Err(e) => {
                                warn!(
                                    "flatbuffer envelope verification failed from {}: {:?}",
                                    peer, e
                                );
                                let resp = protocol::machine::build_keyshare_response(
                                    false,
                                    "keyshare_op",
                                    "envelope verification failed",
                                );
                                let _ = swarm
                                    .behaviour_mut()
                                    .keyshare_rr
                                    .send_response(channel, resp);
                            }
                        }
                        return;
                    }

                    // Not a flatbuffer KeyShareRequest — reject legacy JSON-based payloads outright.
                    warn!(
                        "libp2p: rejecting non-flatbuffer keyshare request from {}",
                        peer
                    );
                    let resp = protocol::machine::build_keyshare_response(
                        false,
                        "keyshare_op",
                        "unsupported_payload_format",
                    );
                    let _ = swarm
                        .behaviour_mut()
                        .keyshare_rr
                        .send_response(channel, resp);
                }
                Err(e) => {
                    warn!(
                        "libp2p: failed to preprocess recipient blob from {}: {:?}",
                        peer, e
                    );
                    let resp = protocol::machine::build_keyshare_response(
                        false,
                        "keyshare_op",
                        "invalid recipient blob",
                    );
                    let _ = swarm
                        .behaviour_mut()
                        .keyshare_rr
                        .send_response(channel, resp);
                }
            }
        }
        request_response::Message::Response { response, .. } => {
            debug!("libp2p: received keyshare response from peer={}", peer);
            // Try to parse as KeyShareResponse and forward to pending waiter if present
            match protocol::machine::root_as_key_share_response(&response) {
                Ok(resp) => {
                    let ok = resp.ok();
                    let msg = resp.message().map(|s| s.to_string()).unwrap_or_default();
                    debug!("libp2p: keyshare response - ok={} msg={}", ok, msg);
                    // If there's a pending fetch waiter for this peer, forward the raw response bytes
                    if let Some(tx) = crate::libp2p_beemesh::control::take_pending_keyshare_for_peer(
                        &peer.to_string(),
                    ) {
                        // send the raw response bytes so caller can parse
                        let _ = tx.send(Ok(response.clone()));
                    }
                }
                Err(e) => {
                    warn!("libp2p: failed to parse keyshare response: {:?}", e);
                    if let Some(tx) = crate::libp2p_beemesh::control::take_pending_keyshare_for_peer(
                        &peer.to_string(),
                    ) {
                        let _ = tx.send(Err(format!("failed to parse keyshare response: {:?}", e)));
                    }
                }
            }
        }
    }
}

/// Verify capability token holder signature
/// Ensures the presenting peer actually signed the token presentation
fn verify_capability_holder_signature(
    token_bytes: &[u8],
    manifest_id: &str,
    presenting_peer: &libp2p::PeerId,
) -> Result<(), anyhow::Error> {
    // Parse the capability token
    let capability_token = protocol::machine::root_as_capability_token(token_bytes)
        .map_err(|e| anyhow::anyhow!("Failed to parse capability token: {}", e))?;

    // Check if we have a signature chain
    let signature_chain = capability_token
        .signature_chain()
        .ok_or_else(|| anyhow::anyhow!("Missing signature chain"))?;

    if signature_chain.len() == 0 {
        return Err(anyhow::anyhow!("Empty signature chain"));
    }

    // Get the holder signature (should be the last/only entry)
    let signature_entry = signature_chain.get(0);

    let holder_peer_id_str = signature_entry
        .signer_peer_id()
        .ok_or_else(|| anyhow::anyhow!("Missing signer peer ID"))?;

    let holder_pub_key = signature_entry
        .public_key()
        .ok_or_else(|| anyhow::anyhow!("Missing public key"))?
        .iter()
        .collect::<Vec<u8>>();

    let holder_signature = signature_entry
        .signature()
        .ok_or_else(|| anyhow::anyhow!("Missing signature"))?
        .iter()
        .collect::<Vec<u8>>();

    let presentation_nonce = signature_entry
        .presentation_nonce()
        .ok_or_else(|| anyhow::anyhow!("Missing presentation nonce"))?;

    let presentation_timestamp = signature_entry.presentation_timestamp();

    // Get the root capability for token reconstruction
    let root_cap = capability_token
        .root_capability()
        .ok_or_else(|| anyhow::anyhow!("Missing root capability"))?;

    // Verify the holder peer ID matches the presenting peer
    if holder_peer_id_str != presenting_peer.to_string() {
        return Err(anyhow::anyhow!(
            "Holder peer ID mismatch: expected {}, got {}",
            presenting_peer,
            holder_peer_id_str
        ));
    }

    // Get authorized peer from caveats to verify authorization
    let authorized_peer = if let Some(caveats) = capability_token.caveats() {
        if caveats.len() > 0 {
            let caveat = caveats.get(0);
            if caveat.condition_type().unwrap_or("") == "authorized_peer" {
                if let Some(value) = caveat.value() {
                    let value_bytes: Vec<u8> = value.iter().collect();
                    std::str::from_utf8(&value_bytes).unwrap_or("").to_string()
                } else {
                    String::new()
                }
            } else {
                String::new()
            }
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    // Verify the presenting peer is authorized
    if authorized_peer != presenting_peer.to_string() {
        return Err(anyhow::anyhow!(
            "Peer {} not authorized for this capability (authorized: {})",
            presenting_peer,
            authorized_peer
        ));
    }

    // Reconstruct the original unsigned token to create the presentation context
    // The presentation context must be created using the original token, not the holder-signed token
    let original_token = protocol::machine::build_capability_token(
        manifest_id,
        &root_cap.issuer_peer_id().unwrap_or(""),
        &authorized_peer,
        root_cap.issued_at(),
        root_cap.expires_at(),
    );

    let presentation_context = crypto::create_capability_presentation_context(
        &original_token,
        presentation_nonce,
        presentation_timestamp,
        manifest_id,
        "KeyShareRequest",
    );

    // Verify the holder signature
    crypto::verify_capability_holder_signature(
        &holder_pub_key,
        &presentation_context,
        &holder_signature,
    )
    .map_err(|e| anyhow::anyhow!("Holder signature verification failed: {}", e))?;

    log::info!(
        "libp2p: verified capability holder signature for peer {} on manifest {}",
        presenting_peer,
        manifest_id
    );

    Ok(())
}
