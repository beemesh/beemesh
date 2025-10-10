use base64::Engine;
use crypto::{encrypt_manifest, ensure_keypair_on_disk, split_symmetric_key, KEY_DIR};
use dirs::home_dir;
use log::info;
use serde_json::Value as JsonValue;
use serde_yaml;
use std::env;
use std::fs;
use std::path::PathBuf;

mod flatbuffers;
use flatbuffers::{CapabilityTarget, FlatbufferClient};

mod flatbuffer_envelope;
use flatbuffer_envelope::{envelope_to_json, FlatbufferEnvelopeBuilder};

pub async fn apply_file(path: PathBuf) -> anyhow::Result<String> {
    if !path.exists() {
        anyhow::bail!("file not found: {}", path.display());
    }

    let contents = tokio::fs::read_to_string(&path).await?;

    // Parse manifest to JSON if possible, else wrap raw
    let manifest_json: JsonValue = match serde_yaml::from_str(&contents) {
        Ok(v) => v,
        Err(_) => serde_json::json!({"raw": contents}),
    };

    // ensure keypair
    let (pk_bytes, sk_bytes) = ensure_keypair_on_disk()?;

    // encrypt manifest
    let (ciphertext, nonce_bytes, sym, _nonce) = encrypt_manifest(&manifest_json)?;

    // split symmetric key into shares (n=3, k=2)
    let n = 3usize;
    let k = 2usize;
    let shares_vec = split_symmetric_key(&sym, n, k);

    // store shares under ~/.beemesh/shares
    let home = home_dir().ok_or_else(|| anyhow::anyhow!("could not determine home dir"))?;
    let shares_dir = home.join(KEY_DIR).join("shares");
    if !shares_dir.exists() {
        fs::create_dir_all(&shares_dir)?;
    }
    for (i, share) in shares_vec.iter().enumerate() {
        let fname = shares_dir.join(format!("share-{}.bin", i + 1));
        fs::write(&fname, share)?;
    }

    // build manifest envelope using flatbuffers (contains original YAML content)
    let mut envelope_builder = FlatbufferEnvelopeBuilder::new();
    let manifest_envelope_bytes = envelope_builder.build_simple_envelope(
        "manifest", &contents, // use original YAML content instead of encrypted ciphertext
    )?;

    // sign manifest envelope
    let manifest_envelope_signed_bytes =
        FlatbufferEnvelopeBuilder::sign_envelope(&manifest_envelope_bytes, &sk_bytes, &pk_bytes)?;

    // build shares envelope using flatbuffers (contains the base64 shares for peers)
    let shares_b64: Vec<String> = shares_vec
        .iter()
        .map(|s| base64::engine::general_purpose::STANDARD.encode(s))
        .collect();
    let shares_envelope_bytes =
        envelope_builder.build_shares_envelope(&shares_b64, n, k, shares_vec.len())?;

    // sign shares envelope
    let shares_envelope_signed_bytes =
        FlatbufferEnvelopeBuilder::sign_envelope(&shares_envelope_bytes, &sk_bytes, &pk_bytes)?;

    // Convert shares envelope to base64 for transport (keep as flatbuffer, not JSON)
    let shares_envelope_signed_b64 =
        base64::engine::general_purpose::STANDARD.encode(&shares_envelope_signed_bytes);

    // Convert signed envelopes to base64 for any needed JSON compatibility
    let manifest_envelope_signed_b64 =
        base64::engine::general_purpose::STANDARD.encode(&manifest_envelope_signed_bytes);

    // wrapper message containing both envelopes (as base64)
    let _final_msg = serde_json::json!({
        "manifest_envelope": manifest_envelope_signed_b64,
        "shares_envelope": shares_envelope_signed_b64,
    });

    // hardcoded tenant for now
    let tenant = "00000000-0000-0000-0000-000000000000";

    // Calculate manifest_id deterministically using the same method as the machine
    let operation_id = uuid::Uuid::new_v4().to_string();
    let manifest_envelope_signed_str = manifest_envelope_signed_b64;
    let manifest_id = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        tenant.hash(&mut hasher);
        operation_id.hash(&mut hasher);
        manifest_envelope_signed_str.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    };

    // API base URL can be overridden with BEEMESH_API env var
    let base = env::var("BEEMESH_API").unwrap_or_else(|_| "http://127.0.0.1:3000".to_string());
    let fb_client = FlatbufferClient::new(base);

    // 1) Create task using flatbuffer client (store manifest in task store)
    let create_resp = fb_client
        .create_task(
            tenant,
            &manifest_envelope_signed_bytes,
            Some(manifest_id.clone()),
            Some(operation_id.clone()),
        )
        .await?;
    let returned_manifest_id = create_resp
        .get("manifest_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("no manifest_id in response"))?
        .to_string();
    info!("Created task with manifest_id {}", returned_manifest_id);

    // 2) Query candidates using flatbuffer client
    let peers = fb_client.get_candidates(tenant, &manifest_id).await?;
    info!("Candidates: {:?}", peers);

    // 3) Distribute shares to the responders using flatbuffers
    let mut target_flatbuffers: Vec<Vec<u8>> = Vec::new();
    let mut target_peer_ids: Vec<String> = Vec::new();

    // For simplicity, send shares to all responders found
    for (i, peer) in peers.iter().enumerate() {
        // read local share file created earlier
        let share_path = home
            .join(KEY_DIR)
            .join("shares")
            .join(format!("share-{}.bin", i + 1));
        if share_path.exists() {
            let share_bytes = tokio::fs::read(&share_path).await?;

            // Create a flatbuffer envelope containing the share data
            let mut envelope_builder = FlatbufferEnvelopeBuilder::new();
            let share_envelope_bytes = envelope_builder.build_simple_envelope(
                "keyshare",
                &base64::engine::general_purpose::STANDARD.encode(&share_bytes),
            )?;

            // Sign the share envelope
            let signed_share_envelope = FlatbufferEnvelopeBuilder::sign_envelope(
                &share_envelope_bytes,
                &sk_bytes,
                &pk_bytes,
            )?;

            target_flatbuffers.push(signed_share_envelope);
            target_peer_ids.push(peer.clone());
        } else {
            log::warn!("missing local share file: {}", share_path.display());
        }
    }

    let dist_resp = fb_client
        .distribute_shares_flatbuffer(
            tenant,
            &manifest_id,
            &shares_envelope_signed_b64,
            &target_peer_ids,
            &target_flatbuffers,
        )
        .await?;
    info!("Distribute shares response: {:?}", dist_resp);

    // 4) Distribute capability tokens using flatbuffers
    let capability_targets =
        create_capability_tokens(&peers, &returned_manifest_id, &pk_bytes, &sk_bytes).await?;
    let capability_resp = fb_client
        .distribute_capabilities(tenant, &manifest_id, &capability_targets)
        .await?;
    info!("Distribute capabilities response: {:?}", capability_resp);

    // 5) Assign task to replicas using flatbuffers
    // Determine replicas desired from manifest
    let replicas = manifest_json
        .get("replicas")
        .and_then(|v| v.as_u64())
        .or_else(|| {
            manifest_json
                .get("spec")
                .and_then(|s| s.get("replicas").and_then(|r| r.as_u64()))
        })
        .unwrap_or(1) as usize;
    let chosen_peers: Vec<String> = peers.into_iter().take(replicas).collect();
    let assign_resp = fb_client
        .assign_task(tenant, &manifest_id, chosen_peers)
        .await?;
    info!("Assign response: {:?}", assign_resp);

    let ok = assign_resp
        .get("ok")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !ok {
        anyhow::bail!("assign failed");
    }

    info!("Apply succeeded");

    Ok(manifest_id)
}

/// Create capability tokens for peers using flatbuffers
async fn create_capability_tokens(
    peers: &[String],
    manifest_id: &str,
    pk_bytes: &[u8],
    sk_bytes: &[u8],
) -> anyhow::Result<Vec<CapabilityTarget>> {
    // Get current timestamp
    let ts_millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    // Create capability tokens for each peer
    let mut targets: Vec<CapabilityTarget> = Vec::new();

    for peer_id in peers {
        // Create capability envelope using flatbuffers
        let mut envelope_builder = FlatbufferEnvelopeBuilder::new();
        let capability_envelope_bytes =
            envelope_builder.build_capability_envelope(manifest_id, peer_id, ts_millis)?;

        // Sign capability envelope
        let capability_envelope_signed_bytes = FlatbufferEnvelopeBuilder::sign_envelope(
            &capability_envelope_bytes,
            sk_bytes,
            pk_bytes,
        )?;

        // Convert to JSON for compatibility with existing APIs
        let signed_env_json = envelope_to_json(&capability_envelope_signed_bytes)?;

        // Create target for this peer
        let target = CapabilityTarget {
            peer_id: peer_id.clone(),
            payload: signed_env_json,
        };
        targets.push(target);
    }

    Ok(targets)
}
