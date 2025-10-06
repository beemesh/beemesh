use log::info;
use dirs::home_dir;
use reqwest;
use serde_json::Value as JsonValue;
use serde_yaml;
use std::env;
use std::fs;
use std::path::PathBuf;
use base64::Engine;
use crypto::{ensure_keypair_on_disk, encrypt_manifest, split_symmetric_key, sign_envelope, KEY_DIR};

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

    // build manifest envelope (contains encrypted manifest payload to store in DHT)
    let manifest_envelope = serde_json::json!({
        "payload": base64::engine::general_purpose::STANDARD.encode(&ciphertext),
        "nonce": base64::engine::general_purpose::STANDARD.encode(&nonce_bytes),
        "shares_meta": { "n": n, "k": k, "count": shares_vec.len() },
    });

    // sign manifest envelope
    let manifest_envelope_bytes = serde_json::to_vec(&manifest_envelope)?;
    let (manifest_sig_b64, manifest_pub_b64) = sign_envelope(&sk_bytes, &pk_bytes, &manifest_envelope_bytes)?;
    let mut manifest_envelope_signed = manifest_envelope;
    manifest_envelope_signed["sig"] = serde_json::Value::String(format!("ml-dsa-65:{}", manifest_sig_b64));
    manifest_envelope_signed["pubkey"] = serde_json::Value::String(manifest_pub_b64.clone());

    // build shares envelope (contains the base64 shares for peers) in Envelope shape
    let shares_b64: Vec<String> = shares_vec.iter().map(|s| base64::engine::general_purpose::STANDARD.encode(s)).collect();
    let shares_payload = serde_json::json!({
        "shares": shares_b64,
        "shares_meta": { "n": n, "k": k, "count": shares_vec.len() }
    });
    let shares_payload_bytes = serde_json::to_vec(&shares_payload)?;
    // envelope expects a base64 payload and a nonce field
    let shares_nonce_bytes: [u8; 16] = rand::random();
    let shares_envelope = serde_json::json!({
        "payload": base64::engine::general_purpose::STANDARD.encode(&shares_payload_bytes),
        "nonce": base64::engine::general_purpose::STANDARD.encode(&shares_nonce_bytes),
        "shares_meta": { "n": n, "k": k, "count": shares_vec.len() }
    });
    let shares_envelope_bytes = serde_json::to_vec(&shares_envelope)?;
    let (shares_sig_b64, shares_pub_b64) = sign_envelope(&sk_bytes, &pk_bytes, &shares_envelope_bytes)?;
    let mut shares_envelope_signed = shares_envelope;
    shares_envelope_signed["sig"] = serde_json::Value::String(format!("ml-dsa-65:{}", shares_sig_b64));
    shares_envelope_signed["pubkey"] = serde_json::Value::String(shares_pub_b64);

    // wrapper message containing both envelopes
    let _final_msg = serde_json::json!({
        "manifest_envelope": manifest_envelope_signed,
        "shares_envelope": shares_envelope_signed,
    });

    // hardcoded tenant for now
    let tenant = "00000000-0000-0000-0000-000000000000";

    // Calculate manifest_id deterministically using the same method as the machine
    let operation_id = uuid::Uuid::new_v4().to_string();
    let manifest_envelope_signed_str = serde_json::to_string(&manifest_envelope_signed)?;
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
    let client = reqwest::Client::new();

    // 1) Create task (store manifest in task store)
    let create_url = format!("{}/tenant/{}/tasks", base.trim_end_matches('/'), tenant);
    let create_body = serde_json::json!({ 
        "manifest": manifest_envelope_signed, 
        "manifest_id": manifest_id.clone(),
        "operation_id": operation_id.clone()
    });
    let resp = client
        .post(&create_url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(create_body.to_string())
        .send()
        .await?;
    let status = resp.status();
    let body_text = resp.text().await?;
    if !status.is_success() {
        anyhow::bail!("create_task failed: {} {}", status, body_text);
    }
    let body_json: serde_json::Value = serde_json::from_str(&body_text)?;
    let returned_manifest_id = body_json.get("manifest_id").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("no manifest_id in response"))?.to_string();
    info!("Created task with manifest_id {}", returned_manifest_id);

    // 2) Query candidates for the required replicas
    let candidates_url = format!("{}/tenant/{}/tasks/{}/candidates", base.trim_end_matches('/'), tenant, manifest_id);
    let resp = client.get(&candidates_url).send().await?;
    let status = resp.status();
    let body_text = resp.text().await?;
    if !status.is_success() {
        anyhow::bail!("candidates request failed: {} {}", status, body_text);
    }
    let cand_json: serde_json::Value = serde_json::from_str(&body_text)?;
    let responders = cand_json.get("responders").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let peers: Vec<String> = responders.into_iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect();
    info!("Candidates: {:?}", peers);

    // 3) Distribute shares to the responders (build targets)
    let mut targets: Vec<serde_json::Value> = Vec::new();
    // For simplicity, send shares to all responders found
    for (i, peer) in peers.iter().enumerate() {
        // read local share file created earlier
        let share_path = home.join(KEY_DIR).join("shares").join(format!("share-{}.bin", i + 1));
        if share_path.exists() {
            let share_bytes = tokio::fs::read(&share_path).await?;
            let share_b64 = base64::engine::general_purpose::STANDARD.encode(&share_bytes);
            let target = serde_json::json!({ "peer_id": peer, "payload": { "share": share_b64 } });
            targets.push(target);
        } else {
            log::warn!("missing local share file: {}", share_path.display());
        }
    }

    let dist_url = format!("{}/tenant/{}/tasks/{}/distribute_shares", base.trim_end_matches('/'), tenant, manifest_id);
    let dist_body = serde_json::json!({ "shares_envelope": shares_envelope_signed, "targets": targets });
    let resp = client.post(&dist_url).header(reqwest::header::CONTENT_TYPE, "application/json").body(dist_body.to_string()).send().await?;
    let status = resp.status();
    let body_text = resp.text().await?;
    if !status.is_success() {
        anyhow::bail!("distribute_shares failed: {} {}", status, body_text);
    }
    let dist_resp: serde_json::Value = serde_json::from_str(&body_text)?;
    info!("Distribute shares response: {:?}", dist_resp);

    // 4) Distribute capability tokens to authorize share fetching
    let capability_resp = distribute_capability_tokens(&client, &base, tenant, &manifest_id, &peers, &pk_bytes, &sk_bytes).await?;
    info!("Distribute capabilities response: {:?}", capability_resp);

    // 5) Assign task to replicas
    // Determine replicas desired from manifest
    let replicas = manifest_json.get("replicas").and_then(|v| v.as_u64()).or_else(|| manifest_json.get("spec").and_then(|s| s.get("replicas").and_then(|r| r.as_u64()))).unwrap_or(1) as usize;
    let chosen_peers: Vec<String> = peers.into_iter().take(replicas).collect();
    let assign_url = format!("{}/tenant/{}/tasks/{}/assign", base.trim_end_matches('/'), tenant, manifest_id);
    let assign_body = serde_json::json!({ "chosen_peers": chosen_peers });
    let resp = client.post(&assign_url).header(reqwest::header::CONTENT_TYPE, "application/json").body(assign_body.to_string()).send().await?;
    let status = resp.status();
    let body = resp.text().await?;
    info!("Assign response: {}\n{}", status, body);

    if !status.is_success() {
        anyhow::bail!("assign failed: {}", status);
    }

    info!("Apply succeeded");

    Ok(manifest_id)
}

/// Create and distribute capability tokens to peers to enable keyshare fetches
async fn distribute_capability_tokens(
    client: &reqwest::Client,
    base: &str,
    tenant: &str,
    task_id: &str,
    peers: &[String],
    pk_bytes: &[u8],
    sk_bytes: &[u8],
) -> anyhow::Result<serde_json::Value> {
    // Get manifest_id from the task
    let mid_resp = client.get(&format!("{}/tenant/{}/tasks/{}/manifest_id", base.trim_end_matches('/'), tenant, task_id)).send().await?;
    let mid_status = mid_resp.status();
    let mid_body_text = mid_resp.text().await?;
    if !mid_status.is_success() {
        anyhow::bail!("get manifest_id failed: {} {}", mid_status, mid_body_text);
    }
    let mid_json: serde_json::Value = serde_json::from_str(&mid_body_text)?;
    let manifest_id = mid_json.get("manifest_id").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("no manifest_id in response"))?.to_string();

    // Get current timestamp
    let ts_millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    // Create capability tokens for each peer
    let mut targets: Vec<serde_json::Value> = Vec::new();
    
    for peer_id in peers {
        // Create capability token payload
        let token_obj = serde_json::json!({
            "task_id": manifest_id,
            "issuer": "cli", // CLI acts as the issuer
            "required_quorum": 1,
            "caveats": { "authorized_peer": peer_id },
            "ts": ts_millis,
        });
        let token_bytes = serde_json::to_vec(&token_obj)?;
        
        // Create capability envelope
        let cap_nonce_bytes: [u8; 16] = rand::random();
        let mut env_map = serde_json::Map::new();
        env_map.insert("payload".to_string(), serde_json::Value::String(base64::engine::general_purpose::STANDARD.encode(&token_bytes)));
        env_map.insert("manifest_id".to_string(), serde_json::Value::String(manifest_id.clone()));
        env_map.insert("type".to_string(), serde_json::Value::String("capability".to_string()));
        env_map.insert("nonce".to_string(), serde_json::Value::String(base64::engine::general_purpose::STANDARD.encode(&cap_nonce_bytes)));
        let env_value = serde_json::Value::Object(env_map);
        let env_bytes = serde_json::to_vec(&env_value)?;

        // Sign capability envelope
        let (cap_sig_b64, cap_pub_b64) = sign_envelope(sk_bytes, pk_bytes, &env_bytes)?;
        let mut signed_env_obj = env_value.as_object().cloned().ok_or_else(|| anyhow::anyhow!("failed to get env object"))?;
        signed_env_obj.insert("sig".to_string(), serde_json::Value::String(format!("ml-dsa-65:{}", cap_sig_b64)));
        signed_env_obj.insert("pubkey".to_string(), serde_json::Value::String(cap_pub_b64));
        
        // Create target for this peer
        let target = serde_json::json!({
            "peer_id": peer_id,
            "payload": serde_json::Value::Object(signed_env_obj)
        });
        targets.push(target);
    }

    // Send capability tokens via REST API
    let cap_url = format!("{}/tenant/{}/tasks/{}/distribute_capabilities", base.trim_end_matches('/'), tenant, task_id);
    let cap_body = serde_json::json!({ "targets": targets });
    let resp = client.post(&cap_url).header(reqwest::header::CONTENT_TYPE, "application/json").body(cap_body.to_string()).send().await?;
    let status = resp.status();
    let body_text = resp.text().await?;
    if !status.is_success() {
        anyhow::bail!("distribute_capabilities failed: {} {}", status, body_text);
    }
    
    Ok(serde_json::from_str(&body_text)?)
}