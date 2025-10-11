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
    println!("apply_file: starting with path: {:?}", path);
    info!("apply_file called for path: {:?}", path);

    if !path.exists() {
        println!("apply_file: file not found: {}", path.display());
        anyhow::bail!("file not found: {}", path.display());
    }

    println!("apply_file: file exists, reading contents...");
    let contents = tokio::fs::read_to_string(&path).await?;
    println!(
        "apply_file: file contents read successfully, length: {}",
        contents.len()
    );
    info!(
        "File contents read successfully, length: {}",
        contents.len()
    );

    // Parse manifest to JSON if possible, else wrap raw
    println!("apply_file: parsing manifest...");
    let manifest_json: JsonValue = match serde_yaml::from_str(&contents) {
        Ok(v) => v,
        Err(_) => serde_json::json!({"raw": contents}),
    };
    println!("apply_file: manifest parsed successfully");

    // ensure keypair
    println!("apply_file: ensuring keypair...");
    let (pk_bytes, sk_bytes) = ensure_keypair_on_disk()?;
    println!("apply_file: keypair ensured");

    // encrypt manifest
    println!("apply_file: encrypting manifest...");
    let (ciphertext, nonce_bytes, sym, _nonce) = encrypt_manifest(&manifest_json)?;
    println!("apply_file: manifest encrypted");

    // split symmetric key into shares (n=3, k=2)
    println!("apply_file: splitting symmetric key...");
    let n = 3usize;
    let k = 2usize;
    let shares_vec = split_symmetric_key(&sym, n, k);
    println!(
        "apply_file: symmetric key split into {} shares",
        shares_vec.len()
    );

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

    // build encrypted manifest envelope using flatbuffers (contains encrypted ciphertext)
    let pk_b64 = base64::engine::general_purpose::STANDARD.encode(&pk_bytes);
    let mut envelope_builder =
        FlatbufferEnvelopeBuilder::with_keys("cli-client".to_string(), pk_b64.clone());
    let manifest_envelope_bytes = envelope_builder.build_manifest_envelope(
        &ciphertext,
        &nonce_bytes,
        n,
        k,
        shares_vec.len(),
    )?;

    // sign manifest envelope
    let manifest_envelope_signed_bytes =
        envelope_builder.sign_envelope(&manifest_envelope_bytes, &sk_bytes, &pk_bytes)?;

    // Convert signed envelope to base64 for manifest_id calculation
    let manifest_envelope_signed_b64 =
        base64::engine::general_purpose::STANDARD.encode(&manifest_envelope_signed_bytes);

    // hardcoded tenant for now
    let tenant = "00000000-0000-0000-0000-000000000000";

    // Generate operation_id for the server
    let operation_id = uuid::Uuid::new_v4().to_string();

    // build shares envelope using flatbuffers (contains the base64 shares for peers)
    let shares_b64: Vec<String> = shares_vec
        .iter()
        .map(|s| base64::engine::general_purpose::STANDARD.encode(s))
        .collect();
    // Create a temporary placeholder for shares envelope - we'll rebuild it with correct manifest_id later
    let shares_envelope_bytes = envelope_builder.build_shares_envelope(
        &shares_b64,
        n,
        k,
        shares_vec.len(),
        "placeholder", // Will be replaced after create_task
    )?;

    // sign shares envelope
    let shares_envelope_signed_bytes =
        envelope_builder.sign_envelope(&shares_envelope_bytes, &sk_bytes, &pk_bytes)?;

    // Convert shares envelope to base64 for transport (keep as flatbuffer, not JSON)
    let shares_envelope_signed_b64 =
        base64::engine::general_purpose::STANDARD.encode(&shares_envelope_signed_bytes);

    // wrapper message containing both envelopes (as base64)
    let _final_msg = serde_json::json!({
        "manifest_envelope": manifest_envelope_signed_b64,
        "shares_envelope": shares_envelope_signed_b64,
    });

    // API base URL can be overridden with BEEMESH_API env var
    let base = env::var("BEEMESH_API").unwrap_or_else(|_| "http://127.0.0.1:3000".to_string());
    println!(
        "apply_file: creating FlatbufferClient with base URL: {}",
        base
    );
    info!("Creating FlatbufferClient with base URL: {}", base);
    let mut fb_client = FlatbufferClient::new(base)?;
    println!("apply_file: FlatbufferClient created");

    // Fetch machine's public key for encrypted communication
    println!("apply_file: fetching machine's public key...");
    info!("Fetching machine's public key...");
    fb_client.fetch_machine_public_key().await?;
    println!("apply_file: machine's public key fetched successfully");
    info!("Successfully fetched machine's public key");

    // TODO: Get machine's public key from somewhere (e.g., discovery, config file, etc.)
    // For now, communication will work but won't be encrypted without the machine's pubkey

    // 1) Create task using flatbuffer client (store manifest in task store)
    println!("apply_file: about to call create_task...");
    info!("About to call create_task...");
    let create_resp = fb_client
        .create_task(
            tenant,
            &manifest_envelope_signed_bytes,
            None, // Let server compute manifest_id
            Some(operation_id.clone()),
        )
        .await?;
    println!("apply_file: create_task completed successfully");
    info!("create_task completed successfully");
    let returned_manifest_id = create_resp
        .get("manifest_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("no manifest_id in response"))?
        .to_string();
    info!("Created task with manifest_id {}", returned_manifest_id);

    // Now rebuild the shares envelope with the correct manifest_id
    let shares_envelope_bytes = envelope_builder.build_shares_envelope(
        &shares_b64,
        n,
        k,
        shares_vec.len(),
        &returned_manifest_id,
    )?;

    // Rebuild signed shares envelope
    let shares_envelope_signed_bytes =
        envelope_builder.sign_envelope(&shares_envelope_bytes, &sk_bytes, &pk_bytes)?;

    // Convert to base64 again
    let shares_envelope_signed_b64 =
        base64::engine::general_purpose::STANDARD.encode(&shares_envelope_signed_bytes);

    // 2) Query candidates using flatbuffer client
    println!("apply_file: about to call get_candidates...");
    info!("About to call get_candidates...");
    let peers = fb_client
        .get_candidates(tenant, &returned_manifest_id)
        .await?;
    println!(
        "apply_file: get_candidates completed successfully, found {} peers",
        peers.len()
    );
    info!("get_candidates completed successfully");
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

            // Create a KeyShares flatbuffer containing just this one share with manifest_id
            let share_b64 = base64::engine::general_purpose::STANDARD.encode(&share_bytes);
            let individual_keyshares_fb = protocol::machine::build_key_shares(
                &[share_b64], // Just this one share
                n as u32,
                k as u32,
                1, // count = 1 for individual share
                &returned_manifest_id,
            );

            // Create a flatbuffer envelope containing the KeyShares data
            let mut envelope_builder =
                FlatbufferEnvelopeBuilder::with_keys("cli-client".to_string(), pk_b64.clone());
            let share_envelope_bytes = envelope_builder.build_simple_envelope(
                "keyshare",
                &base64::engine::general_purpose::STANDARD.encode(&individual_keyshares_fb),
            )?;

            // Sign the share envelope
            let signed_share_envelope =
                envelope_builder.sign_envelope(&share_envelope_bytes, &sk_bytes, &pk_bytes)?;

            target_flatbuffers.push(signed_share_envelope);
            target_peer_ids.push(peer.clone());
        } else {
            log::warn!("missing local share file: {}", share_path.display());
        }
    }

    println!("apply_file: about to call distribute_shares_flatbuffer...");
    info!("About to call distribute_shares_flatbuffer...");
    let dist_resp = fb_client
        .distribute_shares_flatbuffer(
            tenant,
            &returned_manifest_id,
            &shares_envelope_signed_b64,
            &target_peer_ids,
            &target_flatbuffers,
        )
        .await?;
    println!("apply_file: distribute_shares_flatbuffer completed successfully");
    info!("distribute_shares_flatbuffer completed successfully");
    info!("Distribute shares response: {:?}", dist_resp);

    // 4) Distribute capability tokens using flatbuffers
    let capability_targets =
        create_capability_tokens(&peers, &returned_manifest_id, &pk_bytes, &sk_bytes).await?;
    println!("apply_file: about to call distribute_capabilities...");
    info!("About to call distribute_capabilities...");
    let capability_resp = fb_client
        .distribute_capabilities(tenant, &returned_manifest_id, &capability_targets)
        .await?;
    println!("apply_file: distribute_capabilities completed successfully");
    info!("distribute_capabilities completed successfully");
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
    println!(
        "apply_file: about to call assign_task with {} chosen_peers",
        chosen_peers.len()
    );
    info!(
        "About to call assign_task with tenant={}, manifest_id={}, chosen_peers={:?}",
        tenant, &returned_manifest_id, &chosen_peers
    );

    let assign_resp = fb_client
        .assign_task(tenant, &returned_manifest_id, chosen_peers)
        .await?;
    println!("apply_file: assign_task completed successfully");
    info!("Assign response: {:?}", assign_resp);

    let ok = assign_resp
        .get("ok")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !ok {
        anyhow::bail!("assign failed");
    }

    info!("Apply succeeded");

    Ok(returned_manifest_id)
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
        let pk_b64 = base64::engine::general_purpose::STANDARD.encode(pk_bytes);
        let mut envelope_builder =
            FlatbufferEnvelopeBuilder::with_keys("cli-client".to_string(), pk_b64);
        let capability_envelope_bytes =
            envelope_builder.build_capability_envelope(manifest_id, peer_id, ts_millis)?;

        // Sign capability envelope
        let capability_envelope_signed_bytes =
            envelope_builder.sign_envelope(&capability_envelope_bytes, sk_bytes, pk_bytes)?;

        // Create target for this peer with flatbuffer bytes
        let target = CapabilityTarget {
            peer_id: peer_id.clone(),
            payload: capability_envelope_signed_bytes,
        };
        targets.push(target);
    }

    Ok(targets)
}
