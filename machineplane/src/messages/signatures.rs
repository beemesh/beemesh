use crate::messages::types::{ApplyRequest, ApplyResponse, Bid, LeaseHint, SchedulerEvent, Tender};
use libp2p::identity::{Keypair, PublicKey};
use serde::Serialize;
use sha2::{Digest, Sha256};

fn digest_message<T: Serialize>(value: &T) -> anyhow::Result<[u8; 32]> {
    let bytes = bincode::serialize(value)?;
    Ok(Sha256::digest(bytes).into())
}

macro_rules! define_sign_verify {
    ($name:ident, $typ:ty, $view:ty, $builder:expr) => {
        pub fn $name(message: &mut $typ, keypair: &Keypair) -> anyhow::Result<()> {
            let view: $view = $builder(message);
            let digest = digest_message(&view)?;
            message.signature = keypair
                .sign(&digest)
                .map_err(|e| anyhow::anyhow!("failed to sign {}: {}", stringify!($typ), e))?;
            Ok(())
        }

        paste::paste! {
            pub fn [<verify_ $name>](message: &$typ, public_key: &PublicKey) -> bool {
                if message.signature.is_empty() {
                    return false;
                }
                let view: $view = $builder(message);
                let digest = match digest_message(&view) {
                    Ok(d) => d,
                    Err(_) => return false,
                };
                public_key.verify(&digest, &message.signature)
            }
        }
    };
}

#[derive(Serialize)]
struct TenderView {
    id: String,
    manifest_ref: String,
    manifest_json: String,
    workload_type: String,
    duplicate_tolerant: bool,
    placement_token: String,
    qos_preemptible: bool,
    timestamp: u64,
}

#[derive(Serialize)]
struct BidView {
    tender_id: String,
    node_id: String,
    score: f64,
    resource_fit_score: f64,
    network_locality_score: f64,
    timestamp: u64,
}

#[derive(Serialize)]
struct SchedulerEventView {
    tender_id: String,
    node_id: String,
    event_type: super::types::EventType,
    reason: String,
    timestamp: u64,
}

#[derive(Serialize)]
struct LeaseHintView {
    tender_id: String,
    node_id: String,
    score: f64,
    ttl_ms: u32,
    renew_nonce: u64,
    timestamp: u64,
}

#[derive(Serialize)]
struct ApplyRequestView {
    replicas: u32,
    operation_id: String,
    manifest_json: String,
    origin_peer: String,
    manifest_id: String,
}

#[derive(Serialize)]
struct ApplyResponseView {
    ok: bool,
    operation_id: String,
    message: String,
}

define_sign_verify!(
    sign_tender,
    Tender,
    TenderView,
    (|t: &Tender| {
        TenderView {
            id: t.id.clone(),
            manifest_ref: t.manifest_ref.clone(),
            manifest_json: t.manifest_json.clone(),
            workload_type: t.workload_type.clone(),
            duplicate_tolerant: t.duplicate_tolerant,
            placement_token: t.placement_token.clone(),
            qos_preemptible: t.qos_preemptible,
            timestamp: t.timestamp,
        }
    })
);

define_sign_verify!(
    sign_bid,
    Bid,
    BidView,
    (|b: &Bid| BidView {
        tender_id: b.tender_id.clone(),
        node_id: b.node_id.clone(),
        score: b.score,
        resource_fit_score: b.resource_fit_score,
        network_locality_score: b.network_locality_score,
        timestamp: b.timestamp,
    })
);

define_sign_verify!(
    sign_scheduler_event,
    SchedulerEvent,
    SchedulerEventView,
    (|e: &SchedulerEvent| SchedulerEventView {
        tender_id: e.tender_id.clone(),
        node_id: e.node_id.clone(),
        event_type: e.event_type,
        reason: e.reason.clone(),
        timestamp: e.timestamp,
    })
);

define_sign_verify!(
    sign_lease_hint,
    LeaseHint,
    LeaseHintView,
    (|l: &LeaseHint| LeaseHintView {
        tender_id: l.tender_id.clone(),
        node_id: l.node_id.clone(),
        score: l.score,
        ttl_ms: l.ttl_ms,
        renew_nonce: l.renew_nonce,
        timestamp: l.timestamp,
    })
);

define_sign_verify!(
    sign_apply_request,
    ApplyRequest,
    ApplyRequestView,
    (|r: &ApplyRequest| ApplyRequestView {
        replicas: r.replicas,
        operation_id: r.operation_id.clone(),
        manifest_json: r.manifest_json.clone(),
        origin_peer: r.origin_peer.clone(),
        manifest_id: r.manifest_id.clone(),
    })
);

define_sign_verify!(
    sign_apply_response,
    ApplyResponse,
    ApplyResponseView,
    (|r: &ApplyResponse| ApplyResponseView {
        ok: r.ok,
        operation_id: r.operation_id.clone(),
        message: r.message.clone(),
    })
);
