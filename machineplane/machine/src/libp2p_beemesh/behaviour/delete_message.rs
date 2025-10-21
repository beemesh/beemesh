use super::message_verifier::verify_signed_message;
use libp2p::request_response;
use log::{error, info, warn};
use protocol::machine;

const UNKNOWN_OPERATION: &str = "unknown";
const UNKNOWN_MANIFEST: &str = "unknown";
const UNSIGNED_MESSAGE: &str = "unsigned or invalid envelope";
const INVALID_FORMAT: &str = "invalid delete request format";
const ACK_MESSAGE: &str = "delete request received and processing";

fn delete_error_response(message: &str) -> Vec<u8> {
    machine::build_delete_response(false, UNKNOWN_OPERATION, message, UNKNOWN_MANIFEST, &[])
}

fn delete_ack_response(operation_id: &str, manifest_id: &str) -> Vec<u8> {
    machine::build_delete_response(true, operation_id, ACK_MESSAGE, manifest_id, &[])
}

/// Handle a delete message (request or response)
pub fn delete_message(
    message: request_response::Message<Vec<u8>, Vec<u8>>,
    peer: libp2p::PeerId,
    swarm: &mut libp2p::Swarm<super::MyBehaviour>,
    _local_peer: libp2p::PeerId,
) {
    match message {
        request_response::Message::Request {
            request, channel, ..
        } => {
            info!("Received delete request from peer={}", peer);

            let verified = match verify_signed_message(&peer, &request, |err| {
                error!("Rejecting delete request with invalid signature: {}", err);
            }) {
                Some(envelope) => envelope,
                None => {
                    let error_response = delete_error_response(UNSIGNED_MESSAGE);
                    let _ = swarm
                        .behaviour_mut()
                        .delete_rr
                        .send_response(channel, error_response);
                    return;
                }
            };

            let envelope_pubkey = verified.pubkey.clone();
            let effective_request = verified.payload;

            // Parse the FlatBuffer delete request
            match machine::root_as_delete_request(&effective_request) {
                Ok(delete_req) => {
                    info!(
                        "Delete request - manifest_id={:?} tenant={:?} operation_id={:?} force={}",
                        delete_req.manifest_id(),
                        delete_req.tenant(),
                        delete_req.operation_id(),
                        delete_req.force()
                    );

                    // Process the delete request asynchronously
                    let manifest_id = delete_req.manifest_id().unwrap_or("").to_string();
                    let tenant = delete_req.tenant().unwrap_or("").to_string();
                    let operation_id = delete_req.operation_id().unwrap_or("").to_string();
                    let force = delete_req.force();
                    let requesting_peer = peer.to_string();
                    let envelope_pubkey_inner = envelope_pubkey.clone();

                    tokio::spawn(async move {
                        let (success, message, removed_workloads) = process_delete_request(
                            &manifest_id,
                            &tenant,
                            force,
                            &envelope_pubkey_inner,
                            &requesting_peer,
                        )
                        .await;

                        let _response = machine::build_delete_response(
                            success,
                            &operation_id,
                            &message,
                            &manifest_id,
                            &removed_workloads,
                        );

                        // Note: In a real implementation, we'd need to send this response back through a channel
                        // For now, we'll just log the result
                        info!(
                            "Delete request processed: success={} message={} removed_workloads={:?}",
                            success, message, removed_workloads
                        );
                    });

                    // Send immediate acknowledgment (the actual processing happens async)
                    let ack_response = delete_ack_response(
                        delete_req.operation_id().unwrap_or(UNKNOWN_OPERATION),
                        delete_req.manifest_id().unwrap_or(UNKNOWN_MANIFEST),
                    );
                    let _ = swarm
                        .behaviour_mut()
                        .delete_rr
                        .send_response(channel, ack_response);
                }
                Err(e) => {
                    error!("Failed to parse delete request: {}", e);
                    let error_response = delete_error_response(INVALID_FORMAT);
                    let _ = swarm
                        .behaviour_mut()
                        .delete_rr
                        .send_response(channel, error_response);
                }
            }
        }
        request_response::Message::Response { response, .. } => {
            info!("Received delete response from peer={}", peer);

            // Parse the response
            match machine::root_as_delete_response(&response) {
                Ok(delete_resp) => {
                    info!(
                        "Delete response - ok={} operation_id={:?} message={:?} removed_workloads={:?}",
                        delete_resp.ok(),
                        delete_resp.operation_id(),
                        delete_resp.message(),
                        delete_resp.removed_workloads().map(|w| w.iter().map(|s| s.to_string()).collect::<Vec<_>>())
                    );
                }
                Err(e) => {
                    warn!("Failed to parse delete response: {:?}", e);
                }
            }
        }
    }
}

/// Process a delete request by verifying ownership and removing workloads
async fn process_delete_request(
    manifest_id: &str,
    tenant: &str,
    force: bool,
    envelope_pubkey: &[u8],
    _requesting_peer: &str,
) -> (bool, String, Vec<String>) {
    // Step 1: Verify ownership if not force delete
    if !force {
        match verify_delete_ownership(manifest_id, tenant, envelope_pubkey).await {
            Ok(true) => {
                info!("Delete ownership verified for manifest_id={}", manifest_id);
            }
            Ok(false) => {
                warn!(
                    "Delete ownership verification failed for manifest_id={}",
                    manifest_id
                );
                return (false, "ownership verification failed".to_string(), vec![]);
            }
            Err(e) => {
                error!(
                    "Delete ownership verification error for manifest_id={}: {}",
                    manifest_id, e
                );
                return (
                    false,
                    format!("ownership verification error: {}", e),
                    vec![],
                );
            }
        }
    } else {
        info!(
            "Forced delete for manifest_id={}, skipping ownership verification",
            manifest_id
        );
    }

    // Step 2: Remove workloads using the workload manager
    match remove_workloads_by_manifest_id(manifest_id).await {
        Ok(removed_workloads) => {
            if removed_workloads.is_empty() {
                info!("No workloads found for manifest_id={}", manifest_id);
                (
                    true,
                    "no workloads found for manifest".to_string(),
                    removed_workloads,
                )
            } else {
                info!(
                    "Successfully removed {} workloads for manifest_id={}",
                    removed_workloads.len(),
                    manifest_id
                );
                (
                    true,
                    format!("removed {} workloads", removed_workloads.len()),
                    removed_workloads,
                )
            }
        }
        Err(e) => {
            error!(
                "Failed to remove workloads for manifest_id={}: {}",
                manifest_id, e
            );
            (false, format!("failed to remove workloads: {}", e), vec![])
        }
    }
}

/// Verify that the requesting peer is the owner of the manifest
async fn verify_delete_ownership(
    manifest_id: &str,
    _tenant: &str,
    envelope_pubkey: &[u8],
) -> Result<bool, anyhow::Error> {
    // TODO: Implement DHT lookup to get the AppliedManifest record
    // For now, we'll implement a simplified version that always allows deletion
    // In the real implementation, this should:
    // 1. Query DHT for AppliedManifest record using manifest_id
    // 2. Extract owner_pubkey from the stored manifest
    // 3. Compare with envelope_pubkey from delete request
    // 4. Return true if they match, false otherwise

    info!(
        "verify_delete_ownership: manifest_id={} envelope_pubkey_len={}",
        manifest_id,
        envelope_pubkey.len()
    );

    // For testing purposes, allow deletion if envelope_pubkey is not empty
    Ok(!envelope_pubkey.is_empty())
}

/// Remove workloads associated with a manifest ID
async fn remove_workloads_by_manifest_id(manifest_id: &str) -> Result<Vec<String>, anyhow::Error> {
    info!(
        "remove_workloads_by_manifest_id: manifest_id={}",
        manifest_id
    );

    // Use the integrated workload manager to remove workloads
    match crate::workload_integration::remove_workloads_by_manifest_id(manifest_id).await {
        Ok(removed_workloads) => {
            info!(
                "Successfully removed {} workloads for manifest_id '{}'",
                removed_workloads.len(),
                manifest_id
            );
            Ok(removed_workloads)
        }
        Err(e) => {
            error!(
                "Failed to remove workloads for manifest_id '{}': {}",
                manifest_id, e
            );
            Err(anyhow::anyhow!("Failed to remove workloads: {}", e))
        }
    }
}

/// Handle delete outbound failure
pub fn delete_outbound_failure(peer: libp2p::PeerId, error: request_response::OutboundFailure) {
    warn!("Delete outbound failure for peer {}: {:?}", peer, error);
}

/// Handle delete inbound failure  
pub fn delete_inbound_failure(peer: libp2p::PeerId, error: request_response::InboundFailure) {
    warn!("Delete inbound failure for peer {}: {:?}", peer, error);
}
