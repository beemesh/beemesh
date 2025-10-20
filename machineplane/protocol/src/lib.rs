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
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/generated/health_generated.rs"
        ));
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
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/generated/capacity_request_generated.rs"
        ));
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
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/generated/capacity_reply_generated.rs"
        ));
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
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/generated/apply_request_generated.rs"
        ));
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
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/generated/apply_response_generated.rs"
        ));
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
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/generated/handshake_generated.rs"
        ));
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
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/generated/envelope_generated.rs"
        ));
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
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/generated/assign_request_generated.rs"
        ));
    }

    pub mod generated_nodes_response {
        #![allow(
            dead_code,
            non_camel_case_types,
            non_snake_case,
            unused_imports,
            unused_variables,
            mismatched_lifetime_syntaxes
        )]
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/generated/nodes_response_generated.rs"
        ));
    }

    pub mod generated_candidates_response {
        #![allow(
            dead_code,
            non_camel_case_types,
            non_snake_case,
            unused_imports,
            unused_variables,
            mismatched_lifetime_syntaxes
        )]
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/generated/candidates_response_generated.rs"
        ));
    }

    pub mod generated_task_create_response {
        #![allow(
            dead_code,
            non_camel_case_types,
            non_snake_case,
            unused_imports,
            unused_variables,
            mismatched_lifetime_syntaxes
        )]
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/generated/task_create_response_generated.rs"
        ));
    }

    pub mod generated_task_status_response {
        #![allow(
            dead_code,
            non_camel_case_types,
            non_snake_case,
            unused_imports,
            unused_variables,
            mismatched_lifetime_syntaxes
        )]
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/generated/task_status_response_generated.rs"
        ));
    }

    pub mod generated_assign_response {
        #![allow(
            dead_code,
            non_camel_case_types,
            non_snake_case,
            unused_imports,
            unused_variables,
            mismatched_lifetime_syntaxes
        )]
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/generated/assign_response_generated.rs"
        ));
    }

    pub mod generated_apply_manifest_request {
        #![allow(
            dead_code,
            non_camel_case_types,
            non_snake_case,
            unused_imports,
            unused_variables,
            mismatched_lifetime_syntaxes
        )]
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/generated/apply_manifest_request_generated.rs"
        ));
    }

    pub mod generated_apply_manifest_response {
        #![allow(
            dead_code,
            non_camel_case_types,
            non_snake_case,
            unused_imports,
            unused_variables,
            mismatched_lifetime_syntaxes
        )]
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/generated/apply_manifest_response_generated.rs"
        ));
    }

    pub mod generated_applied_manifest {
        #![allow(
            dead_code,
            non_camel_case_types,
            non_snake_case,
            unused_imports,
            unused_variables,
            mismatched_lifetime_syntaxes
        )]
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/generated/applied_manifest_generated.rs"
        ));
    }
}

pub mod machine {
    // Avoid glob imports; re-export specific items below.
    pub use crate::generated::generated_capacity_reply::beemesh::machine::{
        finish_capacity_reply_buffer, root_as_capacity_reply, CapacityReply,
    };
    pub use crate::generated::generated_capacity_request::beemesh::machine::{
        finish_capacity_request_buffer, root_as_capacity_request, CapacityRequest,
    };
    pub use crate::generated::generated_health::beemesh::machine::{root_as_health, Health};
    // Re-export Args to allow building nested FB objects in other modules
    pub use crate::generated::generated_apply_manifest_request::beemesh::machine::root_as_apply_manifest_request;
    pub use crate::generated::generated_apply_request::beemesh::machine::{
        root_as_apply_request, ApplyRequest,
    };
    pub use crate::generated::generated_apply_response::beemesh::machine::{
        root_as_apply_response, ApplyResponse,
    };
    pub use crate::generated::generated_assign_request::beemesh::machine::root_as_assign_request;
    pub use crate::generated::generated_assign_response::beemesh::machine::root_as_assign_response;
    pub use crate::generated::generated_candidates_response::beemesh::machine::root_as_candidates_response;
    pub use crate::generated::generated_capacity_reply::beemesh::machine::CapacityReplyArgs;
    pub use crate::generated::generated_capacity_request::beemesh::machine::CapacityRequestArgs;
    pub use crate::generated::generated_envelope::beemesh::machine::{
        root_as_envelope, Envelope as FbEnvelope,
    };
    pub use crate::generated::generated_handshake::beemesh::machine::{
        root_as_handshake, Handshake,
    };
    pub use crate::generated::generated_nodes_response::beemesh::machine::root_as_nodes_response;
    pub use crate::generated::generated_task_create_response::beemesh::machine::root_as_task_create_response;
    // Also export Args and helper finish function for builders/tests
    pub use crate::generated::generated_envelope::beemesh::machine::{
        finish_envelope_buffer, EnvelopeArgs,
    };

    // Assign types
    pub use crate::generated::generated_assign_request::beemesh::machine::{
        AssignRequest, AssignRequestArgs,
    };
    pub use crate::generated::generated_assign_response::beemesh::machine::{
        AssignResponse, AssignResponseArgs,
    };

    // Nodes response
    pub use crate::generated::generated_nodes_response::beemesh::machine::{
        NodesResponse, NodesResponseArgs,
    };

    // Candidates response
    pub use crate::generated::generated_candidates_response::beemesh::machine::{
        CandidateNode, CandidateNodeArgs, CandidatesResponse, CandidatesResponseArgs,
    };

    // Task create response
    pub use crate::generated::generated_task_create_response::beemesh::machine::{
        TaskCreateResponse, TaskCreateResponseArgs,
    };

    // Task status response
    pub use crate::generated::generated_task_status_response::beemesh::machine::{
        TaskStatusResponse, TaskStatusResponseArgs,
    };

    // Apply manifest types
    pub use crate::generated::generated_apply_manifest_request::beemesh::machine::{
        ApplyManifestRequest, ApplyManifestRequestArgs,
    };
    pub use crate::generated::generated_apply_manifest_response::beemesh::machine::{
        ApplyManifestResponse, ApplyManifestResponseArgs,
    };

    pub mod generated_applied_manifest {
        pub use crate::generated::generated_applied_manifest::beemesh::machine::*;
    }

    pub use crate::machine::generated_applied_manifest::{
        root_as_applied_manifest, AppliedManifest, AppliedManifestArgs, OperationType,
        SignatureScheme,
    };

    use base64::Engine;
    use flatbuffers::FlatBufferBuilder;

    // Health
    pub fn build_health(ok: bool, status: &str) -> Vec<u8> {
        let mut fbb = FlatBufferBuilder::with_capacity(128);
        let status_off = fbb.create_string(status);
        let mut args: crate::generated::generated_health::beemesh::machine::HealthArgs =
            Default::default();
        args.ok = ok;
        args.status = Some(status_off);
        let off =
            crate::generated::generated_health::beemesh::machine::Health::create(&mut fbb, &args);
        fbb.finish(off, None);
        fbb.finished_data().to_vec()
    }

    pub fn build_capacity_request(
        cpu_milli: u32,
        memory_bytes: u64,
        storage_bytes: u64,
        replicas: u32,
    ) -> Vec<u8> {
        let mut fbb = FlatBufferBuilder::with_capacity(128);
        let mut args: crate::generated::generated_capacity_request::beemesh::machine::CapacityRequestArgs = Default::default();
        args.cpu_milli = cpu_milli;
        args.memory_bytes = memory_bytes;
        args.storage_bytes = storage_bytes;
        args.replicas = replicas;
        let off =
            crate::generated::generated_capacity_request::beemesh::machine::CapacityRequest::create(
                &mut fbb, &args,
            );
        fbb.finish(off, None);
        fbb.finished_data().to_vec()
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
        let mut fbb = FlatBufferBuilder::with_capacity(256);
        let req_off = fbb.create_string(request_id);
        let node_off = fbb.create_string(node_id);
        let region_off = fbb.create_string(region);
        let kem_off = kem_pubkey.map(|s| fbb.create_string(s));
        let mut caps_vec: Vec<flatbuffers::WIPOffset<&str>> =
            Vec::with_capacity(capabilities.len());
        for &c in capabilities.iter() {
            caps_vec.push(fbb.create_string(c));
        }
        let caps_off = fbb.create_vector(&caps_vec);
        let mut args: crate::generated::generated_capacity_reply::beemesh::machine::CapacityReplyArgs = Default::default();
        args.ok = ok;
        args.cpu_available_milli = cpu_available_milli;
        args.memory_available_bytes = memory_available_bytes;
        args.storage_available_bytes = storage_available_bytes;
        args.request_id = Some(req_off);
        args.node_id = Some(node_off);
        args.region = Some(region_off);
        if let Some(k) = kem_off {
            args.kem_pubkey = Some(k);
        }
        args.capabilities = Some(caps_off);
        let off =
            crate::generated::generated_capacity_reply::beemesh::machine::CapacityReply::create(
                &mut fbb, &args,
            );
        fbb.finish(off, None);
        fbb.finished_data().to_vec()
    }

    pub fn build_apply_request(
        replicas: u32,
        tenant: &str,
        operation_id: &str,
        manifest_json: &str,
        origin_peer: &str,
    ) -> Vec<u8> {
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
        let off = crate::generated::generated_apply_request::beemesh::machine::ApplyRequest::create(
            &mut fbb, &args,
        );
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
        let off =
            crate::generated::generated_apply_response::beemesh::machine::ApplyResponse::create(
                &mut fbb, &args,
            );
        fbb.finish(off, None);
        fbb.finished_data().to_vec()
    }

    // Utility functions for envelope creation

    /// Create an envelope with canonical signature using ml-dsa-65
    pub fn build_envelope_canonical(
        payload: &[u8],
        payload_type: &str,
        nonce: &str,
        ts: u64,
        alg: &str,
        kem_pub: Option<&str>,
    ) -> Vec<u8> {
        // Build an Envelope with empty sig/pubkey (canonical bytes to sign)
        let mut fbb = FlatBufferBuilder::with_capacity(256);
        let payload_vec = fbb.create_vector(payload);
        let payload_type_off = fbb.create_string(payload_type);
        let nonce_off = fbb.create_string(nonce);
        let alg_off = fbb.create_string(alg);
        let sig_off = fbb.create_string("");
        let pubkey_off = fbb.create_string("");
        // kem_pub is optional; use empty string when not provided so canonicalization
        // remains stable.
        let kem_pub_off = fbb.create_string(kem_pub.unwrap_or(""));
        let peer_id_off = fbb.create_string("");

        let mut args =
            crate::generated::generated_envelope::beemesh::machine::EnvelopeArgs::default();
        args.payload = Some(payload_vec);
        args.payload_type = Some(payload_type_off);
        args.nonce = Some(nonce_off);
        args.ts = ts;
        args.alg = Some(alg_off);
        args.sig = Some(sig_off);
        args.pubkey = Some(pubkey_off);
        args.kem_pubkey = Some(kem_pub_off);
        args.peer_id = Some(peer_id_off);

        let env_off = crate::generated::generated_envelope::beemesh::machine::Envelope::create(
            &mut fbb, &args,
        );
        fbb.finish(env_off, None);
        fbb.finished_data().to_vec()
    }

    /// Create an envelope with signed payload using ml-dsa-65
    pub fn build_envelope_signed(
        payload: &[u8],
        payload_type: &str,
        nonce: &str,
        ts: u64,
        alg: &str,
        sig_prefix: &str,
        sig_b64: &str,
        pubkey_b64: &str,
        kem_pub_b64: Option<&str>,
    ) -> Vec<u8> {
        let mut fbb = FlatBufferBuilder::with_capacity(256);
        let payload_vec = fbb.create_vector(payload);
        let payload_type_off = fbb.create_string(payload_type);
        let nonce_off = fbb.create_string(nonce);
        let alg_off = fbb.create_string(alg);
        let sig_full = fbb.create_string(&format!("{}:{}", sig_prefix, sig_b64));
        let pubkey_off = fbb.create_string(pubkey_b64);
        // kem_pub optional - embed base64 string (or empty) in envelope
        let kem_pub_off = fbb.create_string(kem_pub_b64.unwrap_or(""));
        let peer_id_off = fbb.create_string("");

        let mut args =
            crate::generated::generated_envelope::beemesh::machine::EnvelopeArgs::default();
        args.payload = Some(payload_vec);
        args.payload_type = Some(payload_type_off);
        args.nonce = Some(nonce_off);
        args.ts = ts;
        args.alg = Some(alg_off);
        args.sig = Some(sig_full);
        args.pubkey = Some(pubkey_off);
        args.kem_pubkey = Some(kem_pub_off);
        args.peer_id = Some(peer_id_off);

        let env_off = crate::generated::generated_envelope::beemesh::machine::Envelope::create(
            &mut fbb, &args,
        );
        fbb.finish(env_off, None);
        fbb.finished_data().to_vec()
    }

    /// Create an envelope with canonical signature using ml-dsa-65, including peer_id
    pub fn build_envelope_canonical_with_peer(
        payload: &[u8],
        payload_type: &str,
        nonce: &str,
        timestamp: u64,
        algorithm: &str,
        peer_id: &str,
        kem_pub: Option<&str>,
    ) -> Vec<u8> {
        // Build an Envelope with empty sig/pubkey (canonical bytes to sign)
        let mut fbb = FlatBufferBuilder::with_capacity(256);
        let payload_vec = fbb.create_vector(payload);
        let payload_type_off = fbb.create_string(payload_type);
        let nonce_off = fbb.create_string(nonce);
        let alg_off = fbb.create_string(algorithm);
        let sig_off = fbb.create_string("");
        let pubkey_off = fbb.create_string("");
        let kem_pub_off = fbb.create_string(kem_pub.unwrap_or(""));
        let peer_id_off = fbb.create_string(peer_id);

        let mut args =
            crate::generated::generated_envelope::beemesh::machine::EnvelopeArgs::default();
        args.payload = Some(payload_vec);
        args.payload_type = Some(payload_type_off);
        args.nonce = Some(nonce_off);
        args.ts = timestamp;
        args.alg = Some(alg_off);
        args.sig = Some(sig_off);
        args.pubkey = Some(pubkey_off);
        args.kem_pubkey = Some(kem_pub_off);
        args.peer_id = Some(peer_id_off);

        let env_off = crate::generated::generated_envelope::beemesh::machine::Envelope::create(
            &mut fbb, &args,
        );
        fbb.finish(env_off, None);
        fbb.finished_data().to_vec()
    }

    /// Create an envelope with signed payload using ml-dsa-65, including peer_id
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
        let mut fbb = FlatBufferBuilder::with_capacity(256);
        let payload_vec = fbb.create_vector(payload);
        let payload_type_off = fbb.create_string(payload_type);
        let nonce_off = fbb.create_string(nonce);
        let alg_off = fbb.create_string(algorithm);
        let sig_full = fbb.create_string(&format!("{}:{}", sig_prefix, sig_b64));
        let pubkey_off = fbb.create_string(pubkey_b64);
        let kem_pub_off = fbb.create_string(kem_pub_b64.unwrap_or(""));
        let peer_id_off = fbb.create_string(peer_id);

        let mut args =
            crate::generated::generated_envelope::beemesh::machine::EnvelopeArgs::default();
        args.payload = Some(payload_vec);
        args.payload_type = Some(payload_type_off);
        args.nonce = Some(nonce_off);
        args.ts = timestamp;
        args.alg = Some(alg_off);
        args.sig = Some(sig_full);
        args.pubkey = Some(pubkey_off);
        args.kem_pubkey = Some(kem_pub_off);
        args.peer_id = Some(peer_id_off);

        let env_off = crate::generated::generated_envelope::beemesh::machine::Envelope::create(
            &mut fbb, &args,
        );
        fbb.finish(env_off, None);
        fbb.finished_data().to_vec()
    }

    /// Extract signature and public key from flatbuffer envelope for verification
    pub fn fb_envelope_extract_sig_pub(envelope_bytes: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
        if let Ok(envelope) = root_as_envelope(envelope_bytes) {
            // Parse signature from sig field (might have prefix like "ml-dsa-65:BASE64")
            if let Some(sig_field) = envelope.sig() {
                let sig_b64 = sig_field
                    .splitn(2, ':')
                    .nth(if sig_field.contains(':') { 1 } else { 0 })
                    .unwrap_or(sig_field);
                if let Ok(sig_bytes) = base64::engine::general_purpose::STANDARD.decode(sig_b64) {
                    if let Some(pubkey_field) = envelope.pubkey() {
                        if let Ok(pub_bytes) =
                            base64::engine::general_purpose::STANDARD.decode(pubkey_field)
                        {
                            return Some((sig_bytes, pub_bytes));
                        }
                    }
                }
            }
        }
        None
    }

    /// Parse a FlatBuffer Envelope and return canonical bytes for signature verification
    pub fn fb_envelope_extract_sig_pub_legacy(
        buf: &[u8],
    ) -> anyhow::Result<(Vec<u8>, Vec<u8>, Vec<u8>, String, String)> {
        let fb_env = root_as_envelope(buf)
            .map_err(|e| anyhow::anyhow!("failed to parse envelope flatbuffer: {:?}", e))?;

        let payload_vec = fb_env
            .payload()
            .map(|b| b.iter().collect::<Vec<u8>>())
            .unwrap_or_default();
        let payload_type = fb_env.payload_type().unwrap_or("");
        let nonce = fb_env.nonce().unwrap_or("");
        let ts = fb_env.ts();
        let alg = fb_env.alg().unwrap_or("");
        let kem_pub_field = fb_env.kem_pubkey().unwrap_or("");

        let canonical = build_envelope_canonical(
            &payload_vec,
            payload_type,
            nonce,
            ts,
            alg,
            Some(kem_pub_field),
        );

        let sig_field = fb_env.sig().unwrap_or("").to_string();
        let pubkey_field = fb_env.pubkey().unwrap_or("").to_string();

        // Extract base64 portion of signature (after possible prefix)
        let sig_b64 = sig_field
            .splitn(2, ':')
            .nth(if sig_field.contains(':') { 1 } else { 0 })
            .unwrap_or(&sig_field);
        let sig_bytes = base64::engine::general_purpose::STANDARD
            .decode(sig_b64)
            .map_err(|e| anyhow::anyhow!("failed to base64-decode signature: {}", e))?;

        let pub_bytes = base64::engine::general_purpose::STANDARD
            .decode(&pubkey_field)
            .map_err(|e| anyhow::anyhow!("failed to base64-decode pubkey: {}", e))?;

        Ok((canonical, sig_bytes, pub_bytes, sig_field, pubkey_field))
    }

    pub fn build_handshake(
        nonce: u32,
        timestamp: u64,
        protocol_version: &str,
        signature: &str,
    ) -> Vec<u8> {
        let mut fbb = FlatBufferBuilder::with_capacity(128);
        let proto = fbb.create_string(protocol_version);
        let sig = fbb.create_string(signature);
        let mut args: crate::generated::generated_handshake::beemesh::machine::HandshakeArgs =
            Default::default();
        args.nonce = nonce;
        args.timestamp = timestamp;
        args.protocol_version = Some(proto);
        args.signature = Some(sig);
        let off = crate::generated::generated_handshake::beemesh::machine::Handshake::create(
            &mut fbb, &args,
        );
        fbb.finish(off, None);
        fbb.finished_data().to_vec()
    }

    pub fn build_applied_manifest(
        id: &str,
        tenant: &str,
        operation_id: &str,
        origin_peer: &str,
        owner_pubkey: &[u8],
        signature: &[u8],
        manifest_json: &str,
        manifest_kind: &str,
        labels: Vec<(String, String)>,
        timestamp: u64,
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
        let label_offsets: Vec<_> = labels
            .iter()
            .map(|(k, v)| {
                let key_offset = fbb.create_string(k);
                let value_offset = fbb.create_string(v);
                crate::machine::generated_applied_manifest::KeyValue::create(
                    &mut fbb,
                    &crate::machine::generated_applied_manifest::KeyValueArgs {
                        key: Some(key_offset),
                        value: Some(value_offset),
                    },
                )
            })
            .collect();
        let labels_offset = fbb.create_vector(&label_offsets);

        let args = AppliedManifestArgs {
            id: Some(id_offset),
            tenant: Some(tenant_offset),
            operation_id: Some(operation_id_offset),
            origin_peer: Some(origin_peer_offset),
            owner_pubkey: Some(owner_pubkey_offset),
            signature_scheme: crate::machine::generated_applied_manifest::SignatureScheme::NONE,
            signature: Some(signature_offset),
            manifest_json: Some(manifest_json_offset),
            manifest_kind: Some(manifest_kind_offset),
            labels: Some(labels_offset),
            timestamp,
            operation: crate::machine::generated_applied_manifest::OperationType::APPLY,
            ttl_secs,
            content_hash: Some(content_hash_offset),
        };

        let manifest = AppliedManifest::create(&mut fbb, &args);
        fbb.finish(manifest, None);
        fbb.finished_data().to_vec()
    }

    // Helper function to build a ManifestTarget (returns the tuple for the builder)
    pub fn build_manifest_target(peer_id: &str, payload_json: &str) -> (String, String) {
        (peer_id.to_string(), payload_json.to_string())
    }

    pub fn build_assign_request(chosen_peers: &[String]) -> Vec<u8> {
        let mut fbb = FlatBufferBuilder::with_capacity(512);

        // Create vector of peer ID strings
        let peer_offsets: Vec<_> = chosen_peers
            .iter()
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

    pub fn build_nodes_response(peers: &[String]) -> Vec<u8> {
        let mut fbb = FlatBufferBuilder::with_capacity(512);
        let peer_strings: Vec<_> = peers.iter().map(|p| fbb.create_string(p)).collect();
        let peers_vector = fbb.create_vector(&peer_strings);

        let nodes_response = NodesResponse::create(
            &mut fbb,
            &NodesResponseArgs {
                peers: Some(peers_vector),
            },
        );
        fbb.finish(nodes_response, None);
        fbb.finished_data().to_vec()
    }

    pub fn build_candidates_response_with_keys(
        ok: bool,
        candidates: &[(String, String)], // (peer_id, public_key)
    ) -> Vec<u8> {
        let mut fbb = FlatBufferBuilder::with_capacity(1024);

        // Create CandidateNode objects
        let candidate_offsets: Vec<_> = candidates
            .iter()
            .map(|(peer_id, public_key)| {
                let peer_id_offset = fbb.create_string(peer_id);
                let public_key_offset = fbb.create_string(public_key);
                CandidateNode::create(
                    &mut fbb,
                    &CandidateNodeArgs {
                        peer_id: Some(peer_id_offset),
                        public_key: Some(public_key_offset),
                    },
                )
            })
            .collect();
        let candidates_vector = fbb.create_vector(&candidate_offsets);

        let candidates_response = CandidatesResponse::create(
            &mut fbb,
            &CandidatesResponseArgs {
                ok,
                candidates: Some(candidates_vector),
            },
        );
        fbb.finish(candidates_response, None);
        fbb.finished_data().to_vec()
    }

    pub fn build_candidates_response(ok: bool, responders: &[String]) -> Vec<u8> {
        let candidates: Vec<(String, String)> = responders
            .iter()
            .map(|peer_id| (peer_id.clone(), String::new())) // Empty public key for backward compatibility
            .collect();
        build_candidates_response_with_keys(ok, &candidates)
    }

    pub fn build_task_create_response(
        ok: bool,
        task_id: &str,
        manifest_id: &str,
        selection_window_ms: u64,
    ) -> Vec<u8> {
        let mut fbb = FlatBufferBuilder::with_capacity(256);
        let task_id_off = fbb.create_string(task_id);
        let manifest_id_off = fbb.create_string(manifest_id);
        let message_off = fbb.create_string("task created");

        let task_response = crate::generated::generated_task_create_response::beemesh::machine::TaskCreateResponse::create(&mut fbb, &crate::generated::generated_task_create_response::beemesh::machine::TaskCreateResponseArgs {
            ok,
            task_id: Some(task_id_off),
            manifest_ref: Some(manifest_id_off),
            selection_window_ms,
            message: Some(message_off),
        });
        fbb.finish(task_response, None);
        fbb.finished_data().to_vec()
    }

    pub fn build_task_status_response(
        task_id: &str,
        state: &str,
        assigned_peers: &[String],
        shares_distributed: &[String],
        manifest_cid: Option<&str>,
    ) -> Vec<u8> {
        let mut fbb = FlatBufferBuilder::with_capacity(512);
        let task_id_off = fbb.create_string(task_id);
        let state_off = fbb.create_string(state);

        let assigned_strings: Vec<_> = assigned_peers
            .iter()
            .map(|p| fbb.create_string(p))
            .collect();
        let assigned_vector = fbb.create_vector(&assigned_strings);

        let distributed_strings: Vec<_> = shares_distributed
            .iter()
            .map(|p| fbb.create_string(p))
            .collect();
        let distributed_vector = fbb.create_vector(&distributed_strings);

        let manifest_cid_off = manifest_cid.map(|c| fbb.create_string(c));

        let status_response = TaskStatusResponse::create(
            &mut fbb,
            &TaskStatusResponseArgs {
                task_id: Some(task_id_off),
                state: Some(state_off),
                assigned_peers: Some(assigned_vector),
                shares_distributed: Some(distributed_vector),
                manifest_cid: manifest_cid_off,
            },
        );
        fbb.finish(status_response, None);
        fbb.finished_data().to_vec()
    }

    pub fn build_assign_response(
        ok: bool,
        task_id: &str,
        assigned_peers: &[String],
        per_peer_results: &[(String, String)],
    ) -> Vec<u8> {
        let mut fbb = FlatBufferBuilder::with_capacity(512);
        let task_id_off = fbb.create_string(task_id);

        let assigned_strings: Vec<_> = assigned_peers
            .iter()
            .map(|p| fbb.create_string(p))
            .collect();
        let assigned_vector = fbb.create_vector(&assigned_strings);

        let per_peer_json = serde_json::to_string(
            &per_peer_results
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect::<std::collections::HashMap<_, _>>(),
        )
        .unwrap_or_default();
        let per_peer_off = fbb.create_string(&per_peer_json);

        let assign_response =
            crate::generated::generated_assign_response::beemesh::machine::AssignResponse::create(
                &mut fbb,
                &crate::generated::generated_assign_response::beemesh::machine::AssignResponseArgs {
                    ok,
                    task_id: Some(task_id_off),
                    assigned_peers: Some(assigned_vector),
                    per_peer_results_json: Some(per_peer_off),
                },
            );
        fbb.finish(assign_response, None);
        fbb.finished_data().to_vec()
    }

    pub fn build_apply_manifest_response(
        ok: bool,
        tenant: &str,
        replicas_requested: u32,
        assigned_peers: &[String],
        per_peer_results: &[(String, String)],
    ) -> Vec<u8> {
        let mut fbb = FlatBufferBuilder::with_capacity(512);
        let tenant_off = fbb.create_string(tenant);

        let assigned_strings: Vec<_> = assigned_peers
            .iter()
            .map(|p| fbb.create_string(p))
            .collect();
        let assigned_vector = fbb.create_vector(&assigned_strings);

        let per_peer_json = serde_json::to_string(
            &per_peer_results
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect::<std::collections::HashMap<_, _>>(),
        )
        .unwrap_or_default();
        let per_peer_off = fbb.create_string(&per_peer_json);

        let manifest_response = crate::generated::generated_apply_manifest_response::beemesh::machine::ApplyManifestResponse::create(&mut fbb, &crate::generated::generated_apply_manifest_response::beemesh::machine::ApplyManifestResponseArgs {
            ok,
            tenant: Some(tenant_off),
            replicas_requested,
            assigned_peers: Some(assigned_vector),
            per_peer_results_json: Some(per_peer_off),
        });
        fbb.finish(manifest_response, None);
        fbb.finished_data().to_vec()
    }

    /// Create an encrypted envelope with hybrid encryption (KEM + AES-GCM)
    pub fn build_encrypted_envelope(
        payload: &[u8],
        payload_type: &str,
        recipient_pubkey: &[u8],
        sender_privkey: &[u8],
        sender_pubkey: &str,
    ) -> anyhow::Result<Vec<u8>> {
        // Encrypt the payload for the recipient
        let encrypted_payload = crypto::encrypt_payload_for_recipient(recipient_pubkey, payload)?;

        // Generate nonce and timestamp
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        std::time::SystemTime::now().hash(&mut hasher);
        let nonce = format!("{:x}", hasher.finish());
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Build canonical envelope for signing
        let canonical = build_envelope_canonical(
            &encrypted_payload,
            payload_type,
            &nonce,
            ts,
            "ml-dsa-65",
            None,
        );

        // Decode sender public key from base64 for signing
        let sender_pubkey_bytes = base64::engine::general_purpose::STANDARD
            .decode(sender_pubkey)
            .map_err(|e| anyhow::anyhow!("Failed to decode sender public key: {}", e))?;

        // Sign the canonical envelope and unpack signature/pub tuple
        let (sig_b64, pub_b64) =
            crypto::sign_envelope(sender_privkey, &sender_pubkey_bytes, &canonical)?;

        // Build the final signed envelope with encrypted payload
        let envelope = build_envelope_signed(
            &encrypted_payload,
            payload_type,
            &nonce,
            ts,
            "ml-dsa-65",
            "ml-dsa-65",
            &sig_b64,
            &pub_b64,
            None,
        );

        Ok(envelope)
    }

    /// Create an encrypted envelope with hybrid encryption and peer ID
    pub fn build_encrypted_envelope_with_peer(
        payload: &[u8],
        payload_type: &str,
        recipient_pubkey: &[u8],
        sender_privkey: &[u8],
        sender_pubkey: &str,
        peer_id: &str,
        sender_kem_pub_b64: Option<&str>,
    ) -> anyhow::Result<Vec<u8>> {
        // Encrypt the payload for the recipient
        let encrypted_payload = crypto::encrypt_payload_for_recipient(recipient_pubkey, payload)?;

        // Generate nonce and timestamp
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        std::time::SystemTime::now().hash(&mut hasher);
        let nonce = format!("{:x}", hasher.finish());
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Build canonical envelope for signing (with peer_id) and include optional kem_pub
        let canonical = build_envelope_canonical_with_peer(
            &encrypted_payload,
            payload_type,
            &nonce,
            ts,
            "ml-dsa-65",
            peer_id,
            sender_kem_pub_b64,
        );

        // Decode sender public key from base64 for signing
        let sender_pubkey_bytes = base64::engine::general_purpose::STANDARD
            .decode(sender_pubkey)
            .map_err(|e| anyhow::anyhow!("Failed to decode sender public key: {}", e))?;

        // Sign the canonical envelope and unpack signature/pub tuple
        let (sig_b64, pub_b64) =
            crypto::sign_envelope(sender_privkey, &sender_pubkey_bytes, &canonical)?;

        // Build the final signed envelope with encrypted payload (with peer_id and optional kem_pub)
        let envelope = build_envelope_signed_with_peer(
            &encrypted_payload,
            payload_type,
            &nonce,
            ts,
            "ml-dsa-65",
            "ml-dsa-65",
            &sig_b64,
            &pub_b64,
            peer_id,
            sender_kem_pub_b64,
        );

        Ok(envelope)
    }

    pub fn build_apply_manifest_request(
        manifest_envelope_json: &str,
    ) -> Vec<u8> {
        let mut fbb = FlatBufferBuilder::with_capacity(1024);
        let manifest_off = fbb.create_string(manifest_envelope_json);

        let request = crate::generated::generated_apply_manifest_request::beemesh::machine::ApplyManifestRequest::create(&mut fbb, &crate::generated::generated_apply_manifest_request::beemesh::machine::ApplyManifestRequestArgs {
            manifest_envelope_json: Some(manifest_off),
        });
        fbb.finish(request, None);
        fbb.finished_data().to_vec()
    }

    // Extract manifest name from manifest data
    pub fn extract_manifest_name(manifest_data: &[u8]) -> Option<String> {
        let manifest_str = std::str::from_utf8(manifest_data).ok()?;
        // Simple YAML parsing - look for name field
        for line in manifest_str.lines() {
            if line.trim().starts_with("name:") {
                return line.split(':').nth(1).map(|s| s.trim().to_string());
            }
        }
        None
    }

    // Compute manifest ID from name and version
    pub fn compute_manifest_id(name: &str, version: u64) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        name.hash(&mut hasher);
        version.hash(&mut hasher);
        format!("{:x}", hasher.finish())[..16].to_string()
    }

    // Compute manifest ID directly from content
    pub fn compute_manifest_id_from_content(manifest_data: &[u8]) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        manifest_data.hash(&mut hasher);
        format!("{:x}", hasher.finish())[..16].to_string()
    }
}

pub mod libp2p_constants;

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn flatbuffers_health_roundtrip() {
        let health_data = machine::build_health(true, "healthy");
        let health = machine::root_as_health(&health_data).unwrap();
        assert!(health.ok());
        assert_eq!(health.status().unwrap(), "healthy");
    }
}
