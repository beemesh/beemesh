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
    pub fn decode_capacity_request(buf: &[u8]) -> bincode::Result<CapacityRequest> {
        deserialize(buf)
    }

    pub fn decode_capacity_reply(buf: &[u8]) -> bincode::Result<CapacityReply> {
        deserialize(buf)
    }

    pub fn decode_health(buf: &[u8]) -> bincode::Result<Health> {
        deserialize(buf)
    }

    pub fn decode_apply_request(buf: &[u8]) -> bincode::Result<ApplyRequest> {
        deserialize(buf)
    }

    pub fn decode_apply_response(buf: &[u8]) -> bincode::Result<ApplyResponse> {
        deserialize(buf)
    }

    pub fn decode_delete_request(buf: &[u8]) -> bincode::Result<DeleteRequest> {
        deserialize(buf)
    }

    pub fn decode_delete_response(buf: &[u8]) -> bincode::Result<DeleteResponse> {
        deserialize(buf)
    }

    pub fn decode_handshake(buf: &[u8]) -> bincode::Result<Handshake> {
        deserialize(buf)
    }

    pub fn decode_applied_manifest(buf: &[u8]) -> bincode::Result<AppliedManifest> {
        deserialize(buf)
    }

    pub fn decode_tender(buf: &[u8]) -> bincode::Result<Tender> {
        deserialize(buf)
    }

    pub fn decode_bid(buf: &[u8]) -> bincode::Result<Bid> {
        deserialize(buf)
    }

    pub fn decode_scheduler_event(buf: &[u8]) -> bincode::Result<SchedulerEvent> {
        deserialize(buf)
    }

    pub fn decode_lease_hint(buf: &[u8]) -> bincode::Result<LeaseHint> {
        deserialize(buf)
    }

    pub fn decode_candidates_response(buf: &[u8]) -> bincode::Result<CandidatesResponse> {
        deserialize(buf)
    }

    pub fn decode_nodes_response(buf: &[u8]) -> bincode::Result<NodesResponse> {
        deserialize(buf)
    }

    pub fn decode_tender_create_response(buf: &[u8]) -> bincode::Result<TenderCreateResponse> {
        deserialize(buf)
    }

    pub fn decode_tender_status_response(buf: &[u8]) -> bincode::Result<TenderStatusResponse> {
        deserialize(buf)
    }

    // ---------------------------- Builders ---------------------------------
    pub fn build_health(ok: bool, status: &str) -> Vec<u8> {
        serialize(&Health {
            ok,
            status: status.to_string(),
        })
    }

    pub fn build_capacity_request_with_id(
        request_id: &str,
        cpu_milli: u32,
        memory_bytes: u64,
        storage_bytes: u64,
        replicas: u32,
    ) -> Vec<u8> {
        serialize(&CapacityRequest {
            request_id: request_id.to_string(),
            cpu_milli,
            memory_bytes,
            storage_bytes,
            replicas,
        })
    }

    pub fn build_capacity_request(
        cpu_milli: u32,
        memory_bytes: u64,
        storage_bytes: u64,
        replicas: u32,
    ) -> Vec<u8> {
        build_capacity_request_with_id("", cpu_milli, memory_bytes, storage_bytes, replicas)
    }

    pub fn build_capacity_reply(
        ok: bool,
        cpu_available_milli: u32,
        memory_available_bytes: u64,
        storage_available_bytes: u64,
        request_id: &str,
        node_id: &str,
        region: &str,
        kem_pubkey: Option<&str>,
        capabilities: &[&str],
    ) -> Vec<u8> {
        serialize(&CapacityReply {
            request_id: request_id.to_string(),
            ok,
            node_id: node_id.to_string(),
            region: region.to_string(),
            kem_pubkey: kem_pubkey.unwrap_or_default().to_string(),
            capabilities: capabilities.iter().map(|c| c.to_string()).collect(),
            cpu_available_milli,
            memory_available_bytes,
            storage_available_bytes,
        })
    }

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

    pub fn build_handshake(
        nonce: u32,
        timestamp: u64,
        protocol_version: &str,
        signature: &str,
    ) -> Vec<u8> {
        serialize(&Handshake {
            nonce,
            timestamp,
            protocol_version: protocol_version.to_string(),
            signature: signature.to_string(),
        })
    }

    pub fn build_applied_manifest(
        id: &str,
        operation_id: &str,
        origin_peer: &str,
        owner_pubkey: &[u8],
        signature: &[u8],
        signature_scheme: SignatureScheme,
        manifest_json: &str,
        manifest_kind: &str,
        labels: &[KeyValue],
        timestamp: u64,
        operation: OperationType,
        ttl_secs: u32,
        content_hash: &str,
    ) -> Vec<u8> {
        serialize(&AppliedManifest {
            id: id.to_string(),
            operation_id: operation_id.to_string(),
            origin_peer: origin_peer.to_string(),
            owner_pubkey: owner_pubkey.to_vec(),
            signature_scheme,
            signature: signature.to_vec(),
            manifest_json: manifest_json.to_string(),
            manifest_kind: manifest_kind.to_string(),
            labels: labels.to_vec(),
            timestamp,
            operation,
            ttl_secs,
            content_hash: content_hash.to_string(),
        })
    }

    pub fn build_manifest_target(peer_id: &str, payload_json: &str) -> (String, String) {
        let content_hash = compute_manifest_id_from_content(payload_json.as_bytes());
        let key = format!("manifest:{}:{}", peer_id, content_hash);
        (key, content_hash)
    }

    pub fn build_nodes_response(peers: &[String]) -> Vec<u8> {
        serialize(&NodesResponse {
            peers: peers.to_vec(),
        })
    }

    pub fn build_tender(
        id: &str,
        manifest_ref: &str,
        manifest_json: &str,
        requirements_cpu_cores: u32,
        requirements_memory_mb: u32,
        workload_type: &str,
        duplicate_tolerant: bool,
        placement_token: &str,
        qos_preemptible: bool,
        timestamp: u64,
    ) -> Vec<u8> {
        serialize(&Tender {
            id: id.to_string(),
            manifest_ref: manifest_ref.to_string(),
            manifest_json: manifest_json.to_string(),
            requirements_cpu_cores,
            requirements_memory_mb,
            workload_type: workload_type.to_string(),
            duplicate_tolerant,
            placement_token: placement_token.to_string(),
            qos_preemptible,
            timestamp,
            signature: Vec::new(),
        })
    }

    pub fn build_bid(
        tender_id: &str,
        node_id: &str,
        score: f64,
        resource_fit_score: f64,
        network_locality_score: f64,
        timestamp: u64,
        signature: &[u8],
    ) -> Vec<u8> {
        serialize(&Bid {
            tender_id: tender_id.to_string(),
            node_id: node_id.to_string(),
            score,
            resource_fit_score,
            network_locality_score,
            timestamp,
            signature: signature.to_vec(),
        })
    }

    pub fn build_lease_hint(
        tender_id: &str,
        node_id: &str,
        score: f64,
        ttl_ms: u32,
        renew_nonce: u64,
        timestamp: u64,
        signature: &[u8],
    ) -> Vec<u8> {
        serialize(&LeaseHint {
            tender_id: tender_id.to_string(),
            node_id: node_id.to_string(),
            score,
            ttl_ms,
            renew_nonce,
            timestamp,
            signature: signature.to_vec(),
        })
    }

    pub fn build_candidates_response_with_keys(ok: bool, candidates: &[CandidateNode]) -> Vec<u8> {
        serialize(&CandidatesResponse {
            ok,
            candidates: candidates.to_vec(),
        })
    }

    pub fn build_candidates_response(ok: bool, responders: &[String]) -> Vec<u8> {
        let candidate_nodes: Vec<CandidateNode> = responders
            .iter()
            .map(|peer_id| CandidateNode {
                peer_id: peer_id.clone(),
                public_key: String::new(),
            })
            .collect();
        build_candidates_response_with_keys(ok, &candidate_nodes)
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

    pub fn compute_manifest_id(name: &str, version: u64) -> String {
        format!("{}:{}", name, version)
    }

    pub fn compute_manifest_id_from_content(manifest_data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(manifest_data);
        let hash = hasher.finalize();
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(hash)
    }
}
