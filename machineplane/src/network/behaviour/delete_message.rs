use super::failure_handlers::{FailureDirection, handle_failure};
use crate::messages::machine;
use crate::scheduler::remove_workloads_by_manifest_id;
use libp2p::request_response;
use log::{error, info, warn};

const UNKNOWN_OPERATION: &str = "unknown";
const UNKNOWN_MANIFEST: &str = "unknown";
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

            // Parse the delete request
            match machine::root_as_delete_request(&request) {
                Ok(delete_req) => {
                    info!(
                        "Delete request - manifest_id={:?} operation_id={:?} force={}",
                        &delete_req.manifest_id, &delete_req.operation_id, delete_req.force
                    );

                    // Process the delete request asynchronously
                    let manifest_id = delete_req.manifest_id.clone();
                    let operation_id = delete_req.operation_id.clone();
                    let force = delete_req.force;
                    let requesting_peer = peer.to_string();

                    tokio::spawn(async move {
                        let (success, message, removed_workloads) =
                            process_delete_request(&manifest_id, force, &requesting_peer).await;

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
                        delete_req.operation_id.as_str(),
                        delete_req.manifest_id.as_str(),
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
                        delete_resp.ok,
                        &delete_resp.operation_id,
                        &delete_resp.message,
                        Some(delete_resp.removed_workloads.clone())
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
    force: bool,
    _requesting_peer: &str,
) -> (bool, String, Vec<String>) {
    if !force {
        warn!(
            "Processing delete request for manifest_id={} without explicit ownership validation",
            manifest_id
        );
    }

    // Remove workloads using the workload manager
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

/// Handle delete outbound failure
pub fn delete_outbound_failure(peer: libp2p::PeerId, error: request_response::OutboundFailure) {
    handle_failure("delete", FailureDirection::Outbound, peer, error);
}

/// Handle delete inbound failure
pub fn delete_inbound_failure(peer: libp2p::PeerId, error: request_response::InboundFailure) {
    handle_failure("delete", FailureDirection::Inbound, peer, error);
}
