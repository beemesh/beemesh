mod generated {
    pub mod generated_health {
        #![allow(
            dead_code,
            non_camel_case_types,
            non_snake_case,
            unused_imports,
            unused_variables,
            mismatched_lifetime_syntaxes
        )]
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/generated/health_generated.rs"));
    }

    pub mod generated_capacity_request {
        #![allow(
            dead_code,
            non_camel_case_types,
            non_snake_case,
            unused_imports,
            unused_variables,
            mismatched_lifetime_syntaxes
        )]
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/generated/capacity_request_generated.rs"));
    }

    pub mod generated_capacity_reply {
        #![allow(
            dead_code,
            non_camel_case_types,
            non_snake_case,
            unused_imports,
            unused_variables,
            mismatched_lifetime_syntaxes
        )]
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/generated/capacity_reply_generated.rs"));
    }

    pub mod generated_apply_request {
        #![allow(
            dead_code,
            non_camel_case_types,
            non_snake_case,
            unused_imports,
            unused_variables,
            mismatched_lifetime_syntaxes
        )]
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/generated/apply_request_generated.rs"));
    }

    pub mod generated_apply_response {
        #![allow(
            dead_code,
            non_camel_case_types,
            non_snake_case,
            unused_imports,
            unused_variables,
            mismatched_lifetime_syntaxes
        )]
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/generated/apply_response_generated.rs"));
    }

    pub mod generated_keyshare_response {
        #![allow(
            dead_code,
            non_camel_case_types,
            non_snake_case,
            unused_imports,
            unused_variables,
            mismatched_lifetime_syntaxes
        )]
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/generated/keyshare_response_generated.rs"));
    }

    pub mod generated_handshake {
        #![allow(
            dead_code,
            non_camel_case_types,
            non_snake_case,
            unused_imports,
            unused_variables,
            mismatched_lifetime_syntaxes
        )]
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/generated/handshake_generated.rs"));
    }

    pub mod generated_envelope {
        #![allow(
            dead_code,
            non_camel_case_types,
            non_snake_case,
            unused_imports,
            unused_variables,
            mismatched_lifetime_syntaxes
        )]
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/generated/envelope_generated.rs"));
    }
    pub mod generated_keyshare_request {
        #![allow(
            dead_code,
            non_camel_case_types,
            non_snake_case,
            unused_imports,
            unused_variables,
            mismatched_lifetime_syntaxes
        )]
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/generated/keyshare_request_generated.rs"));
    }
    pub mod generated_capability_token {
        #![allow(
            dead_code,
            non_camel_case_types,
            non_snake_case,
            unused_imports,
            unused_variables,
            mismatched_lifetime_syntaxes
        )]
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/generated/capability_token_generated.rs"));
    }

    pub mod generated_distribute_shares_request {
        #![allow(
            dead_code,
            non_camel_case_types,
            non_snake_case,
            unused_imports,
            unused_variables,
            mismatched_lifetime_syntaxes
        )]
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/generated/distribute_shares_request_generated.rs"));
    }

    pub mod generated_distribute_capabilities_request {
        #![allow(
            dead_code,
            non_camel_case_types,
            non_snake_case,
            unused_imports,
            unused_variables,
            mismatched_lifetime_syntaxes
        )]
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/generated/distribute_capabilities_request_generated.rs"));
    }

    pub mod generated_assign_request {
        #![allow(
            dead_code,
            non_camel_case_types,
            non_snake_case,
            unused_imports,
            unused_variables,
            mismatched_lifetime_syntaxes
        )]
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/generated/assign_request_generated.rs"));
    }
}

pub mod machine {
    // Avoid glob imports; re-export specific items below.
    pub use crate::generated::generated_health::beemesh::machine::{Health, root_as_health};
    pub use crate::generated::generated_capacity_reply::beemesh::machine::{ CapacityReply, root_as_capacity_reply, finish_capacity_reply_buffer };
    pub use crate::generated::generated_capacity_request::beemesh::machine::{ CapacityRequest, root_as_capacity_request, finish_capacity_request_buffer };
    // Re-export Args to allow building nested FB objects in other modules
    pub use crate::generated::generated_capacity_request::beemesh::machine::CapacityRequestArgs;
    pub use crate::generated::generated_capacity_reply::beemesh::machine::CapacityReplyArgs;
    pub use crate::generated::generated_apply_request::beemesh::machine::{ ApplyRequest, root_as_apply_request };
    pub use crate::generated::generated_apply_response::beemesh::machine::{ ApplyResponse, root_as_apply_response };
    pub use crate::generated::generated_handshake::beemesh::machine::{ Handshake, root_as_handshake };
    pub use crate::generated::generated_envelope::beemesh::machine::{ Envelope as FbEnvelope, root_as_envelope };
    pub use crate::generated::generated_keyshare_response::beemesh::machine::{ KeyShareResponse, root_as_key_share_response };
    pub use crate::generated::generated_keyshare_request::beemesh::machine::{ KeyShareRequest, root_as_key_share_request };
    // Also export Args and helper finish function for builders/tests
    pub use crate::generated::generated_envelope::beemesh::machine::{ EnvelopeArgs, finish_envelope_buffer };

    // CapabilityToken for key share authorization (generated)
    #[allow(
        dead_code,
        non_camel_case_types,
        non_snake_case,
        unused_imports,
        unused_variables,
        mismatched_lifetime_syntaxes,
    )]
    pub mod generated_capability_token {
        pub use crate::generated::generated_capability_token::beemesh::machine::*;
    }

    pub use crate::machine::generated_capability_token::CapabilityToken;
    pub use crate::machine::generated_capability_token::CapabilityTokenArgs;
    pub use crate::machine::generated_capability_token::Capability;
    pub use crate::machine::generated_capability_token::CapabilityArgs;
    pub use crate::machine::generated_capability_token::Caveat;
    pub use crate::machine::generated_capability_token::CaveatArgs;
    pub use crate::machine::generated_capability_token::SignatureEntry;
    pub use crate::machine::generated_capability_token::SignatureEntryArgs;
    pub use crate::machine::generated_capability_token::root_as_capability_token;
    
    // Distribute shares types  
    pub use crate::generated::generated_distribute_shares_request::beemesh::machine::{
        DistributeSharesRequest, DistributeSharesRequestArgs, ShareTarget, ShareTargetArgs, 
        root_as_distribute_shares_request
    };
    
    // Distribute capabilities types
    pub use crate::generated::generated_distribute_capabilities_request::beemesh::machine::{
        DistributeCapabilitiesRequest, DistributeCapabilitiesRequestArgs, 
        CapabilityTarget, CapabilityTargetArgs, root_as_distribute_capabilities_request  
    };
    
    // Assign request types
    pub use crate::generated::generated_assign_request::beemesh::machine::{
        AssignRequest, AssignRequestArgs, root_as_assign_request
    };
    
    // AppliedManifest for DHT storage (generated). Wrap include in a module with
    // liberal allow attributes so generated code doesn't emit warnings.
    #[allow(
        dead_code,
        non_camel_case_types,
        non_snake_case,
        unused_imports,
        unused_variables,
        mismatched_lifetime_syntaxes,
    )]
    pub mod generated_applied_manifest {
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/generated/applied_manifest_generated.rs"));
    }

    pub use crate::machine::generated_applied_manifest::beemesh::machine::{
        AppliedManifest,
        root_as_applied_manifest,
        AppliedManifestArgs,
        SignatureScheme,
        OperationType,
        KeyValue,
        KeyValueArgs,
    };

    use flatbuffers::FlatBufferBuilder;
    use base64::Engine as _;

    // Manual builder functions for commonly used flatbuffers
    pub fn build_health(ok: bool, status: &str) -> Vec<u8> {
        let mut fbb = FlatBufferBuilder::with_capacity(128);
        let status_off = fbb.create_string(status);
        let mut args: crate::generated::generated_health::beemesh::machine::HealthArgs = Default::default();
        args.ok = ok;
        args.status = Some(status_off);
        let off = crate::generated::generated_health::beemesh::machine::Health::create(&mut fbb, &args);
        fbb.finish(off, None);
        fbb.finished_data().to_vec()
    }

    pub fn build_capacity_request(cpu_milli: u32, memory_bytes: u64, storage_bytes: u64, replicas: u32) -> Vec<u8> {
        let mut fbb = FlatBufferBuilder::with_capacity(128);
        let mut args: crate::generated::generated_capacity_request::beemesh::machine::CapacityRequestArgs = Default::default();
        args.cpu_milli = cpu_milli;
        args.memory_bytes = memory_bytes;
        args.storage_bytes = storage_bytes;
        args.replicas = replicas;
        let off = crate::generated::generated_capacity_request::beemesh::machine::CapacityRequest::create(&mut fbb, &args);
        fbb.finish(off, None);
        fbb.finished_data().to_vec()
    }

    pub fn build_capacity_reply(ok: bool, cpu_available_milli: u32, memory_available_bytes: u64, storage_available_bytes: u64, request_id: &str, node_id: &str, region: &str, kem_pubkey: Option<&str>, capabilities: &[&str]) -> Vec<u8> {
        let mut fbb = FlatBufferBuilder::with_capacity(256);
        let req_off = fbb.create_string(request_id);
        let node_off = fbb.create_string(node_id);
        let region_off = fbb.create_string(region);
        let kem_off = kem_pubkey.map(|s| fbb.create_string(s));
        let mut caps_vec: Vec<flatbuffers::WIPOffset<&str>> = Vec::with_capacity(capabilities.len());
        for &c in capabilities.iter() { caps_vec.push(fbb.create_string(c)); }
        let caps_off = fbb.create_vector(&caps_vec);
        let mut args: crate::generated::generated_capacity_reply::beemesh::machine::CapacityReplyArgs = Default::default();
        args.ok = ok;
        args.cpu_available_milli = cpu_available_milli;
        args.memory_available_bytes = memory_available_bytes;
        args.storage_available_bytes = storage_available_bytes;
        args.request_id = Some(req_off);
        args.node_id = Some(node_off);
        args.region = Some(region_off);
        if let Some(k) = kem_off { args.kem_pubkey = Some(k); }
        args.capabilities = Some(caps_off);
        let off = crate::generated::generated_capacity_reply::beemesh::machine::CapacityReply::create(&mut fbb, &args);
        fbb.finish(off, None);
        fbb.finished_data().to_vec()
    }

    pub fn build_apply_request(replicas: u32, tenant: &str, operation_id: &str, manifest_json: &str, origin_peer: &str) -> Vec<u8> {
        let mut fbb = FlatBufferBuilder::with_capacity(256);
        let tenant_off = fbb.create_string(tenant);
        let op_off = fbb.create_string(operation_id);
        let manifest_off = fbb.create_string(manifest_json);
        let origin_off = fbb.create_string(origin_peer);
        let mut args: crate::generated::generated_apply_request::beemesh::machine::ApplyRequestArgs = Default::default();
        args.replicas = replicas;
        args.tenant = Some(tenant_off);
        args.operation_id = Some(op_off);
        args.manifest_json = Some(manifest_off);
        args.origin_peer = Some(origin_off);
        let off = crate::generated::generated_apply_request::beemesh::machine::ApplyRequest::create(&mut fbb, &args);
        fbb.finish(off, None);
        fbb.finished_data().to_vec()
    }

    pub fn build_apply_response(ok: bool, operation_id: &str, message: &str) -> Vec<u8> {
        let mut fbb = FlatBufferBuilder::with_capacity(128);
        let op_off = fbb.create_string(operation_id);
        let msg_off = fbb.create_string(message);
        let mut args: crate::generated::generated_apply_response::beemesh::machine::ApplyResponseArgs = Default::default();
        args.ok = ok;
        args.operation_id = Some(op_off);
        args.message = Some(msg_off);
        let off = crate::generated::generated_apply_response::beemesh::machine::ApplyResponse::create(&mut fbb, &args);
        fbb.finish(off, None);
        fbb.finished_data().to_vec()
    }

    pub fn build_keyshare_response(ok: bool, operation_id: &str, message: &str) -> Vec<u8> {
        let mut fbb = FlatBufferBuilder::with_capacity(128);
        let op_off = fbb.create_string(operation_id);
        let msg_off = fbb.create_string(message);
        let mut args: crate::generated::generated_keyshare_response::beemesh::machine::KeyShareResponseArgs = Default::default();
        args.ok = ok;
        args.operation_id = Some(op_off);
        args.message = Some(msg_off);
        let off = crate::generated::generated_keyshare_response::beemesh::machine::KeyShareResponse::create(&mut fbb, &args);
        fbb.finish(off, None);
        fbb.finished_data().to_vec()
    }

    // KeyShareRequest builder (manifest_id, capability)
    pub fn build_keyshare_request(manifest_id: &str, capability: &str) -> Vec<u8> {
        let mut fbb = FlatBufferBuilder::with_capacity(128);
        let mid = fbb.create_string(manifest_id);
        let cap = fbb.create_string(capability);
        let mut args: crate::generated::generated_keyshare_request::beemesh::machine::KeyShareRequestArgs = Default::default();
        args.manifest_id = Some(mid);
        args.capability = Some(cap);
        let off = crate::generated::generated_keyshare_request::beemesh::machine::KeyShareRequest::create(&mut fbb, &args);
        fbb.finish(off, None);
        fbb.finished_data().to_vec()
    }

    // Helpers for Envelope flatbuffer canonicalization and building.
    pub fn build_envelope_canonical(
        payload: &[u8],
        payload_type: &str,
        nonce: &str,
        ts: u64,
        alg: &str,
    ) -> Vec<u8> {
        // Build an Envelope with empty sig/pubkey (canonical bytes to sign)
        let mut fbb = FlatBufferBuilder::with_capacity(256);
        let payload_vec = fbb.create_vector(payload);
        let payload_type_off = fbb.create_string(payload_type);
        let nonce_off = fbb.create_string(nonce);
        let alg_off = fbb.create_string(alg);
        let sig_off = fbb.create_string("");
        let pubkey_off = fbb.create_string("");
        let peer_id_off = fbb.create_string("");

        let mut args = crate::generated::generated_envelope::beemesh::machine::EnvelopeArgs::default();
        args.payload = Some(payload_vec);
        args.payload_type = Some(payload_type_off);
        args.nonce = Some(nonce_off);
        args.ts = ts;
        args.alg = Some(alg_off);
        args.sig = Some(sig_off);
        args.pubkey = Some(pubkey_off);
        args.peer_id = Some(peer_id_off);

        let env_off = crate::generated::generated_envelope::beemesh::machine::Envelope::create(&mut fbb, &args);
        crate::machine::finish_envelope_buffer(&mut fbb, env_off);
        fbb.finished_data().to_vec()
    }

    pub fn build_envelope_signed(
        payload: &[u8],
        payload_type: &str,
        nonce: &str,
        ts: u64,
        alg: &str,
        sig_prefix: &str,
        sig_b64: &str,
        pubkey_b64: &str,
    ) -> Vec<u8> {
        let mut fbb = FlatBufferBuilder::with_capacity(256);
        let payload_vec = fbb.create_vector(payload);
        let payload_type_off = fbb.create_string(payload_type);
        let nonce_off = fbb.create_string(nonce);
        let alg_off = fbb.create_string(alg);
        let sig_full = fbb.create_string(&format!("{}:{}", sig_prefix, sig_b64));
        let pubkey_off = fbb.create_string(pubkey_b64);
        let peer_id_off = fbb.create_string("");

        let mut args = crate::generated::generated_envelope::beemesh::machine::EnvelopeArgs::default();
        args.payload = Some(payload_vec);
        args.payload_type = Some(payload_type_off);
        args.nonce = Some(nonce_off);
        args.ts = ts;
        args.alg = Some(alg_off);
        args.sig = Some(sig_full);
        args.pubkey = Some(pubkey_off);
        args.peer_id = Some(peer_id_off);

        let env_off = crate::generated::generated_envelope::beemesh::machine::Envelope::create(&mut fbb, &args);
        crate::machine::finish_envelope_buffer(&mut fbb, env_off);
        fbb.finished_data().to_vec()
    }

    /// Parse a FlatBuffer Envelope (raw bytes) and return the canonical bytes used for
    /// signature verification together with the decoded signature and pubkey bytes and
    /// the original sig/pub strings. This centralizes the prefix parsing logic (e.g.
    /// handling "ml-dsa-65:BASE64") so callers can use a single trusted implementation.
    pub fn fb_envelope_extract_sig_pub(buf: &[u8]) -> anyhow::Result<(Vec<u8>, Vec<u8>, Vec<u8>, String, String)> {
        let fb_env = crate::machine::root_as_envelope(buf).map_err(|e| anyhow::anyhow!("failed to parse envelope flatbuffer: {:?}", e))?;

        let payload_vec = fb_env.payload().map(|b| b.iter().collect::<Vec<u8>>()).unwrap_or_default();
        let payload_type = fb_env.payload_type().unwrap_or("");
        let nonce = fb_env.nonce().unwrap_or("");
        let ts = fb_env.ts();
        let alg = fb_env.alg().unwrap_or("");

        let canonical = crate::machine::build_envelope_canonical(&payload_vec, payload_type, nonce, ts, alg);

        let sig_field = fb_env.sig().unwrap_or("").to_string();
        let pubkey_field = fb_env.pubkey().unwrap_or("").to_string();

        // Extract base64 portion of signature (after possible prefix)
        let sig_b64 = sig_field.splitn(2, ':').nth(if sig_field.contains(':') {1} else {0}).unwrap_or(&sig_field);
        let sig_bytes = base64::engine::general_purpose::STANDARD.decode(sig_b64)
            .map_err(|e| anyhow::anyhow!("failed to base64-decode signature: {}", e))?;

        let pub_bytes = base64::engine::general_purpose::STANDARD.decode(&pubkey_field)
            .map_err(|e| anyhow::anyhow!("failed to base64-decode pubkey: {}", e))?;

        Ok((canonical, sig_bytes, pub_bytes, sig_field, pubkey_field))
    }

    // Custom handshake builder since it only has string fields
    pub fn build_handshake(nonce: u32, timestamp: u64, protocol_version: &str, signature: &str) -> Vec<u8> {
        let mut fbb = FlatBufferBuilder::with_capacity(128);
        let proto = fbb.create_string(protocol_version);
        let sig = fbb.create_string(signature);
        let mut args: crate::generated::generated_handshake::beemesh::machine::HandshakeArgs = Default::default();
        args.nonce = nonce;
        args.timestamp = timestamp;
        args.protocol_version = Some(proto);
        args.signature = Some(sig);
        let off = crate::generated::generated_handshake::beemesh::machine::Handshake::create(&mut fbb, &args);
        fbb.finish(off, None);
        fbb.finished_data().to_vec()
    }

    // Custom builder for AppliedManifest with byte vectors and custom types
    pub fn build_applied_manifest(
        id: &str,
        tenant: &str,
        operation_id: &str,
        origin_peer: &str,
        owner_pubkey: &[u8],
        signature_scheme: SignatureScheme,
        signature: &[u8],
        manifest_json: &str,
        manifest_kind: &str,
        labels: Vec<(String, String)>,
        timestamp: u64,
        operation: OperationType,
        ttl_secs: u32,
        content_hash: &str,
    ) -> Vec<u8> {
        let mut fbb = FlatBufferBuilder::with_capacity(1024);
        
        // Create string offsets
        let id_offset = fbb.create_string(id);
        let tenant_offset = fbb.create_string(tenant);
        let operation_id_offset = fbb.create_string(operation_id);
        let origin_peer_offset = fbb.create_string(origin_peer);
        let manifest_json_offset = fbb.create_string(manifest_json);
        let manifest_kind_offset = fbb.create_string(manifest_kind);
        let content_hash_offset = fbb.create_string(content_hash);
        
        // Create byte vectors
        let owner_pubkey_offset = fbb.create_vector(owner_pubkey);
        let signature_offset = fbb.create_vector(signature);
        
        // Create labels vector
        let label_offsets: Vec<_> = labels.iter().map(|(k, v)| {
            let key_offset = fbb.create_string(k);
            let value_offset = fbb.create_string(v);
            KeyValue::create(&mut fbb, &KeyValueArgs {
                key: Some(key_offset),
                value: Some(value_offset),
            })
        }).collect();
        let labels_offset = fbb.create_vector(&label_offsets);
        
        let args = AppliedManifestArgs {
            id: Some(id_offset),
            tenant: Some(tenant_offset),
            operation_id: Some(operation_id_offset),
            origin_peer: Some(origin_peer_offset),
            owner_pubkey: Some(owner_pubkey_offset),
            signature_scheme,
            signature: Some(signature_offset),
            manifest_json: Some(manifest_json_offset),
            manifest_kind: Some(manifest_kind_offset),
            labels: Some(labels_offset),
            timestamp,
            operation,
            ttl_secs,
            content_hash: Some(content_hash_offset),
        };
        
        let manifest = AppliedManifest::create(&mut fbb, &args);
        fbb.finish(manifest, None);
        fbb.finished_data().to_vec()
    }

    // Builder function for DistributeSharesRequest
    pub fn build_distribute_shares_request(
        shares_envelope_json: &str,
        targets: &[(String, String)], // (peer_id, payload_json)
    ) -> Vec<u8> {
        let mut fbb = FlatBufferBuilder::with_capacity(1024);
        
        let shares_env_offset = fbb.create_string(shares_envelope_json);
        
        // Create ShareTarget vector
        let target_offsets: Vec<_> = targets.iter().map(|(peer_id, payload_json)| {
            let peer_id_offset = fbb.create_string(peer_id);
            let payload_offset = fbb.create_string(payload_json);
            ShareTarget::create(&mut fbb, &ShareTargetArgs {
                peer_id: Some(peer_id_offset),
                payload_json: Some(payload_offset),
            })
        }).collect();
        let targets_offset = fbb.create_vector(&target_offsets);
        
        let args = DistributeSharesRequestArgs {
            shares_envelope_json: Some(shares_env_offset),
            targets: Some(targets_offset),
        };
        
        let request = DistributeSharesRequest::create(&mut fbb, &args);
        fbb.finish(request, None);
        fbb.finished_data().to_vec()
    }

    // Builder function for DistributeCapabilitiesRequest  
    pub fn build_distribute_capabilities_request(
        targets: &[(String, String)], // (peer_id, payload_json)
    ) -> Vec<u8> {
        let mut fbb = FlatBufferBuilder::with_capacity(1024);
        
        // Create CapabilityTarget vector
        let target_offsets: Vec<_> = targets.iter().map(|(peer_id, payload_json)| {
            let peer_id_offset = fbb.create_string(peer_id);
            let payload_offset = fbb.create_string(payload_json);
            CapabilityTarget::create(&mut fbb, &CapabilityTargetArgs {
                peer_id: Some(peer_id_offset),
                payload_json: Some(payload_offset),
            })
        }).collect();
        let targets_offset = fbb.create_vector(&target_offsets);
        
        let args = DistributeCapabilitiesRequestArgs {
            targets: Some(targets_offset),
        };
        
        let request = DistributeCapabilitiesRequest::create(&mut fbb, &args);
        fbb.finish(request, None);
        fbb.finished_data().to_vec()
    }

    // Builder function for AssignRequest
    pub fn build_assign_request(chosen_peers: &[String]) -> Vec<u8> {
        let mut fbb = FlatBufferBuilder::with_capacity(512);
        
        // Create vector of peer ID strings
        let peer_offsets: Vec<_> = chosen_peers.iter()
            .map(|peer_id| fbb.create_string(peer_id))
            .collect();
        let peers_offset = fbb.create_vector(&peer_offsets);
        
        let args = AssignRequestArgs {
            chosen_peers: Some(peers_offset),
        };
        
        let request = AssignRequest::create(&mut fbb, &args);
        fbb.finish(request, None);
        fbb.finished_data().to_vec()
    }
}

pub mod libp2p_constants;
pub mod json;

#[cfg(test)]
mod test {
    use crate::machine::{ build_health, root_as_health };

    #[test]
    fn flatbuffers_health_roundtrip() {
        let buf = build_health(true, "healthy");
        // parse and verify
        let health = root_as_health(&buf).unwrap();
        assert!(health.ok());
        assert_eq!(health.status().unwrap(), "healthy");
    }
}