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
    // Also export Args and helper finish function for builders/tests
    pub use crate::generated::generated_envelope::beemesh::machine::{ EnvelopeArgs, finish_envelope_buffer };
    
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

    // Macro to generate simple flatbuffer builder functions.
    // Usage:
    // fb_builder!(fn_name, fb_mod_path, FBType, FBArgsType, [field1:ty, field2:ty => string, ...]);
    // - fields marked with `=> string` will be converted with `fbb.create_string` and wrapped in `Some(...)`.
    macro_rules! fb_builder {
        // Variant with vector-of-strings support
        ($fn_name:ident, $args_path:path, $type_path:path,
         [ $( $pname:ident : $pty:ty ),* $(,)? ],
         [ $( $sname:ident : $sty:ty ),* $(,)? ],
         [ $( $vname:ident : $vty:ty ),* $(,)? ]
        ) => {
            pub fn $fn_name( $( $pname : $pty ),* , $( $sname : $sty ),* , $( $vname : $vty ),* ) -> Vec<u8> {
                let mut fbb = FlatBufferBuilder::with_capacity(256);
                $( let $sname = fbb.create_string($sname); )*
                // build vectors of string offsets for each vec field
                $(
                    let $vname = {
                        let mut tmp_vec: Vec<flatbuffers::WIPOffset<&str>> = Vec::with_capacity($vname.len());
                        for &s in $vname.iter() {
                            tmp_vec.push(fbb.create_string(s));
                        }
                        fbb.create_vector(&tmp_vec)
                    };
                )*
                let mut args: $args_path = Default::default();
                $( args.$pname = $pname; )*
                $( args.$sname = Some($sname); )*
                $( args.$vname = Some($vname); )*
                let off = <$type_path>::create(&mut fbb, &args);
                fbb.finish(off, None);
                fbb.finished_data().to_vec()
            }
        };

        // Backwards-compatible variant without vector fields
        ($fn_name:ident, $args_path:path, $type_path:path,
         [ $( $pname:ident : $pty:ty ),* $(,)? ],
         [ $( $sname:ident : $sty:ty ),* $(,)? ]
        ) => {
            pub fn $fn_name( $( $pname : $pty ),* , $( $sname : $sty ),* ) -> Vec<u8> {
                let mut fbb = FlatBufferBuilder::with_capacity(128);
                $( let $sname = fbb.create_string($sname); )*
                let mut args: $args_path = Default::default();
                $( args.$pname = $pname; )*
                $( args.$sname = Some($sname); )*
                let off = <$type_path>::create(&mut fbb, &args);
                fbb.finish(off, None);
                fbb.finished_data().to_vec()
            }
        };
    }

    fb_builder!(build_health,
        crate::generated::generated_health::beemesh::machine::HealthArgs,
        crate::generated::generated_health::beemesh::machine::Health,
        [ok: bool],
        [status: &str]
    );

    fb_builder!(build_capacity_request,
        crate::generated::generated_capacity_request::beemesh::machine::CapacityRequestArgs,
        crate::generated::generated_capacity_request::beemesh::machine::CapacityRequest,
        [cpu_milli: u32, memory_bytes: u64, storage_bytes: u64, replicas: u32],
        []
    );

    fb_builder!(build_capacity_reply,
        crate::generated::generated_capacity_reply::beemesh::machine::CapacityReplyArgs,
        crate::generated::generated_capacity_reply::beemesh::machine::CapacityReply,
        [ok: bool, cpu_available_milli: u32, memory_available_bytes: u64, storage_available_bytes: u64],
        [request_id: &str, node_id: &str, region: &str],
        [capabilities: &[&str]]
    );

    fb_builder!(build_apply_request,
        crate::generated::generated_apply_request::beemesh::machine::ApplyRequestArgs,
        crate::generated::generated_apply_request::beemesh::machine::ApplyRequest,
        [replicas: u32],
        [tenant: &str, operation_id: &str, manifest_json: &str, origin_peer: &str]
    );

    fb_builder!(build_apply_response,
        crate::generated::generated_apply_response::beemesh::machine::ApplyResponseArgs,
        crate::generated::generated_apply_response::beemesh::machine::ApplyResponse,
        [ok: bool],
        [operation_id: &str, message: &str]
    );

    fb_builder!(build_keyshare_response,
        crate::generated::generated_keyshare_response::beemesh::machine::KeyShareResponseArgs,
        crate::generated::generated_keyshare_response::beemesh::machine::KeyShareResponse,
        [ok: bool],
        [operation_id: &str, message: &str]
    );

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
    fb_builder!(build_handshake,
        crate::generated::generated_handshake::beemesh::machine::HandshakeArgs,
        crate::generated::generated_handshake::beemesh::machine::Handshake,
        [nonce: u32, timestamp: u64],
        [protocol_version: &str, signature: &str]
    );

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