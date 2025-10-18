use base64::Engine;
use crypto::{encrypt_payload_for_recipient, ensure_keypair_on_disk};
use log::debug;
use log::error;
use log::info;
use scheduler::{NodeCandidate, NodeCapabilities, Scheduler, SchedulerConfig, SchedulingStrategy};

use serde_json::Value as JsonValue;
use serde_yaml;
use std::collections::HashMap;
use std::env;
use std::path::PathBuf;

mod flatbuffers;
use flatbuffers::FlatbufferClient;

mod flatbuffer_envelope;

pub async fn apply_file(path: PathBuf) -> anyhow::Result<String> {
    debug!("apply_file called for path: {:?}", path);

    if !path.exists() {
        error!("apply_file: file not found: {}", path.display());
        anyhow::bail!("file not found: {}", path.display());
    }

    let contents = tokio::fs::read_to_string(&path).await?;
    debug!(
        "apply_file: file contents read successfully, length: {}",
        contents.len()
    );
    info!(
        "File contents read successfully, length: {}",
        contents.len()
    );

    // Parse manifest to JSON if possible, else wrap raw
    let manifest_json: JsonValue = match serde_yaml::from_str(&contents) {
        Ok(v) => v,
        Err(_) => serde_json::json!({"raw": contents}),
    };
    debug!("apply_file: manifest parsed successfully");

    // Extract replicas count from manifest (check spec.replicas or top-level replicas, default to 1)
    let replicas = manifest_json
        .get("spec")
        .and_then(|s| s.get("replicas"))
        .and_then(|r| r.as_u64())
        .or_else(|| manifest_json.get("replicas").and_then(|r| r.as_u64()))
        .unwrap_or(1) as usize;

    info!("Manifest requires {} replicas", replicas);

    // Ensure CLI keypair - use ephemeral in test mode to match machine nodes
    let (_pk_bytes, _sk_bytes) = if std::env::var("BEEMESH_MOCK_ONLY_RUNTIME").is_ok() {
        crypto::ensure_keypair_ephemeral()?
    } else {
        ensure_keypair_on_disk()?
    };

    // Hardcoded tenant for now
    let tenant = "00000000-0000-0000-0000-000000000000";

    // Compute stable manifest_id from manifest content (like Kubernetes)
    let manifest_id = protocol::machine::compute_manifest_id_from_content(&manifest_json, tenant)
        .ok_or_else(|| anyhow::anyhow!("Failed to extract name from manifest"))?;
    debug!("Computed manifest_id: {}", manifest_id);

    // API base URL can be overridden with BEEMESH_API env var
    let base = env::var("BEEMESH_API").unwrap_or_else(|_| "http://127.0.0.1:3000".to_string());
    debug!("Creating FlatbufferClient with base URL: {}", base);
    let mut fb_client = FlatbufferClient::new(base)?;

    // Fetch machine's public key for encrypted communication
    debug!("Fetching machine's public key...");
    fb_client.fetch_machine_public_key().await?;
    debug!("Successfully fetched machine's public key");

    // 1) Get candidates for node selection
    debug!("About to call get_candidates...");
    let peers = fb_client.get_candidates(tenant, &manifest_id).await?;
    debug!(
        "apply_file: get_candidates completed successfully, found {} peers",
        peers.len()
    );

    if peers.is_empty() {
        anyhow::bail!("No candidate nodes available for scheduling");
    }

    // Ensure we have enough peers for the requested replicas
    if peers.len() < replicas {
        anyhow::bail!(
            "Not enough candidate nodes available: need {}, got {}",
            replicas,
            peers.len()
        );
    }

    // 2) Parse peer IDs and public keys from candidates response and use scheduler
    // Expected format: "peer_id:pubkey_b64"
    let mut node_candidates = Vec::new();
    let mut peer_info_map = HashMap::new();

    for peer in &peers {
        if let Some(colon_pos) = peer.find(':') {
            let peer_id = &peer[..colon_pos];
            let pubkey_b64 = &peer[colon_pos + 1..];

            // Store peer info for later lookup
            peer_info_map.insert(peer_id.to_string(), pubkey_b64.to_string());

            // Create NodeCandidate for scheduler
            let candidate = NodeCandidate {
                node_id: peer_id.to_string(),
                load_factor: 0.0, // We don't have load info, assume available
                available: true,
                capabilities: NodeCapabilities::default(),
            };
            node_candidates.push(candidate);
        } else {
            anyhow::bail!(
                "Invalid candidate format: expected 'peer_id:pubkey_b64', got '{}'",
                peer
            );
        }
    }

    debug!(
        "Parsed {} node candidates from peers",
        node_candidates.len()
    );

    // 3) Use scheduler to select candidates with round-robin strategy
    let scheduler_config = SchedulerConfig {
        strategy: SchedulingStrategy::RoundRobin,
        max_candidates: None,
        enable_load_balancing: false,
    };
    let scheduler = Scheduler::new(scheduler_config);

    debug!(
        "Using scheduler with round-robin strategy to select {} replicas from {} candidates",
        replicas,
        node_candidates.len()
    );
    let scheduling_plan = scheduler.schedule_workload(&node_candidates, replicas)?;
    debug!(
        "Scheduler selected candidate indices: {:?}",
        scheduling_plan.selected_candidates
    );

    // 4) Build selected_nodes list from scheduling plan
    let mut selected_nodes: Vec<(String, String)> = Vec::new();
    for candidate_idx in scheduling_plan.selected_candidates {
        let candidate = &node_candidates[candidate_idx];
        let node_id = &candidate.node_id;
        let pubkey_b64 = peer_info_map
            .get(node_id)
            .ok_or_else(|| anyhow::anyhow!("Missing pubkey for node: {}", node_id))?;
        selected_nodes.push((node_id.clone(), pubkey_b64.clone()));
    }

    info!(
        "Scheduled {} nodes for {} replicas using {:?} strategy: {:?}",
        selected_nodes.len(),
        replicas,
        scheduling_plan.strategy_used,
        selected_nodes.iter().map(|(id, _)| id).collect::<Vec<_>>()
    );
    debug!("Selected nodes with pubkeys: {:?}", selected_nodes);

    // 5) Create encrypted tasks for each node sequentially with same manifest_id
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let mut created_task_ids = Vec::new();

    // Create and assign tasks for each node sequentially to avoid store conflicts
    for (node_id, node_pubkey) in &selected_nodes {
        debug!("Creating encrypted task for node: {}", node_id);

        // Create a modified manifest with replicas=1 for this specific node
        let mut node_manifest = manifest_json.clone();
        if let Some(spec) = node_manifest.get_mut("spec") {
            if let Some(spec_obj) = spec.as_object_mut() {
                spec_obj.insert(
                    "replicas".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(1)),
                );
            }
        } else if manifest_json.get("replicas").is_some() {
            // Handle top-level replicas field
            if let Some(manifest_obj) = node_manifest.as_object_mut() {
                manifest_obj.insert(
                    "replicas".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(1)),
                );
            }
        }

        let node_manifest_str = serde_json::to_string(&node_manifest)?;
        debug!("Node {} will receive manifest with replicas=1", node_id);

        let node_pubkey_bytes = base64::engine::general_purpose::STANDARD
            .decode(node_pubkey)
            .map_err(|e| {
                anyhow::anyhow!("Failed to decode node public key for {}: {}", node_id, e)
            })?;

        let encrypted_blob =
            encrypt_payload_for_recipient(&node_pubkey_bytes, node_manifest_str.as_bytes())?;

        let payload_b64 = base64::engine::general_purpose::STANDARD.encode(&encrypted_blob);

        let encrypted_manifest_bytes = protocol::machine::build_encrypted_manifest(
            "",
            &payload_b64,
            "ml-kem-512",
            1,
            1,
            Some("kubernetes"),
            &[],
            ts,
            Some(node_id),
        );

        // 4) Create task with base manifest_id so nodes announce the same ID to DHT
        debug!(
            "Creating task for node {} with base manifest_id {} for DHT consistency",
            node_id, manifest_id
        );
        let create_resp = fb_client
            .create_task(
                tenant,
                &encrypted_manifest_bytes,
                Some(manifest_id.clone()),
                None,
            )
            .await?;
        debug!("Task created for node {}: {:?}", node_id, create_resp);

        // 5) Assign this specific task to its intended recipient node only
        let chosen_peers = vec![node_id.clone()];
        debug!(
            "Assigning task {} to specific node: {}",
            manifest_id, node_id
        );

        let assign_resp = fb_client
            .assign_task(tenant, &manifest_id, chosen_peers)
            .await?;
        debug!("Task assigned to node {}: {:?}", node_id, assign_resp);

        let ok = assign_resp
            .get("ok")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !ok {
            anyhow::bail!("assign failed for node {}", node_id);
        }

        created_task_ids.push(manifest_id.clone());

        // Add a small delay to avoid task store conflicts
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    debug!(
        "Successfully created and assigned tasks to {} nodes",
        created_task_ids.len()
    );

    info!(
        "Apply completed for manifest_id {} distributed to {} nodes (all will announce same ID to DHT)",
        manifest_id,
        selected_nodes.len()
    );

    Ok(manifest_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_candidate_parsing_and_scheduling() {
        // Test data in the format returned by get_candidates
        let peers = vec![
            "peer1:dGVzdF9wdWJrZXlfMQ==".to_string(),
            "peer2:dGVzdF9wdWJrZXlfMg==".to_string(),
            "peer3:dGVzdF9wdWJrZXlfMw==".to_string(),
            "peer4:dGVzdF9wdWJrZXlfNA==".to_string(),
        ];

        // Parse candidates
        let mut node_candidates = Vec::new();
        let mut peer_info_map = HashMap::new();

        for peer in &peers {
            if let Some(colon_pos) = peer.find(':') {
                let peer_id = &peer[..colon_pos];
                let pubkey_b64 = &peer[colon_pos + 1..];

                peer_info_map.insert(peer_id.to_string(), pubkey_b64.to_string());

                let candidate = NodeCandidate {
                    node_id: peer_id.to_string(),
                    load_factor: 0.0,
                    available: true,
                    capabilities: NodeCapabilities::default(),
                };
                node_candidates.push(candidate);
            }
        }

        // Test scheduler with round-robin
        let scheduler_config = SchedulerConfig {
            strategy: SchedulingStrategy::RoundRobin,
            max_candidates: None,
            enable_load_balancing: false,
        };
        let scheduler = Scheduler::new(scheduler_config);

        // Schedule 2 replicas
        let replicas = 2;
        let scheduling_plan = scheduler
            .schedule_workload(&node_candidates, replicas)
            .unwrap();

        assert_eq!(scheduling_plan.selected_candidates.len(), replicas);
        assert_eq!(scheduling_plan.total_candidates, peers.len());
        assert_eq!(
            scheduling_plan.strategy_used,
            SchedulingStrategy::RoundRobin
        );

        // Verify selected nodes can be mapped back to peer info
        for candidate_idx in &scheduling_plan.selected_candidates {
            let candidate = &node_candidates[*candidate_idx];
            assert!(peer_info_map.contains_key(&candidate.node_id));
        }
    }

    #[test]
    fn test_invalid_peer_format() {
        let peers = vec!["invalid_peer_format".to_string()];

        // This should result in a format error when parsing
        let mut node_candidates = Vec::new();
        let mut has_error = false;

        for peer in &peers {
            if let Some(colon_pos) = peer.find(':') {
                let peer_id = &peer[..colon_pos];
                let _pubkey_b64 = &peer[colon_pos + 1..];

                let candidate = NodeCandidate {
                    node_id: peer_id.to_string(),
                    load_factor: 0.0,
                    available: true,
                    capabilities: NodeCapabilities::default(),
                };
                node_candidates.push(candidate);
            } else {
                has_error = true;
            }
        }

        assert!(has_error);
        assert!(node_candidates.is_empty());
    }
}
