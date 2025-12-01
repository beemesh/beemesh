//! # Message Signature Module
//!
//! This module provides cryptographic signing and verification for all scheduler
//! messages (Tender, Bid, AwardWithManifest, SchedulerEvent) and API messages
//! (ApplyRequest, ApplyResponse).
//!
//! ## Signature Scheme
//!
//! All signatures use Ed25519 keys (from libp2p identity) with SHA-256 digest:
//!
//! 1. A "view" struct is created containing all fields to be signed (excluding signature)
//! 2. The view is serialized using deterministic bincode encoding
//! 3. SHA-256 hash is computed over the serialized view
//! 4. The 32-byte digest is signed using the Ed25519 private key
//!
//! ## Spec Reference
//!
//! - §7: All scheduler messages MUST be signed using Ed25519
//! - §7.1: Signature covers all fields except the signature field itself
//! - §7.2: Replay protection via timestamp + nonce combination

use crate::messages::{ApplyRequest, ApplyResponse, AwardWithManifest, Bid, Disposal, SchedulerEvent, Tender};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use bincode::Options;
use libp2p::identity::{Keypair, PublicKey};
use log::{debug, warn};
use serde::Serialize;
use sha2::{Digest, Sha256};

// ============================================================================
// Serialization Helpers
// ============================================================================

/// Serializes a value using deterministic bincode configuration.
///
/// Uses little-endian byte order and fixed integer encoding to ensure
/// consistent serialization across platforms and bincode versions.
fn serialize_view<T: Serialize>(value: &T) -> anyhow::Result<Vec<u8>> {
    Ok(bincode::DefaultOptions::new()
        .with_little_endian()
        .with_fixint_encoding()
        .serialize(value)?)
}

// ============================================================================
// Signature Macro
// ============================================================================

/// Generates sign and verify functions for a message type.
///
/// This macro creates two functions:
/// - `sign_$name`: Signs a mutable message, storing the signature in-place
/// - `verify_sign_$name`: Verifies a message's signature against a public key
///
/// The signing process:
/// 1. Builds a view struct from the message (all fields except signature)
/// 2. Serializes the view using deterministic bincode
/// 3. Computes SHA-256 digest of the serialized view
/// 4. Signs the 32-byte digest using Ed25519
/// 5. Stores the signature in the message's `signature` field
macro_rules! define_sign_verify {
    ($name:ident, $typ:ty, $view:ty, $builder:expr) => {
        /// Signs the message using the provided keypair.
        ///
        /// The signature is stored in the message's `signature` field.
        pub fn $name(message: &mut $typ, keypair: &Keypair) -> anyhow::Result<()> {
            let view: $view = $builder(message);
            let view_bytes = serialize_view(&view).map_err(|err| {
                anyhow::anyhow!(
                    "failed to compute digest for {}: {err}; view={:?}",
                    stringify!($typ),
                    view,
                )
            })?;
            let digest: [u8; 32] = Sha256::digest(&view_bytes).into();
            let signature = keypair
                .sign(&digest)
                .map_err(|e| anyhow::anyhow!("failed to sign {}: {}", stringify!($typ), e))?;

            let digest_b64 = BASE64.encode(digest);
            let signature_b64 = BASE64.encode(&signature);
            let view_bytes_b64 = BASE64.encode(&view_bytes);
            let public_key_peer_id = keypair.public().to_peer_id();

            debug!(
                "signature created for {}: digest_b64={}, signature_b64={}, view_bytes_b64={}, public_key_peer_id={}, view={:?}",
                stringify!($typ),
                digest_b64,
                signature_b64,
                view_bytes_b64,
                public_key_peer_id,
                view,
            );

            message.signature = signature;
            Ok(())
        }

        paste::paste! {
            /// Verifies the message's signature against the provided public key.
            ///
            /// Returns `true` if the signature is valid, `false` otherwise.
            pub fn [<verify_ $name>](message: &$typ, public_key: &PublicKey) -> bool {
                if message.signature.is_empty() {
                    let view: $view = $builder(message);
                    warn!(
                        "signature verification failed for {}: missing signature; view={:?}",
                        stringify!($typ),
                        view,
                    );
                    return false;
                }
                let view: $view = $builder(message);
                let view_bytes = match serialize_view(&view) {
                    Ok(bytes) => bytes,
                    Err(err) => {
                        warn!(
                            "signature verification failed for {}: digest error: {err}; view={:?}",
                            stringify!($typ),
                            view,
                        );
                        return false;
                    },
                };
                let digest: [u8; 32] = Sha256::digest(&view_bytes).into();
                let verified = public_key.verify(&digest, &message.signature);

                if !verified {
                    let digest_b64 = BASE64.encode(digest);
                    let signature_b64 = BASE64.encode(&message.signature);
                    let view_bytes_b64 = BASE64.encode(&view_bytes);
                    let public_key_peer_id = public_key.to_peer_id();

                    warn!(
                        "signature verification failed for {}: digest_b64={}, signature_b64={}, view_bytes_b64={}, public_key_peer_id={}, view={:?}",
                        stringify!($typ),
                        digest_b64,
                        signature_b64,
                        view_bytes_b64,
                        public_key_peer_id,
                        view,
                    );
                }

                verified
            }
        }
    };
}

// ============================================================================
// View Structs
// ============================================================================

/// Signing view for Tender messages.
///
/// Contains all fields that are signed (excludes signature).
/// Note: origin_peer is not included since the tender owner is identified
/// by the gossipsub message source (guaranteed via MessageAuthenticity::Signed).
#[derive(Serialize, Debug)]
struct TenderView {
    id: String,
    manifest_digest: String,
    qos_preemptible: bool,
    timestamp: u64,
    nonce: u64,
}

/// Signing view for Bid messages.
#[derive(Serialize, Debug)]
struct BidView {
    tender_id: String,
    node_id: String,
    score: f64,
    resource_fit_score: f64,
    network_locality_score: f64,
    timestamp: u64,
    nonce: u64,
}

/// Signing view for SchedulerEvent messages.
#[derive(Serialize, Debug)]
struct SchedulerEventView {
    tender_id: String,
    node_id: String,
    event_type: crate::messages::EventType,
    reason: String,
    timestamp: u64,
    nonce: u64,
}

/// Signing view for ApplyRequest messages.
#[derive(Serialize, Debug)]
struct ApplyRequestView {
    replicas: u32,
    operation_id: String,
    manifest_json: String,
    origin_peer: String,
}

/// Signing view for ApplyResponse messages.
#[derive(Serialize, Debug)]
struct ApplyResponseView {
    ok: bool,
    operation_id: String,
    message: String,
}

/// Signing view for AwardWithManifest messages.
///
/// Used for direct award delivery to winners (includes manifest).
/// The manifest_json is included in the signature, so integrity is verified
/// implicitly - any tampering would invalidate the signature.
#[derive(Serialize, Debug)]
struct AwardWithManifestView {
    tender_id: String,
    manifest_json: String,
    owner_peer_id: String,
    replicas: u32,
    timestamp: u64,
    nonce: u64,
}

/// Signing view for Disposal messages.
///
/// Contains all fields that are signed (excludes signature).
#[derive(Serialize, Debug)]
struct DisposalView {
    namespace: String,
    kind: String,
    name: String,
    timestamp: u64,
    nonce: u64,
}

// ============================================================================
// Sign/Verify Function Definitions
// ============================================================================

define_sign_verify!(
    sign_tender,
    Tender,
    TenderView,
    (|t: &Tender| {
        TenderView {
            id: t.id.clone(),
            manifest_digest: t.manifest_digest.clone(),
            qos_preemptible: t.qos_preemptible,
            timestamp: t.timestamp,
            nonce: t.nonce,
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
        nonce: b.nonce,
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
        nonce: e.nonce,
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

define_sign_verify!(
    sign_award_with_manifest,
    AwardWithManifest,
    AwardWithManifestView,
    (|a: &AwardWithManifest| AwardWithManifestView {
        tender_id: a.tender_id.clone(),
        manifest_json: a.manifest_json.clone(),
        owner_peer_id: a.owner_peer_id.clone(),
        replicas: a.replicas,
        timestamp: a.timestamp,
        nonce: a.nonce,
    })
);

define_sign_verify!(
    sign_disposal,
    Disposal,
    DisposalView,
    (|d: &Disposal| DisposalView {
        namespace: d.namespace.clone(),
        kind: d.kind.clone(),
        name: d.name.clone(),
        timestamp: d.timestamp,
        nonce: d.nonce,
    })
);

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::Tender;

    #[test]
    fn tender_signatures_verify_and_detect_tampering() {
        let keypair = Keypair::generate_ed25519();
        let public_key = keypair.public();

        let mut tender = Tender {
            id: "test-tender".to_string(),
            manifest_digest: "digest".to_string(),
            qos_preemptible: false,
            timestamp: 123,
            nonce: 42,
            signature: Vec::new(),
        };

        sign_tender(&mut tender, &keypair).expect("signing should succeed");
        assert!(verify_sign_tender(&tender, &public_key));

        let mut tampered = tender.clone();
        tampered.nonce += 1;
        assert!(!verify_sign_tender(&tampered, &public_key));

        let mut missing_sig = tender.clone();
        missing_sig.signature.clear();
        assert!(!verify_sign_tender(&missing_sig, &public_key));
    }
}
