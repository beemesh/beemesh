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
struct TenderView<'a> {
    id: &'a str,
    manifest_ref: &'a str,
    manifest_json: &'a str,
    requirements_cpu_cores: u32,
    requirements_memory_mb: u32,
    workload_type: &'a str,
    duplicate_tolerant: bool,
    max_parallel_duplicates: u32,
    placement_token: &'a str,
    qos_preemptible: bool,
    timestamp: u64,
}

#[derive(Serialize)]
struct BidView<'a> {
    tender_id: &'a str,
    node_id: &'a str,
    score: f64,
    resource_fit_score: f64,
    network_locality_score: f64,
    timestamp: u64,
}

#[derive(Serialize)]
struct SchedulerEventView<'a> {
    tender_id: &'a str,
    node_id: &'a str,
    event_type: super::types::EventType,
    reason: &'a str,
    timestamp: u64,
}

#[derive(Serialize)]
struct LeaseHintView<'a> {
    tender_id: &'a str,
    node_id: &'a str,
    score: f64,
    ttl_ms: u32,
    renew_nonce: u64,
    timestamp: u64,
}

#[derive(Serialize)]
struct ApplyRequestView<'a> {
    replicas: u32,
    operation_id: &'a str,
    manifest_json: &'a str,
    origin_peer: &'a str,
    manifest_id: &'a str,
}

#[derive(Serialize)]
struct ApplyResponseView<'a> {
    ok: bool,
    operation_id: &'a str,
    message: &'a str,
}

define_sign_verify!(sign_tender, Tender, TenderView<'_>, |t: &Tender| {
    TenderView {
        id: &t.id,
        manifest_ref: &t.manifest_ref,
        manifest_json: &t.manifest_json,
        requirements_cpu_cores: t.requirements_cpu_cores,
        requirements_memory_mb: t.requirements_memory_mb,
        workload_type: &t.workload_type,
        duplicate_tolerant: t.duplicate_tolerant,
        max_parallel_duplicates: t.max_parallel_duplicates,
        placement_token: &t.placement_token,
        qos_preemptible: t.qos_preemptible,
        timestamp: t.timestamp,
    }
});

define_sign_verify!(sign_bid, Bid, BidView<'_>, |b: &Bid| BidView {
    tender_id: &b.tender_id,
    node_id: &b.node_id,
    score: b.score,
    resource_fit_score: b.resource_fit_score,
    network_locality_score: b.network_locality_score,
    timestamp: b.timestamp,
});

define_sign_verify!(
    sign_scheduler_event,
    SchedulerEvent,
    SchedulerEventView<'_>,
    |e: &SchedulerEvent| SchedulerEventView {
        tender_id: &e.tender_id,
        node_id: &e.node_id,
        event_type: e.event_type,
        reason: &e.reason,
        timestamp: e.timestamp,
    }
);

define_sign_verify!(
    sign_lease_hint,
    LeaseHint,
    LeaseHintView<'_>,
    |l: &LeaseHint| LeaseHintView {
        tender_id: &l.tender_id,
        node_id: &l.node_id,
        score: l.score,
        ttl_ms: l.ttl_ms,
        renew_nonce: l.renew_nonce,
        timestamp: l.timestamp,
    }
);

define_sign_verify!(
    sign_apply_request,
    ApplyRequest,
    ApplyRequestView<'_>,
    |r: &ApplyRequest| ApplyRequestView {
        replicas: r.replicas,
        operation_id: &r.operation_id,
        manifest_json: &r.manifest_json,
        origin_peer: &r.origin_peer,
        manifest_id: &r.manifest_id,
    }
);

define_sign_verify!(
    sign_apply_response,
    ApplyResponse,
    ApplyResponseView<'_>,
    |r: &ApplyResponse| ApplyResponseView {
        ok: r.ok,
        operation_id: &r.operation_id,
        message: &r.message,
    }
);
