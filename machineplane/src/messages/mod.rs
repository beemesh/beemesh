//! Message helpers for BeeMesh.
//!
//! This module provides serde + bincode helpers over the message types defined
//! in `types.rs`. The previous FlatBuffers-specific helpers have been removed in
//! favour of idiomatic binary encoders/decoders.

use base64::Engine;
use serde::{Deserialize, Serialize};

pub mod types;
pub use types::*;
pub mod constants;
pub use constants as libp2p_constants;
pub mod signatures;

fn serialize<T: Serialize>(value: &T) -> Vec<u8> {
    bincode::serialize(value).expect("failed to serialize message")
}

fn deserialize<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> bincode::Result<T> {
    bincode::deserialize(bytes)
}

pub mod machine {
    use super::*;
    use sha2::{Digest, Sha256};

    // ------------------------------ Decoders -------------------------------
    pub fn decode_apply_request(buf: &[u8]) -> bincode::Result<ApplyRequest> {
        deserialize(buf)
    }

    pub fn encode_scheduler_message(message: SchedulerMessage) -> Vec<u8> {
        serialize(&message)
    }

    pub fn decode_scheduler_message(buf: &[u8]) -> bincode::Result<SchedulerMessage> {
        deserialize(buf)
    }

    pub fn decode_manifest_transfer(buf: &[u8]) -> bincode::Result<ManifestTransfer> {
        deserialize(buf)
    }

    // ---------------------------- Builders ---------------------------------
    pub fn build_apply_request(
        replicas: u32,
        operation_id: &str,
        manifest_json: &str,
        origin_peer: &str,
        manifest_id: &str,
    ) -> Vec<u8> {
        serialize(&ApplyRequest {
            replicas,
            operation_id: operation_id.to_string(),
            manifest_json: manifest_json.to_string(),
            origin_peer: origin_peer.to_string(),
            manifest_id: manifest_id.to_string(),
            signature: Vec::new(),
        })
    }

    pub fn build_apply_response(ok: bool, operation_id: &str, message: &str) -> Vec<u8> {
        serialize(&ApplyResponse {
            ok,
            operation_id: operation_id.to_string(),
            message: message.to_string(),
            signature: Vec::new(),
        })
    }

    pub fn build_delete_request(
        manifest_id: &str,
        operation_id: &str,
        origin_peer: &str,
        force: bool,
    ) -> Vec<u8> {
        serialize(&DeleteRequest {
            manifest_id: manifest_id.to_string(),
            operation_id: operation_id.to_string(),
            origin_peer: origin_peer.to_string(),
            force,
        })
    }

    pub fn build_delete_response(
        ok: bool,
        operation_id: &str,
        message: &str,
        manifest_id: &str,
        removed_workloads: &[String],
    ) -> Vec<u8> {
        serialize(&DeleteResponse {
            ok,
            operation_id: operation_id.to_string(),
            message: message.to_string(),
            manifest_id: manifest_id.to_string(),
            removed_workloads: removed_workloads.to_vec(),
        })
    }

    pub fn build_nodes_response(peers: &[String]) -> Vec<u8> {
        serialize(&NodesResponse {
            peers: peers.to_vec(),
        })
    }

    pub fn build_candidates_response_with_keys(ok: bool, candidates: &[CandidateNode]) -> Vec<u8> {
        serialize(&CandidatesResponse {
            ok,
            candidates: candidates.to_vec(),
        })
    }

    pub fn build_tender_create_response(
        ok: bool,
        tender_id: &str,
        manifest_ref: &str,
        selection_window_ms: u64,
        message: &str,
    ) -> Vec<u8> {
        serialize(&TenderCreateResponse {
            ok,
            tender_id: tender_id.to_string(),
            manifest_ref: manifest_ref.to_string(),
            selection_window_ms,
            message: message.to_string(),
        })
    }

    pub fn build_tender_status_response(
        tender_id: &str,
        state: &str,
        assigned_peers: &[String],
        manifest_cid: Option<&str>,
    ) -> Vec<u8> {
        serialize(&TenderStatusResponse {
            tender_id: tender_id.to_string(),
            state: state.to_string(),
            assigned_peers: assigned_peers.to_vec(),
            manifest_cid: manifest_cid.unwrap_or_default().to_string(),
        })
    }

    pub fn extract_manifest_name(manifest_data: &[u8]) -> Option<String> {
        let value: serde_json::Value = serde_json::from_slice(manifest_data).ok()?;
        value
            .get("metadata")
            .and_then(|m| m.get("name"))
            .and_then(|n| n.as_str())
            .map(|s| s.to_string())
    }

    pub fn compute_manifest_id_from_content(manifest_data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(manifest_data);
        let hash = hasher.finalize();
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(hash)
    }

    pub fn build_manifest_transfer(transfer: &ManifestTransfer) -> Vec<u8> {
        serialize(transfer)
    }
}
