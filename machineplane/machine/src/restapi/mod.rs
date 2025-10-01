use crate::pod_communication;
use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use protocol::libp2p_constants::{
    FREE_CAPACITY_PREFIX, FREE_CAPACITY_TIMEOUT_SECS, REPLICAS_FIELD, SPEC_REPLICAS_FIELD,
};
use serde::{Serialize};
use tokio::sync::mpsc;
use tokio::{sync::watch, time::Duration};
use log::{info, debug};

#[derive(Serialize)]
pub struct NodesResponse {
    pub peers: Vec<String>,
}

async fn get_nodes(State(state): State<RestState>) -> Json<NodesResponse> {
    let peers = state.peer_rx.borrow().clone();
    Json(NodesResponse { peers })
}

#[derive(Clone)]
pub struct RestState {
    pub peer_rx: watch::Receiver<Vec<String>>,
    pub control_tx: mpsc::UnboundedSender<crate::libp2p_beemesh::control::Libp2pControl>,
}

pub fn build_router(
    peer_rx: watch::Receiver<Vec<String>>,
    control_tx: mpsc::UnboundedSender<crate::libp2p_beemesh::control::Libp2pControl>,
) -> Router {
    let state = RestState {
        peer_rx,
        control_tx,
    };
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/tenant/{tenant}/apply", post(apply_manifest))
        .route("/tenant/{tenant}/distribute_shares", post(distribute_shares))
        .route("/tenant/{tenant}/nodes", get(get_nodes))
        // state
        .with_state(state)
}

pub async fn apply_manifest(
    Path(tenant): Path<String>,
    State(state): State<RestState>,
    Json(manifest): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    debug!("tenant: {:?}", tenant);

    // The CLI now sends a wrapper containing two envelopes: "manifest_envelope" and "shares_envelope".
    // Try to decode that wrapper first.
    let manifest = if let Some(wrapper) = manifest.get("manifest_envelope") {
        // parse manifest_envelope as Envelope Value and verify
        match serde_json::from_value::<protocol::json::Envelope>(wrapper.clone()) {
            Ok(env) => {
                if env.sig.is_some() && env.pubkey.is_some() {
                    match env
                        .to_value()
                        .and_then(|v| crate::libp2p_beemesh::security::verify_envelope_and_check_nonce(&v).map(|(p, _pub, _sig)| p))
                    {
                        Ok(payload_bytes) => match std::str::from_utf8(&payload_bytes) {
                            Ok(s) => match serde_json::from_str::<serde_json::Value>(s) {
                                Ok(v) => v,
                                Err(e) => {
                                    log::warn!("apply_manifest: envelope payload not JSON: {:?}", e);
                                    return Json(serde_json::json!({
                                        "ok": false,
                                        "error": "envelope payload not JSON; decryption required on server"
                                    }));
                                }
                            },
                            Err(_) => {
                                return Json(serde_json::json!({
                                    "ok": false,
                                    "error": "envelope payload not valid UTF-8; decryption required"
                                }));
                            }
                        },
                        Err(e) => {
                            log::warn!("apply_manifest: envelope verification failed: {:?}", e);
                            return Json(serde_json::json!({
                                "ok": false,
                                "error": "envelope verification failed"
                            }));
                        }
                    }
                } else {
                    return Json(serde_json::json!({"ok": false, "error": "manifest_envelope missing signature"}));
                }
            }
            Err(e) => {
                log::warn!("apply_manifest: manifest_envelope not parseable as Envelope: {:?}", e);
                return Json(serde_json::json!({"ok": false, "error": "invalid manifest_envelope"}));
            }
        }
    } else if manifest.get("sig").is_some() && manifest.get("pubkey").is_some() {
        // older single-envelope format
        match crate::libp2p_beemesh::security::verify_envelope_and_check_nonce(&manifest) {
            Ok((payload_bytes, _pubkey_s, _sig_s)) => {
                match std::str::from_utf8(&payload_bytes) {
                    Ok(s) => match serde_json::from_str::<serde_json::Value>(s) {
                        Ok(v) => v,
                        Err(e) => {
                            log::warn!("apply_manifest: envelope payload not JSON: {:?}", e);
                            return Json(serde_json::json!({
                                "ok": false,
                                "error": "envelope payload not JSON; decryption required on server"
                            }));
                        }
                    },
                    Err(_) => {
                        return Json(serde_json::json!({
                            "ok": false,
                            "error": "envelope payload not valid UTF-8; decryption required"
                        }));
                    }
                }
            }
            Err(e) => {
                log::warn!("apply_manifest: envelope verification failed: {:?}", e);
                return Json(serde_json::json!({
                    "ok": false,
                    "error": "envelope verification failed"
                }));
            }
        }
    } else {
        // Raw manifest JSON
        manifest
    };

    // If a shares_envelope was provided, parse and verify it for bookkeeping/forwarding
    if let Some(wrapper) = manifest.get("shares_envelope") {
        match serde_json::from_value::<protocol::json::Envelope>(wrapper.clone()) {
            Ok(sh_env) => {
                if sh_env.sig.is_some() && sh_env.pubkey.is_some() {
                    match sh_env.to_value().and_then(|v| crate::libp2p_beemesh::security::verify_envelope_and_check_nonce(&v)) {
                        Ok((_payload_bytes, _pub, _sig)) => {
                            // payload here is the shares payload (json with base64 shares). We don't
                            // need to decode the shares at this layer; pod_communication or DHT storage
                            // can consume the shares_envelope directly if needed. For now we just log.
                            debug!("apply_manifest: received verified shares_envelope");
                        }
                        Err(e) => {
                            log::warn!("apply_manifest: shares_envelope verification failed: {:?}", e);
                        }
                    }
                } else {
                    log::warn!("apply_manifest: shares_envelope missing signature/pubkey");
                }
            }
            Err(e) => {
                log::warn!("apply_manifest: shares_envelope not parseable as Envelope: {:?}", e);
            }
        }
    }

    // determine desired replica count from manifest; check top-level `replicas` or `spec.replicas`
    let replicas = manifest
        .get(REPLICAS_FIELD)
        .and_then(|v| v.as_u64())
        .or_else(|| {
            manifest
                .get(SPEC_REPLICAS_FIELD)
                .and_then(|s| s.get("replicas"))
                .and_then(|r| r.as_u64())
        })
        .unwrap_or(1) as usize;

    // publish a QueryCapacity control message to the libp2p task and collect replies
    let request_id = format!("{}-{}", FREE_CAPACITY_PREFIX, uuid::Uuid::new_v4());
    // build a sample flatbuffer CapacityRequest (sample values for now)
    let capacity_fb = protocol::machine::build_capacity_request(
        500u32,                    // cpu_milli
        512u64 * 1024 * 1024,      // memory_bytes (512MB)
        10u64 * 1024 * 1024 * 1024, // storage_bytes (10GB)
        replicas as u32,          // replicas
    );
    info!("apply_manifest: request_id={}, replicas={} payload_bytes={}", request_id, replicas, capacity_fb.len());
    let (reply_tx, mut reply_rx) = mpsc::unbounded_channel::<String>();
    let _ = state
        .control_tx
        .send(crate::libp2p_beemesh::control::Libp2pControl::QueryCapacityWithPayload {
            request_id: request_id.clone(),
            reply_tx: reply_tx.clone(),
            payload: capacity_fb,
        });

    // collect replies for the configured timeout (or until we have enough responders)
    let mut responders: Vec<String> = Vec::new();
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(FREE_CAPACITY_TIMEOUT_SECS) {
        let remaining =
            Duration::from_secs(FREE_CAPACITY_TIMEOUT_SECS).saturating_sub(start.elapsed());
        match tokio::time::timeout(remaining, reply_rx.recv()).await {
            Ok(Some(peer)) => {
                if !responders.contains(&peer) {
                    responders.push(peer);
                }
                if responders.len() >= replicas {
                    break;
                }
            }
            _ => break, // timeout or closed
        }
    }

    info!("apply_manifest: collected {} responders", responders.len());

    let mut per_peer = serde_json::Map::new();

    // pick up to `replicas` peers from responders
    let assigned: Vec<String> = responders.into_iter().take(replicas).collect();
    if assigned.len() == 0 {
        return Json(serde_json::json!({
            "ok": false,
            "tenant": tenant,
            "replicas_requested": replicas,
            "assigned_peers": assigned,
            "per_peer": serde_json::Value::Object(per_peer),
        }));
    }

    // dispatch manifest to each assigned peer (stubbed)
    for peer in &assigned {
        match pod_communication::send_apply_to_peer(peer, &manifest, &state.control_tx).await {
            Ok(_) => {
                per_peer.insert(peer.clone(), serde_json::Value::String("ok".to_string()));
            }
            Err(e) => {
                per_peer.insert(
                    peer.clone(),
                    serde_json::Value::String(format!("error: {}", e)),
                );
            }
        }
    }

    Json(serde_json::json!({
        "ok": true,
        "tenant": tenant,
        "replicas_requested": replicas,
        "assigned_peers": assigned,
        "per_peer": serde_json::Value::Object(per_peer),
    }))
}

#[derive(serde::Deserialize)]
struct ShareTarget {
    peer_id: String,
    /// Arbitrary JSON payload (the CLI should have encrypted the share for the recipient)
    payload: serde_json::Value,
}

pub async fn distribute_shares(
    Path(_tenant): Path<String>,
    State(state): State<RestState>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    // Expect a signed shares_envelope and a list of targets
    let shares_env_val = match body.get("shares_envelope") {
        Some(v) => v.clone(),
        None => return Json(serde_json::json!({"ok": false, "error": "missing shares_envelope"})),
    };

    // Verify the shares envelope signature
    let shares_env = match serde_json::from_value::<protocol::json::Envelope>(shares_env_val.clone()) {
        Ok(env) => env,
        Err(e) => {
            log::warn!("distribute_shares: shares_envelope not parseable as Envelope: {:?}", e);
            return Json(serde_json::json!({"ok": false, "error": "invalid shares_envelope"}));
        }
    };

    if shares_env.sig.is_none() || shares_env.pubkey.is_none() {
        return Json(serde_json::json!({"ok": false, "error": "shares_envelope missing signature"}));
    }

    if let Err(e) = shares_env.to_value().and_then(|v| crate::libp2p_beemesh::security::verify_envelope_and_check_nonce(&v)) {
        log::warn!("distribute_shares: shares_envelope verification failed: {:?}", e);
        return Json(serde_json::json!({"ok": false, "error": "shares_envelope verification failed"}));
    }

    // Parse targets
    let mut results = serde_json::Map::new();
    if let Some(targets_val) = body.get("targets") {
        if let Ok(targets) = serde_json::from_value::<Vec<ShareTarget>>(targets_val.clone()) {
            for t in targets {
                // parse peer id
                match t.peer_id.parse::<libp2p::PeerId>() {
                    Ok(peer_id) => {
                        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<Result<(), String>>();
                        let _ = state.control_tx.send(crate::libp2p_beemesh::control::Libp2pControl::SendKeyShare {
                            peer_id,
                            share_payload: t.payload.clone(),
                            reply_tx: tx,
                        });
                        // For now we don't wait on reply channel; assume success
                        results.insert(t.peer_id.clone(), serde_json::Value::String("dispatched".to_string()));
                    }
                    Err(e) => {
                        results.insert(t.peer_id.clone(), serde_json::Value::String(format!("invalid peer id: {}", e)));
                    }
                }
            }
        } else {
            return Json(serde_json::json!({"ok": false, "error": "invalid targets"}));
        }
    } else {
        return Json(serde_json::json!({"ok": false, "error": "missing targets"}));
    }

    Json(serde_json::json!({"ok": true, "results": serde_json::Value::Object(results)}))
}
