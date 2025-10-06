use std::time::Duration;
use std::path::PathBuf;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use tokio::time::sleep;
use tokio::sync::mpsc;
use base64::engine::general_purpose;
use base64::Engine as _;

use machine::libp2p_beemesh;
use machine::libp2p_beemesh::behaviour::apply_message::process_self_apply_request;
use crypto;
use protocol;

const TENANT: &str = "00000000-0000-0000-0000-000000000000";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn in_process_apply_decrypt_flow_ephemeral_keys() {
    // Configure ephemeral shared keystore for this test node
    std::env::set_var("BEEMESH_KEYSTORE_EPHEMERAL", "1");
    // Use a deterministic shared name for the ephemeral keystore during tests
    let shared_name = format!("test_node_in_memory_apply");
    std::env::set_var("BEEMESH_KEYSTORE_SHARED_NAME", &shared_name);
    libp2p_beemesh::set_keystore_shared_name(Some(shared_name.clone()));

    // Initialize PQC layer
    crypto::ensure_pqc_init().expect("pqc init failed");

    // Use an ephemeral signing keypair (kept in-memory) for signing envelopes in the test.
    // ensure_keypair_ephemeral returns (pub_bytes, priv_bytes)
    let (epub, esk) = crypto::ensure_keypair_ephemeral().expect("ephemeral keypair");

    // Ensure a KEM keypair exists on disk for encrypt_share_for_keystore usage
    // This helper writes to $HOME/.beemesh for the KEM keypair if needed.
    let (_kem_pub, _kem_priv) = crypto::ensure_kem_keypair_on_disk().expect("ensure_kem_keypair_on_disk");

    // Start a local libp2p swarm (used for in-process DHT/local peer id)
    let (mut swarm, _topic, _peer_rx, _peer_tx) = libp2p_beemesh::setup_libp2p_node().expect("setup_libp2p_node failed");

    // Read manifest file from tests folder (same as integration test)
    let manifest_path = PathBuf::from(format!("{}/../tests/sample_manifests/nginx", env!("CARGO_MANIFEST_DIR")));
    let manifest_contents = tokio::fs::read_to_string(&manifest_path).await.expect("read manifest file");
    let manifest_json: serde_json::Value = match serde_yaml::from_str(&manifest_contents) {
        Ok(v) => v,
        Err(_) => serde_json::json!({ "raw": manifest_contents.clone() }),
    };

    // Encrypt manifest (same as CLI) and prepare envelope
    let (ciphertext, nonce_bytes_vec, sym_key, _nonce_arr) = crypto::encrypt_manifest(&manifest_json).expect("encrypt_manifest failed");
    let manifest_envelope = serde_json::json!({
        "payload": general_purpose::STANDARD.encode(&ciphertext),
        "nonce": general_purpose::STANDARD.encode(&nonce_bytes_vec),
        "shares_meta": { "n": 3, "k": 2, "count": 3 },
    });
    let manifest_envelope_bytes = serde_json::to_vec(&manifest_envelope).expect("serialize manifest envelope");

    // Sign envelope using ephemeral signing key (pass sk then pk per sign_envelope API)
    let (sig_b64, pub_b64) = crypto::sign_envelope(&esk, &epub, &manifest_envelope_bytes).expect("sign_envelope failed");
    let mut manifest_envelope_signed = manifest_envelope;
    manifest_envelope_signed["sig"] = serde_json::Value::String(format!("ml-dsa-65:{}", sig_b64));
    manifest_envelope_signed["pubkey"] = serde_json::Value::String(pub_b64.clone());
    let manifest_envelope_signed_str = serde_json::to_string(&manifest_envelope_signed).expect("signed envelope string");

    // Split symmetric key into shares (n=3, k=2)
    let shares = crypto::split_symmetric_key(&sym_key, 3usize, 2usize);

    // Compute operation_id and manifest_id deterministically the same way apply processing does
    let operation_id = uuid::Uuid::new_v4().to_string();
    let mut hasher = DefaultHasher::new();
    TENANT.hash(&mut hasher);
    operation_id.hash(&mut hasher);
    manifest_envelope_signed_str.hash(&mut hasher);
    let manifest_id = format!("{:x}", hasher.finish());

    // Open the shared ephemeral keystore and store just one local share with meta = manifest_id
    // (leave at least one share remote so the code exercises the distributed fetch path)
    let ks = crypto::open_keystore_with_shared_name(&shared_name).expect("open_keystore failed");
    if let Some(first_share) = shares.get(0) {
        let (blob, cid) = crypto::encrypt_share_for_keystore(&first_share).expect("encrypt_share_for_keystore failed");
        ks.put(&cid, &blob, Some(&manifest_id)).expect("keystore put failed for share");
    }

    // Build a capability envelope (signed) and store it in the keystore under meta "capability:<manifest_id>"
    let ts_millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let token_obj = serde_json::json!({
        "task_id": manifest_id,
        "issuer": swarm.local_peer_id().to_string(),
        "required_quorum": 1,
        "caveats": { "authorized_peer": swarm.local_peer_id().to_string() },
        "ts": ts_millis,
    });
    let token_bytes = serde_json::to_vec(&token_obj).expect("token serialize");
    let mut env_map = serde_json::Map::new();
    env_map.insert("payload".to_string(), serde_json::Value::String(general_purpose::STANDARD.encode(&token_bytes)));
    env_map.insert("manifest_id".to_string(), serde_json::Value::String(manifest_id.clone()));
    env_map.insert("type".to_string(), serde_json::Value::String("capability".to_string()));
    let env_value = serde_json::Value::Object(env_map);
    let env_bytes = serde_json::to_vec(&env_value).expect("env bytes");

    // Sign capability envelope using ephemeral signing key
    let (cap_sig_b64, cap_pub_b64) = crypto::sign_envelope(&esk, &epub, &env_bytes).expect("sign_envelope token");
    let mut signed_env_obj = env_value.as_object().cloned().expect("env obj");
    signed_env_obj.insert("sig".to_string(), serde_json::Value::String(format!("ml-dsa-65:{}", cap_sig_b64)));
    signed_env_obj.insert("pubkey".to_string(), serde_json::Value::String(cap_pub_b64));
    let signed_bytes = serde_json::to_vec(&serde_json::Value::Object(signed_env_obj)).expect("signed env bytes");

    // Encrypt signed capability envelope for keystore storage using KEM (encrypt_share_for_keystore)
    let (cap_blob, cap_cid) = crypto::encrypt_share_for_keystore(&signed_bytes).expect("encrypt_share_for_keystore(cap)");
    let meta_cap = format!("capability:{}", manifest_id);
    ks.put(&cap_cid, &cap_blob, Some(&meta_cap)).expect("keystore put failed for capability");

    // --- Setup a mock control handler so decrypt_with_id can discover providers and fetch a remote share ---
    // Create a control channel and register it globally so the code under test can send control messages.
    use machine::libp2p_beemesh::control::Libp2pControl;
    let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel::<Libp2pControl>();
    // Install the control sender once for the libp2p helpers to find
    libp2p_beemesh::set_control_sender(ctrl_tx.clone());

    // Prepare a remote share (use the second share) that our mocked peers will return on fetch
    let remote_share = shares.get(1).cloned().expect("expected second share");
    let local_peerid = swarm.local_peer_id().clone();

    // Spawn a task to handle control messages and respond to FindManifestHolders / FetchKeyshare
    tokio::spawn(async move {
        while let Some(msg) = ctrl_rx.recv().await {
            match msg {
                Libp2pControl::FindManifestHolders { manifest_id: _mid, reply_tx } => {
                    // Return a provider list that includes a remote (fake) peer id so the
                    // code takes the remote-fetch path (which triggers the FetchKeyshare control message).
                    // Also include the local peer id as a fallback.
                    let fake_peer = libp2p::PeerId::random();
                    let _ = reply_tx.send(vec![fake_peer.clone(), local_peerid.clone()]);
                }
                Libp2pControl::FetchKeyshare { peer_id: _peer, request_fb: _req, reply_tx } => {
                    // Build a KeyShareResponse containing the base64-encoded remote share and return it
                    let share_b64 = base64::engine::general_purpose::STANDARD.encode(&remote_share);
                    let resp = protocol::machine::build_keyshare_response(true, "fetch", &share_b64);
                    let _ = reply_tx.send(Ok(resp));
                }
                Libp2pControl::GetConnectedPeers { reply_tx } => {
                    let _ = reply_tx.send(vec![local_peerid.clone()]);
                }
                // For other control messages, just ignore / no-op
                _ => {
                    // No-op for this test
                }
            }
        }
    });

    // Build apply FlatBuffer using the signed manifest envelope string as manifest_json
    let local_peer = swarm.local_peer_id().to_string();
    let apply_fb = protocol::machine::build_apply_request(
        1u32,
        TENANT,
        &operation_id,
        &manifest_envelope_signed_str,
        &local_peer,
    );

    // Directly process self-apply (bypassing control/send_apply_request) which stores manifest in DHT
    process_self_apply_request(&apply_fb, &mut swarm);

    // Poll restapi decrypted manifests map for the manifest_id
    let mut found = false;
    for _ in 0..30 {
        let decrypted_map = machine::restapi::get_decrypted_manifests_map().await;
        if let Some(entry) = decrypted_map.get(&manifest_id) {
            // Normalize the decrypted entry to a JSON value we can compare deterministically
            let actual_value_opt: Option<serde_json::Value> = if let Some(raw) = entry.get("raw").and_then(|v| v.as_str()) {
                // Try direct string equality first
                if raw == manifest_contents {
                    Some(serde_json::json!({ "raw": raw }))
                } else {
                    // Try parsing the raw YAML into JSON value for structural comparison
                    match serde_yaml::from_str::<serde_json::Value>(raw) {
                        Ok(parsed) => Some(parsed),
                        Err(_) => Some(serde_json::json!({ "raw": raw })),
                    }
                }
            } else {
                Some(entry.clone())
            };

            if let Some(actual_value) = actual_value_opt {
                // Compare canonical JSON string representations for deterministic equality
                if let (Ok(a_str), Ok(e_str)) = (
                    serde_json::to_string(&actual_value),
                    serde_json::to_string(&manifest_json),
                ) {
                    if a_str == e_str {
                        found = true;
                        break;
                    } else {
                        eprintln!("decrypted manifest mismatch: expected {} got {}", e_str, a_str);
                    }
                }
            }
        }
        sleep(Duration::from_millis(300)).await;
    }

    assert!(found, "decrypted manifest did not appear or did not match expected");

    // Cleanup env
    std::env::remove_var("BEEMESH_KEYSTORE_EPHEMERAL");
    std::env::remove_var("BEEMESH_KEYSTORE_SHARED_NAME");
}
