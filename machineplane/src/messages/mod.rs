//! Message helpers for BeeMesh.
//!
//! The codebase previously relied on FlatBuffers-generated modules and build
//! scripts to produce Rust bindings. Those artifacts were removed in the
//! decentralised resource pool refactor, so this module now implements
//! equivalent helpers using serde + bincode over the message types defined in
//! `types.rs`.

use base64::Engine;
use serde::{Deserialize, Serialize};

pub mod types;
pub use types::*;

fn serialize<T: Serialize>(value: &T) -> Vec<u8> {
    bincode::serialize(value).expect("failed to serialize message")
}

fn deserialize<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> bincode::Result<T> {
    bincode::deserialize(bytes)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Envelope {
    pub payload: Vec<u8>,
    pub payload_type: String,
    pub nonce: String,
    pub ts: u64,
    pub alg: String,
    pub sig: String,
    pub pubkey: String,
    pub kem_pubkey: String,
    pub peer_id: String,
}

pub mod machine {
    use super::*;
    use sha2::{Digest, Sha256};

    // ---------------------------- Root readers -----------------------------
    pub fn root_as_capacity_request(buf: &[u8]) -> bincode::Result<CapacityRequest> {
        deserialize(buf)
    }

    pub fn root_as_capacity_reply(buf: &[u8]) -> bincode::Result<CapacityReply> {
        deserialize(buf)
    }

    pub fn root_as_health(buf: &[u8]) -> bincode::Result<Health> {
        deserialize(buf)
    }

    pub fn root_as_apply_request(buf: &[u8]) -> bincode::Result<ApplyRequest> {
        deserialize(buf)
    }

    pub fn root_as_apply_response(buf: &[u8]) -> bincode::Result<ApplyResponse> {
        deserialize(buf)
    }

    pub fn root_as_delete_request(buf: &[u8]) -> bincode::Result<DeleteRequest> {
        deserialize(buf)
    }

    pub fn root_as_delete_response(buf: &[u8]) -> bincode::Result<DeleteResponse> {
        deserialize(buf)
    }

    pub fn root_as_handshake(buf: &[u8]) -> bincode::Result<Handshake> {
        deserialize(buf)
    }

    pub fn root_as_envelope(buf: &[u8]) -> bincode::Result<Envelope> {
        deserialize(buf)
    }

    pub fn root_as_applied_manifest(buf: &[u8]) -> bincode::Result<AppliedManifest> {
        deserialize(buf)
    }

    pub fn root_as_task(buf: &[u8]) -> bincode::Result<Task> {
        deserialize(buf)
    }

    pub fn root_as_bid(buf: &[u8]) -> bincode::Result<Bid> {
        deserialize(buf)
    }

    pub fn root_as_scheduler_event(buf: &[u8]) -> bincode::Result<SchedulerEvent> {
        deserialize(buf)
    }

    pub fn root_as_lease_hint(buf: &[u8]) -> bincode::Result<LeaseHint> {
        deserialize(buf)
    }

    pub fn root_as_candidates_response(buf: &[u8]) -> bincode::Result<CandidatesResponse> {
        deserialize(buf)
    }

    pub fn root_as_nodes_response(buf: &[u8]) -> bincode::Result<NodesResponse> {
        deserialize(buf)
    }

    pub fn root_as_task_create_response(buf: &[u8]) -> bincode::Result<TaskCreateResponse> {
        deserialize(buf)
    }

    pub fn root_as_task_status_response(buf: &[u8]) -> bincode::Result<TaskStatusResponse> {
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
        })
    }

    pub fn build_apply_response(ok: bool, operation_id: &str, message: &str) -> Vec<u8> {
        serialize(&ApplyResponse {
            ok,
            operation_id: operation_id.to_string(),
            message: message.to_string(),
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

    pub fn build_envelope_canonical(
        payload: &[u8],
        payload_type: &str,
        nonce: &str,
        timestamp: u64,
        algorithm: &str,
        kem_pub_b64: Option<&str>,
    ) -> Vec<u8> {
        serialize(&Envelope {
            payload: payload.to_vec(),
            payload_type: payload_type.to_string(),
            nonce: nonce.to_string(),
            ts: timestamp,
            alg: algorithm.to_string(),
            sig: String::new(),
            pubkey: String::new(),
            kem_pubkey: kem_pub_b64.unwrap_or_default().to_string(),
            peer_id: String::new(),
        })
    }

    pub fn build_envelope_signed(
        payload: &[u8],
        payload_type: &str,
        nonce: &str,
        timestamp: u64,
        algorithm: &str,
        sig_prefix: &str,
        sig_b64: &str,
        pubkey_b64: &str,
        kem_pub_b64: Option<&str>,
    ) -> Vec<u8> {
        serialize(&Envelope {
            payload: payload.to_vec(),
            payload_type: payload_type.to_string(),
            nonce: nonce.to_string(),
            ts: timestamp,
            alg: algorithm.to_string(),
            sig: format!("{}:{}", sig_prefix, sig_b64),
            pubkey: pubkey_b64.to_string(),
            kem_pubkey: kem_pub_b64.unwrap_or_default().to_string(),
            peer_id: String::new(),
        })
    }

    pub fn build_envelope_canonical_with_peer(
        payload: &[u8],
        payload_type: &str,
        nonce: &str,
        timestamp: u64,
        algorithm: &str,
        peer_id: &str,
        kem_pub_b64: Option<&str>,
    ) -> Vec<u8> {
        serialize(&Envelope {
            payload: payload.to_vec(),
            payload_type: payload_type.to_string(),
            nonce: nonce.to_string(),
            ts: timestamp,
            alg: algorithm.to_string(),
            sig: String::new(),
            pubkey: String::new(),
            kem_pubkey: kem_pub_b64.unwrap_or_default().to_string(),
            peer_id: peer_id.to_string(),
        })
    }

    pub fn build_envelope_signed_with_peer(
        payload: &[u8],
        payload_type: &str,
        nonce: &str,
        timestamp: u64,
        algorithm: &str,
        sig_prefix: &str,
        sig_b64: &str,
        pubkey_b64: &str,
        peer_id: &str,
        kem_pub_b64: Option<&str>,
    ) -> Vec<u8> {
        serialize(&Envelope {
            payload: payload.to_vec(),
            payload_type: payload_type.to_string(),
            nonce: nonce.to_string(),
            ts: timestamp,
            alg: algorithm.to_string(),
            sig: format!("{}:{}", sig_prefix, sig_b64),
            pubkey: pubkey_b64.to_string(),
            kem_pubkey: kem_pub_b64.unwrap_or_default().to_string(),
            peer_id: peer_id.to_string(),
        })
    }

    pub fn fb_envelope_extract_sig_pub(envelope_bytes: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
        let env: Envelope = deserialize(envelope_bytes).ok()?;
        let sig_field = env.sig;
        let sig_b64 = sig_field
            .splitn(2, ':')
            .nth(if sig_field.contains(':') { 1 } else { 0 })
            .unwrap_or(&sig_field)
            .to_string();
        let sig_bytes = base64::engine::general_purpose::STANDARD.decode(sig_b64).ok()?;
        let pub_bytes = base64::engine::general_purpose::STANDARD
            .decode(env.pubkey)
            .ok()?;
        Some((sig_bytes, pub_bytes))
    }

    pub fn fb_envelope_extract_sig_pub_legacy(
        buf: &[u8],
    ) -> anyhow::Result<(Vec<u8>, Vec<u8>, Vec<u8>, String, String)> {
        let env: Envelope = root_as_envelope(buf)
            .map_err(|e| anyhow::anyhow!("failed to parse envelope: {}", e))?;

        let canonical = build_envelope_canonical(
            &env.payload,
            &env.payload_type,
            &env.nonce,
            env.ts,
            &env.alg,
            Some(&env.kem_pubkey),
        );

        let sig_field = env.sig.clone();
        let pub_field = env.pubkey.clone();

        let sig_b64 = sig_field
            .splitn(2, ':')
            .nth(if sig_field.contains(':') { 1 } else { 0 })
            .unwrap_or(&sig_field)
            .to_string();
        let sig_bytes = base64::engine::general_purpose::STANDARD
            .decode(&sig_b64)
            .map_err(|e| anyhow::anyhow!("failed to base64-decode signature: {}", e))?;
        let pub_bytes = base64::engine::general_purpose::STANDARD
            .decode(&pub_field)
            .map_err(|e| anyhow::anyhow!("failed to base64-decode pubkey: {}", e))?;

        Ok((canonical, sig_bytes, pub_bytes, sig_field, pub_field))
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

    pub fn build_task(
        id: &str,
        manifest_ref: &str,
        manifest_json: &str,
        requirements_cpu_cores: u32,
        requirements_memory_mb: u32,
        workload_type: &str,
        duplicate_tolerant: bool,
        max_parallel_duplicates: u32,
        placement_token: &str,
        qos_preemptible: bool,
        timestamp: u64,
    ) -> Vec<u8> {
        serialize(&Task {
            id: id.to_string(),
            manifest_ref: manifest_ref.to_string(),
            manifest_json: manifest_json.to_string(),
            requirements_cpu_cores,
            requirements_memory_mb,
            workload_type: workload_type.to_string(),
            duplicate_tolerant,
            max_parallel_duplicates,
            placement_token: placement_token.to_string(),
            qos_preemptible,
            timestamp,
        })
    }

    pub fn build_bid(
        task_id: &str,
        node_id: &str,
        score: f64,
        resource_fit_score: f64,
        network_locality_score: f64,
        timestamp: u64,
        signature: &[u8],
    ) -> Vec<u8> {
        serialize(&Bid {
            task_id: task_id.to_string(),
            node_id: node_id.to_string(),
            score,
            resource_fit_score,
            network_locality_score,
            timestamp,
            signature: signature.to_vec(),
        })
    }

    pub fn build_lease_hint(
        task_id: &str,
        node_id: &str,
        score: f64,
        ttl_ms: u32,
        renew_nonce: u64,
        timestamp: u64,
        signature: &[u8],
    ) -> Vec<u8> {
        serialize(&LeaseHint {
            task_id: task_id.to_string(),
            node_id: node_id.to_string(),
            score,
            ttl_ms,
            renew_nonce,
            timestamp,
            signature: signature.to_vec(),
        })
    }

    pub fn build_candidates_response_with_keys(
        ok: bool,
        candidates: &[CandidateNode],
    ) -> Vec<u8> {
        serialize(&CandidatesResponse {
            ok,
            candidates: candidates.to_vec(),
        })
    }

    pub fn build_candidates_response(ok: bool, responders: &[String]) -> Vec<u8> {
        let candidate_nodes = responders
            .iter()
            .map(|peer_id| CandidateNode {
                peer_id: peer_id.clone(),
                public_key: String::new(),
            })
            .collect();
        build_candidates_response_with_keys(ok, &candidate_nodes)
    }

    pub fn build_task_create_response(
        ok: bool,
        task_id: &str,
        manifest_ref: &str,
        selection_window_ms: u64,
        message: &str,
    ) -> Vec<u8> {
        serialize(&TaskCreateResponse {
            ok,
            task_id: task_id.to_string(),
            manifest_ref: manifest_ref.to_string(),
            selection_window_ms,
            message: message.to_string(),
        })
    }

    pub fn build_task_status_response(
        task_id: &str,
        state: &str,
        assigned_peers: &[String],
        manifest_cid: Option<&str>,
    ) -> Vec<u8> {
        serialize(&TaskStatusResponse {
            task_id: task_id.to_string(),
            state: state.to_string(),
            assigned_peers: assigned_peers.to_vec(),
            manifest_cid: manifest_cid.unwrap_or_default().to_string(),
        })
    }

    pub fn build_encrypted_envelope(
        payload: &[u8],
        payload_type: &str,
        nonce: &str,
        timestamp: u64,
        algorithm: &str,
        sig_prefix: &str,
        sig_b64: &str,
        pubkey_b64: &str,
        kem_pub_b64: Option<&str>,
    ) -> Vec<u8> {
        build_envelope_signed(
            payload,
            payload_type,
            nonce,
            timestamp,
            algorithm,
            sig_prefix,
            sig_b64,
            pubkey_b64,
            kem_pub_b64,
        )
    }

    pub fn build_encrypted_envelope_with_peer(
        payload: &[u8],
        payload_type: &str,
        nonce: &str,
        timestamp: u64,
        algorithm: &str,
        sig_prefix: &str,
        sig_b64: &str,
        pubkey_b64: &str,
        peer_id: &str,
        kem_pub_b64: Option<&str>,
    ) -> Vec<u8> {
        build_envelope_signed_with_peer(
            payload,
            payload_type,
            nonce,
            timestamp,
            algorithm,
            sig_prefix,
            sig_b64,
            pubkey_b64,
            peer_id,
            kem_pub_b64,
        )
    }

    pub fn extract_manifest_name(manifest_data: &[u8]) -> Option<String> {
        serde_json::from_slice::<serde_json::Value>(manifest_data)
            .ok()
            .and_then(|v| v.get("metadata"))
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
