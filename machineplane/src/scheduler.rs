//! Decentralized Scheduler for Machineplane
//!
//! This module implements the **Tender→Bid→Award** scheduling flow as specified in
//! `machineplane-spec.md` §4. It provides **at-least-once scheduling** with
//! **deterministic winner selection** under A/P (Availability/Partition-Tolerance)
//! semantics.
//!
//! # Architecture
//!
//! The scheduler operates as a state machine per tender:
//!
//! ```text
//! [Idle] ──TenderReceived──► [Bidding] ──AwardReceived──► [Deploying] ──DeployOK──► [Running]
//!                                │                              │
//!                                └──NotEligible──► [Idle]       └──DeployFail──► [Idle]
//! ```
//!
//! # Message Flow
//!
//! 1. **Tender**: Published by a producer (e.g., kubectl apply). Contains manifest digest
//!    but NOT the manifest itself. All nodes receive via gossipsub.
//!
//! 2. **Bid**: Each eligible node computes a score and submits a signed bid within
//!    the selection window (~250ms ± jitter).
//!
//! 3. **Award**: The tender owner collects bids, deterministically selects winners,
//!    and publishes the award. Manifest is then sent only to winners via secure stream.
//!
//! 4. **Event**: Winners emit `Deployed`/`Failed` events after attempting deployment.
//!
//! # Security
//!
//! - All messages are Ed25519-signed (see `messages/signatures.rs`)
//! - Replay protection via `(tender_id, nonce)` tuples with 5-minute window
//! - Timestamp freshness enforced (±30 seconds max clock skew)
//!
//! # References
//!
//! - Spec: `machineplane-spec.md` §3 (Protocols), §4 (Scheduling), §6 (Failure Handling)

use crate::messages::constants::{BEEMESH_FABRIC, DEFAULT_SELECTION_WINDOW_MS};
use crate::messages::types::{
    Award, Bid, EventType, ManifestTransfer, SchedulerEvent, SchedulerMessage, Tender,
};
use crate::messages::{machine, signatures};
use crate::network::utils::peer_id_to_public_key;
use libp2p::gossipsub;
use libp2p::identity::Keypair;
use log::{error, info};
use rand::RngCore;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio::time::sleep;

// ============================================================================
// Constants
// ============================================================================

/// Maximum allowed clock skew between nodes (spec §3.3).
/// Messages with timestamps outside this window are rejected.
const MAX_CLOCK_SKEW_MS: u64 = 30_000;

/// Duration to retain seen message tuples for replay protection (spec §3.3).
/// After this window, a replayed message would be accepted again.
const REPLAY_WINDOW_MS: u64 = 300_000;

// ============================================================================
// Global State (Process-Scoped Caches)
// ============================================================================

/// Cache of manifest JSON content keyed by tender ID.
///
/// When a node publishes a tender, it stores the manifest here so it can be
/// distributed to winners after the award. This is NOT persisted to disk—the
/// Machineplane is stateless (spec §1).
static LOCAL_MANIFEST_CACHE: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Maps tender IDs to their user-facing manifest identifiers (Kube resource hash).
///
/// This allows the scheduler to track which Kubernetes resource (by manifest_id)
/// corresponds to each tender, enabling proper cleanup on delete operations.
static TENDER_MANIFEST_IDS: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Maps workload IDs to the peer that deployed them.
///
/// Used in multi-node in-process tests to verify which node deployed which
/// workload. In production, this provides debug introspection.
static WORKLOAD_PEER_MAP: LazyLock<tokio::sync::RwLock<HashMap<String, String>>> =
    LazyLock::new(|| tokio::sync::RwLock::new(HashMap::new()));

// ============================================================================
// Workload-Peer Tracking (Debug/Test Support)
// ============================================================================

/// Records which peer deployed a workload.
///
/// Called after successful deployment to enable debug endpoints like
/// `/debug/workloads_by_peer/{peer_id}` to show workload distribution.
pub async fn record_workload_peer(workload_id: &str, peer_id: &str) {
    let mut map = WORKLOAD_PEER_MAP.write().await;
    map.insert(workload_id.to_string(), peer_id.to_string());
}

/// Returns the peer ID that deployed a given workload, if known.
pub async fn get_workload_peer(workload_id: &str) -> Option<String> {
    let map = WORKLOAD_PEER_MAP.read().await;
    map.get(workload_id).cloned()
}

/// Returns all workload IDs deployed by a specific peer.
///
/// Useful for debug endpoints and testing multi-node workload distribution.
pub async fn get_workloads_by_peer(peer_id: &str) -> Vec<String> {
    let map = WORKLOAD_PEER_MAP.read().await;
    map.iter()
        .filter_map(|(wid, pid)| {
            if pid == peer_id {
                Some(wid.clone())
            } else {
                None
            }
        })
        .collect()
}

// ============================================================================
// Scheduler Core
// ============================================================================

/// The decentralized scheduler that manages the Tender→Bid→Award lifecycle.
///
/// Each node runs one `Scheduler` instance that:
/// - Listens for tenders on the `beemesh-fabric` gossipsub topic
/// - Evaluates eligibility and submits bids for tenders it can fulfill
/// - For owned tenders, collects bids and selects winners deterministically
/// - Emits deployment events after workload operations
///
/// # Concurrency
///
/// The scheduler uses `Arc<Mutex<_>>` for shared state to support concurrent
/// message handling from the libp2p event loop. The owned tenders map tracks
/// active bid collection windows.
///
/// # Example
///
/// ```ignore
/// let scheduler = Scheduler::new(local_peer_id, keypair, outbound_tx);
/// scheduler.handle_message(&topic_hash, &message).await;
/// ```
pub struct Scheduler {
    /// This node's libp2p PeerId as a string (e.g., "12D3KooW...").
    local_node_id: String,

    /// Ed25519 keypair for signing outbound scheduler messages.
    keypair: Keypair,

    /// Tenders originated by this node, awaiting bid collection.
    ///
    /// Key: tender_id, Value: context including received bids and manifest.
    /// Entries are removed after the selection window closes and winners are chosen.
    owned_tenders: Arc<Mutex<HashMap<String, OwnedTenderContext>>>,

    /// Replay filter for tenders: `(tender_id, nonce) -> first_seen_timestamp`.
    ///
    /// Prevents processing the same tender multiple times within REPLAY_WINDOW_MS.
    seen_tenders: Arc<Mutex<HashMap<(String, u64), u64>>>,

    /// Replay filter for bids: `(tender_id, node_id, nonce) -> first_seen_timestamp`.
    seen_bids: Arc<Mutex<HashMap<(String, String, u64), u64>>>,

    /// Replay filter for awards: `(tender_id, nonce) -> first_seen_timestamp`.
    seen_awards: Arc<Mutex<HashMap<(String, u64), u64>>>,

    /// Replay filter for events: `(tender_id, node_id, nonce) -> first_seen_timestamp`.
    seen_events: Arc<Mutex<HashMap<(String, String, u64), u64>>>,

    /// Channel to send commands (Publish, Deploy, SendManifest) to the network loop.
    outbound_tx: mpsc::UnboundedSender<SchedulerCommand>,
}

// ============================================================================
// Internal Bid Tracking Structures
// ============================================================================

/// Tracks collected bids for a tender during the selection window.
struct BidContext {
    /// User-facing manifest identifier (hash of Kube resource).
    manifest_id: String,
    /// Content hash of the manifest (for integrity verification).
    manifest_digest: String,
    /// Bids received during the selection window.
    bids: Vec<BidEntry>,
}

/// A single bid entry from a competing node.
#[derive(Clone)]
struct BidEntry {
    /// PeerId of the bidding node.
    bidder_id: String,
    /// Computed score (higher is better).
    score: f64,
}

/// Full context for a tender owned by this node.
struct OwnedTenderContext {
    /// Bid collection context.
    bid_context: BidContext,
    /// The manifest JSON content (kept local until award).
    manifest_json: Option<String>,
}

// ============================================================================
// Scheduler Commands (Outbound to Network Loop)
// ============================================================================

/// Commands emitted by the scheduler for the libp2p network loop to process.
///
/// The scheduler cannot directly access the libp2p swarm, so it sends these
/// commands through a channel to the network event loop which executes them.
#[derive(Debug, Clone)]
pub enum SchedulerCommand {
    /// Publish a message to a gossipsub topic (bids, awards, events).
    Publish {
        /// Topic name (always `beemesh-fabric` for scheduler messages).
        topic: String,
        /// Bincode-encoded `SchedulerMessage` payload.
        payload: Vec<u8>,
    },

    /// Deploy a workload locally after winning a tender.
    DeployWorkload {
        /// The tender that was won.
        tender_id: String,
        /// User-facing manifest identifier for the Kube resource.
        manifest_id: String,
        /// Full manifest JSON content.
        manifest_json: String,
        /// Number of replicas to deploy (usually 1 per node).
        replicas: u32,
    },

    /// Send manifest content to an awarded winner via request-response protocol.
    SendManifest {
        /// PeerId of the winning node to send the manifest to.
        peer_id: String,
        /// Bincode-encoded `ManifestTransfer` payload.
        payload: Vec<u8>,
    },
}

// ============================================================================
// Scheduler Implementation
// ============================================================================

impl Scheduler {
    /// Creates a new scheduler instance for the given node.
    ///
    /// # Arguments
    ///
    /// * `local_node_id` - This node's PeerId as a string
    /// * `keypair` - Ed25519 keypair for signing messages
    /// * `outbound_tx` - Channel to send commands to the network loop
    pub fn new(
        local_node_id: String,
        keypair: Keypair,
        outbound_tx: mpsc::UnboundedSender<SchedulerCommand>,
    ) -> Self {
        Self {
            local_node_id,
            keypair,
            owned_tenders: Arc::new(Mutex::new(HashMap::new())),
            seen_tenders: Arc::new(Mutex::new(HashMap::new())),
            seen_bids: Arc::new(Mutex::new(HashMap::new())),
            seen_awards: Arc::new(Mutex::new(HashMap::new())),
            seen_events: Arc::new(Mutex::new(HashMap::new())),
            outbound_tx,
        }
    }

    /// Routes an incoming gossipsub message to the appropriate handler.
    ///
    /// Messages on the `beemesh-fabric` topic are decoded and dispatched:
    /// - `Tender` → `handle_tender()` (evaluate and bid)
    /// - `Bid` → `handle_bid()` (collect if we're the tender owner)
    /// - `Award` → `handle_award()` (check if we won)
    /// - `Event` → `handle_event()` (observe deployment status)
    ///
    /// Messages on other topics are silently ignored (spec §2.1 requires
    /// all scheduler payloads on `beemesh-fabric` only).
    pub async fn handle_message(
        &self,
        topic_hash: &gossipsub::TopicHash,
        message: &gossipsub::Message,
    ) {
        let fabric_topic = gossipsub::IdentTopic::new(BEEMESH_FABRIC).hash();
        if *topic_hash != fabric_topic {
            return;
        }

        match machine::decode_scheduler_message(&message.data) {
            Ok(SchedulerMessage::Tender(tender)) => {
                self.handle_tender(&tender, message).await;
            }
            Ok(SchedulerMessage::Bid(bid)) => {
                self.handle_bid(&bid, message).await;
            }
            Ok(SchedulerMessage::Event(event)) => {
                self.handle_event(&event, message).await;
            }
            Ok(SchedulerMessage::Award(award)) => {
                self.handle_award(&award, message).await;
            }
            Err(e) => error!("Failed to parse scheduler message: {}", e),
        }
    }

    // ========================================================================
    // Tender Handling
    // ========================================================================

    /// Processes an incoming tender message.
    ///
    /// # Flow (spec §4.1)
    ///
    /// 1. Verify the message has a source peer (required for signature verification)
    /// 2. Check timestamp freshness (reject if > MAX_CLOCK_SKEW_MS from now)
    /// 3. Check replay filter (reject if seen within REPLAY_WINDOW_MS)
    /// 4. Verify Ed25519 signature
    /// 5. If we're the tender owner: initialize bid collection, skip bidding
    /// 6. If we're a potential bidder: evaluate eligibility, compute score, submit bid
    ///
    /// # Security
    ///
    /// - Unsigned or invalid-signature tenders are discarded (spec §3.3)
    /// - Replayed tenders are ignored to prevent duplicate work
    async fn handle_tender(&self, tender: &Tender, message: &gossipsub::Message) {
        let tender_id = tender.id.clone();
        info!("Received Tender: {}", tender_id);
        println!("tender:received id={}", tender_id);

        // Extract source peer for signature verification
        let source_peer = match message.source.as_ref() {
            Some(peer) => peer,
            None => {
                error!("Discarding tender {}: missing source peer", tender_id);
                eprintln!("tender:error id={} reason=missing_source", tender_id);
                return;
            }
        };

        // Timestamp freshness check (spec §3.3)
        if !is_timestamp_fresh(tender.timestamp) {
            error!(
                "Discarding tender {}: timestamp outside allowed skew",
                tender_id
            );
            eprintln!("tender:error id={} reason=stale_timestamp", tender_id);
            return;
        }

        // Replay protection (spec §3.3): reject if we've seen this (tender_id, nonce) recently
        if !self.mark_tender_seen(&tender_id, tender.nonce) {
            info!("Ignoring replayed tender {}", tender_id);
            eprintln!("tender:replay id={}", tender_id);
            return;
        }

        // Derive public key from source peer for signature verification
        let public_key = match peer_id_to_public_key(source_peer) {
            Some(key) => key,
            None => {
                error!(
                    "Discarding tender {}: unable to derive public key",
                    tender_id
                );
                eprintln!("tender:error id={} reason=no_public_key", tender_id);
                return;
            }
        };

        // Signature verification (spec §5): reject unsigned or tampered messages
        if !signatures::verify_sign_tender(tender, &public_key) {
            error!("Discarding tender {}: invalid signature", tender_id);
            eprintln!("tender:error id={} reason=invalid_signature", tender_id);
            return;
        }

        // Determine if this node is the tender owner (published the tender)
        let is_owner = source_peer.to_string() == self.local_node_id;
        let manifest_json = if is_owner {
            get_local_manifest(&tender_id)
        } else {
            None
        };

        // Owner path: start bid collection window, don't bid on own tender
        if is_owner {
            if manifest_json.is_none() {
                error!(
                    "Local node issued tender {} without cached manifest; refusing to proceed",
                    tender_id
                );
                eprintln!("tender:error id={} reason=missing_manifest", tender_id);
                return;
            }

            let manifest_key =
                take_tender_manifest_id(&tender_id).unwrap_or_else(|| tender_id.clone());

            // Initialize bid collection context and spawn winner selection task
            self.initialize_owned_tender(
                &tender_id,
                &manifest_key,
                &tender.manifest_digest,
                manifest_json.clone(),
            );
            info!(
                "Local node issued tender {}; skipping bid as owner",
                tender_id
            );
            println!("tender:owned id={} action=skip_bid", tender_id);
            return;
        }

        // Bidder path: evaluate eligibility and submit bid
        //
        // TODO(spec §4.1): Implement real resource evaluation against tender requirements.
        // Current implementation uses placeholder scores for testing.

        // Step 1: Evaluate resource fit (placeholder - assumes perfect fit)
        let capacity_score = 1.0;

        // Step 2: Calculate composite bid score
        // Spec §4.1 recommends: Resource fit (50%), Network locality (30%),
        // Historical reliability (10%), Price/QoS (10%)
        // Current: simplified formula for initial implementation
        let my_score = capacity_score * 0.8 + 0.2;

        // Step 3: Construct and sign the bid
        let mut bid = Bid {
            tender_id: tender_id.clone(),
            node_id: self.local_node_id.clone(),
            score: my_score,
            resource_fit_score: capacity_score,
            network_locality_score: 0.5,
            timestamp: now_ms(),
            nonce: rand::thread_rng().next_u64(),
            signature: Vec::new(),
        };

        let bid_bytes = match signatures::sign_bid(&mut bid, &self.keypair) {
            Ok(_) => machine::encode_scheduler_message(SchedulerMessage::Bid(bid.clone())),
            Err(e) => {
                error!("Failed to sign bid for tender {}: {}", tender_id, e);
                eprintln!(
                    "tender:error id={} reason=sign_bid_failure err={}",
                    tender_id, e
                );
                return;
            }
        };

        // Step 4: Queue bid for publication on beemesh-fabric
        if let Err(e) = self.outbound_tx.send(SchedulerCommand::Publish {
            topic: BEEMESH_FABRIC.to_string(),
            payload: bid_bytes,
        }) {
            error!("Failed to queue bid for tender {}: {}", tender_id, e);
        } else {
            info!(
                "Queued Bid for tender {} with score {:.2}",
                tender_id, my_score
            );
        }
    }

    // ========================================================================
    // Owned Tender Management (Bid Collection & Winner Selection)
    // ========================================================================

    /// Initializes bid collection for a tender owned by this node.
    ///
    /// This is called when we receive our own tender back from gossipsub (after
    /// publishing it). It sets up the bid collection context and spawns an async
    /// task that will:
    ///
    /// 1. Wait for the selection window (`DEFAULT_SELECTION_WINDOW_MS`)
    /// 2. Collect all bids received during that window
    /// 3. Deterministically select winners using `select_winners()`
    /// 4. Publish the Award message
    /// 5. Send manifest to each winner via secure stream
    /// 6. If local node won, trigger deployment
    ///
    /// # Spec Reference
    ///
    /// - §4.1: "Tender owner MUST collect bids for at least the bid window"
    /// - §4.1: "Winner selection MUST be reproducible by any observer"
    fn initialize_owned_tender(
        &self,
        tender_id: &str,
        manifest_id: &str,
        manifest_digest: &str,
        manifest_json: Option<String>,
    ) {
        // Register the tender in our owned_tenders map (thread-safe)
        {
            let mut owned = self.owned_tenders.lock().unwrap();
            if owned.contains_key(tender_id) {
                return; // Already initialized (duplicate tender receipt)
            }

            owned.insert(
                tender_id.to_string(),
                OwnedTenderContext {
                    bid_context: BidContext {
                        manifest_id: manifest_id.to_string(),
                        manifest_digest: manifest_digest.to_string(),
                        bids: Vec::new(),
                    },
                    manifest_json,
                },
            );
        }

        // Clone references for the async task
        let owned_tenders = self.owned_tenders.clone();
        let tender_id_clone = tender_id.to_string();
        let local_id = self.local_node_id.clone();
        let keypair = self.keypair.clone();
        let outbound_tx = self.outbound_tx.clone();

        // Spawn async task to handle the selection window and winner announcement
        tokio::spawn(async move {
            // Wait for the selection window to allow bids to arrive
            // TODO(spec §4.1): Add jitter (selection_jitter_ms) to this delay
            sleep(Duration::from_millis(DEFAULT_SELECTION_WINDOW_MS)).await;

            // Close the bidding window and extract collected bids
            let (winners, manifest_id_opt, manifest_json_opt, manifest_digest_opt) = {
                let mut tenders = owned_tenders.lock().unwrap();
                if let Some(ctx) = tenders.remove(&tender_id_clone) {
                    (
                        select_winners(&ctx.bid_context),
                        Some(ctx.bid_context.manifest_id.clone()),
                        ctx.manifest_json.clone(),
                        Some(ctx.bid_context.manifest_digest.clone()),
                    )
                } else {
                    (Vec::new(), None, None, None)
                }
            };

            // Proceed only if we have valid manifest information
            if let (Some(manifest_id), Some(manifest_digest)) =
                (manifest_id_opt, manifest_digest_opt)
            {
                if winners.is_empty() {
                    info!(
                        "No qualifying bids for tender {} (manifest {})",
                        tender_id_clone, manifest_id
                    );
                    return;
                }

                // Log winner summary for debugging
                let summary = winners
                    .iter()
                    .map(|w| format!("{} ({:.2})", w.bidder_id, w.score))
                    .collect::<Vec<_>>()
                    .join(", ");

                info!(
                    "Selected winners for tender {} (manifest {}): {}",
                    tender_id_clone, manifest_id, summary
                );

                // Construct and sign the Award message
                let winners_list: Vec<String> =
                    winners.iter().map(|w| w.bidder_id.clone()).collect();
                let mut award = Award {
                    tender_id: tender_id_clone.clone(),
                    winners: winners_list.clone(),
                    manifest_digest: manifest_digest.clone(),
                    timestamp: now_ms(),
                    nonce: rand::thread_rng().next_u64(),
                    signature: Vec::new(),
                };

                // Publish award to fabric (spec §4.1: "Award MUST be visible on fabric topic")
                if let Err(e) = signatures::sign_award(&mut award, &keypair) {
                    error!("Failed to sign award for tender {}: {}", tender_id_clone, e);
                } else if let Err(e) = outbound_tx.send(SchedulerCommand::Publish {
                    topic: BEEMESH_FABRIC.to_string(),
                    payload: machine::encode_scheduler_message(SchedulerMessage::Award(
                        award.clone(),
                    )),
                }) {
                    error!(
                        "Failed to publish award for tender {}: {}",
                        tender_id_clone, e
                    );
                } else {
                    info!(
                        "Published award for tender {} to {:?}",
                        tender_id_clone, winners_list
                    );
                }

                if let Some(manifest_json) = manifest_json_opt
                    .clone()
                    .or_else(|| get_local_manifest(&manifest_id))
                {
                    let owner_pubkey = keypair.to_protobuf_encoding().unwrap_or_default();

                    // Build manifest transfer payload for remote winners
                    let transfer = ManifestTransfer {
                        tender_id: tender_id_clone.clone(),
                        manifest_id: manifest_id.clone(),
                        manifest_digest: manifest_digest.clone(),
                        manifest_json: manifest_json.clone(),
                        owner_peer_id: local_id.clone(),
                        owner_pubkey,
                        replicas: 1,
                    };

                    let transfer_bytes = machine::build_manifest_transfer(&transfer);

                    // Send manifest to remote winners via request-response protocol
                    // (spec §4.1: "manifest MUST be sent only to awarded nodes")
                    for winner in winners.iter().filter(|bid| bid.bidder_id != local_id) {
                        if let Err(e) = outbound_tx.send(SchedulerCommand::SendManifest {
                            peer_id: winner.bidder_id.clone(),
                            payload: transfer_bytes.clone(),
                        }) {
                            error!(
                                "Failed to queue manifest transfer for tender {} to {}: {}",
                                tender_id_clone, winner.bidder_id, e
                            );
                        } else {
                            info!(
                                "Queued manifest transfer for tender {} to winner {}",
                                tender_id_clone, winner.bidder_id
                            );
                        }
                    }
                } else {
                    error!(
                        "Unable to load manifest content for tender {}; cannot distribute to winners",
                        tender_id_clone
                    );
                }

                // If local node is among the winners, trigger deployment
                if winners.iter().any(|bid| bid.bidder_id == local_id) {
                    info!(
                        "Local node won tender {}; proceeding to deployment",
                        tender_id_clone
                    );

                    if let Some(manifest_json) =
                        manifest_json_opt.or_else(|| get_local_manifest(&manifest_id))
                    {
                        if let Err(e) = outbound_tx.send(SchedulerCommand::DeployWorkload {
                            tender_id: tender_id_clone.clone(),
                            manifest_id,
                            manifest_json,
                            replicas: 1,
                        }) {
                            error!(
                                "Failed to trigger deployment for tender {}: {}",
                                tender_id_clone, e
                            );
                        } else {
                            info!("Triggered deployment for tender {}", tender_id_clone);
                        }
                    } else {
                        error!(
                            "Local node lacks manifest content for tender {}; cannot deploy",
                            tender_id_clone
                        );
                    }
                }
            }
        });
    }

    // ========================================================================
    // Replay Protection Helpers
    // ========================================================================

    /// Records a tender as seen and returns true if this is the first time.
    ///
    /// Uses the (tender_id, nonce) tuple as the dedup key.
    fn mark_tender_seen(&self, tender_id: &str, nonce: u64) -> bool {
        let mut seen = self.seen_tenders.lock().unwrap();
        record_replay(&mut seen, (tender_id.to_string(), nonce))
    }

    /// Records a bid as seen and returns true if this is the first time.
    ///
    /// Uses the (tender_id, node_id, nonce) tuple as the dedup key.
    fn mark_bid_seen(&self, bid: &Bid) -> bool {
        let mut seen = self.seen_bids.lock().unwrap();
        record_replay(
            &mut seen,
            (bid.tender_id.clone(), bid.node_id.clone(), bid.nonce),
        )
    }

    /// Records an award as seen and returns true if this is the first time.
    fn mark_award_seen(&self, tender_id: &str, nonce: u64) -> bool {
        let mut seen = self.seen_awards.lock().unwrap();
        record_replay(&mut seen, (tender_id.to_string(), nonce))
    }

    /// Records an event as seen and returns true if this is the first time.
    fn mark_event_seen(&self, event: &SchedulerEvent) -> bool {
        let mut seen = self.seen_events.lock().unwrap();
        record_replay(
            &mut seen,
            (event.tender_id.clone(), event.node_id.clone(), event.nonce),
        )
    }

    // ========================================================================
    // Bid Handling
    // ========================================================================

    /// Processes an incoming bid message.
    ///
    /// Only the tender owner collects bids. Other nodes ignore bids for tenders
    /// they don't own (they will learn the outcome via the Award message).
    ///
    /// # Validation
    ///
    /// 1. Timestamp freshness check
    /// 2. Signature verification
    /// 3. Replay protection
    /// 4. Ownership check (only collect if we own this tender)
    async fn handle_bid(&self, bid: &Bid, message: &gossipsub::Message) {
        if !is_timestamp_fresh(bid.timestamp) {
            error!(
                "Discarding bid for tender {} from {}: timestamp outside skew",
                bid.tender_id, bid.node_id
            );
            eprintln!(
                "bid:error tender_id={} node_id={} reason=stale_timestamp",
                bid.tender_id, bid.node_id
            );
            return;
        }

        let public_key = match message
            .source
            .as_ref()
            .and_then(|peer_id| peer_id_to_public_key(peer_id))
        {
            Some(key) => key,
            None => {
                error!(
                    "Discarding bid for tender {}: unable to derive public key from source",
                    bid.tender_id
                );
                eprintln!(
                    "bid:error tender_id={} node_id={} reason=no_public_key",
                    bid.tender_id, bid.node_id
                );
                return;
            }
        };

        if !signatures::verify_sign_bid(bid, &public_key) {
            error!(
                "Discarding bid for tender {} from {}: invalid signature",
                bid.tender_id, bid.node_id
            );
            eprintln!(
                "bid:error tender_id={} node_id={} reason=invalid_signature",
                bid.tender_id, bid.node_id
            );
            return;
        }

        // Replay protection
        if !self.mark_bid_seen(bid) {
            info!(
                "Ignoring replayed bid {} from {}",
                bid.tender_id, bid.node_id
            );
            eprintln!(
                "bid:replay tender_id={} node_id={}",
                bid.tender_id, bid.node_id
            );
            return;
        }

        let tender_id = bid.tender_id.clone();
        let bidder_id = bid.node_id.clone();
        let score = bid.score;

        // Ignore our own bids (we already know our score)
        if bidder_id == self.local_node_id {
            println!(
                "bid:local tender_id={} node_id={} action=ignore",
                tender_id, bidder_id
            );
            return;
        }

        // Only collect bids for tenders we own
        let mut owned = self.owned_tenders.lock().unwrap();
        if let Some(ctx) = owned.get_mut(&tender_id) {
            info!(
                "Recorded bid for tender {}: {:.2} from {}",
                tender_id, score, bidder_id
            );
            println!(
                "bid:recorded tender_id={} node_id={} score={:.2}",
                tender_id, bidder_id, score
            );
            ctx.bid_context.bids.push(BidEntry {
                bidder_id: bidder_id.to_string(),
                score,
            });
        } else {
            // Not the tender owner - ignore (we'll learn outcome from Award)
            info!(
                "Ignoring bid for tender {} because this node is not the owner",
                tender_id
            );
            eprintln!(
                "bid:ignored tender_id={} node_id={} reason=not_owner",
                tender_id, bidder_id
            );
        }
    }

    // ========================================================================
    // Event Handling
    // ========================================================================

    /// Processes scheduler events (Deployed, Failed, Preempted, Cancelled).
    ///
    /// Events are informational: they notify the fabric about deployment outcomes
    /// and allow nodes to update their internal state. For example, observing a
    /// `Failed` event might allow a non-winning node to re-evaluate the tender.
    ///
    /// # Spec Reference
    ///
    /// - §9.1: Events MUST be emitted for Deployed, Failed, Preempted, Cancelled
    /// - §6: Failed events may trigger backoff and retry behavior
    async fn handle_event(&self, event: &SchedulerEvent, message: &gossipsub::Message) {
        let tender_id = event.tender_id.clone();
        info!(
            "Received Scheduler Event for tender {}: {:?}",
            tender_id, event.event_type
        );

        // Timestamp freshness check
        if !is_timestamp_fresh(event.timestamp) {
            error!(
                "Discarding scheduler event for tender {}: timestamp outside skew",
                tender_id
            );
            eprintln!("event:error tender_id={} reason=stale_timestamp", tender_id);
            return;
        }

        // Signature verification
        let public_key = match message
            .source
            .as_ref()
            .and_then(|peer_id| peer_id_to_public_key(peer_id))
        {
            Some(key) => key,
            None => {
                error!(
                    "Discarding scheduler event for tender {}: unable to derive public key",
                    tender_id
                );
                eprintln!("event:error tender_id={} reason=no_public_key", tender_id);
                return;
            }
        };

        if !signatures::verify_sign_scheduler_event(event, &public_key) {
            error!(
                "Discarding scheduler event for tender {}: invalid signature",
                tender_id
            );
            eprintln!(
                "event:error tender_id={} reason=invalid_signature",
                tender_id
            );
            return;
        }

        // Replay protection
        if !self.mark_event_seen(event) {
            info!(
                "Ignoring replayed scheduler event for tender {} from {}",
                tender_id, event.node_id
            );
            eprintln!(
                "event:replay tender_id={} node_id={} nonce={}",
                tender_id, event.node_id, event.nonce
            );
            return;
        }

        // Process termination events for locally deployed workloads
        // These events indicate that a previously running workload has stopped
        if event.node_id == self.local_node_id {
            if matches!(
                event.event_type,
                EventType::Cancelled | EventType::Preempted | EventType::Failed
            ) {
                info!(
                    "Received termination event for locally deployed tender {}",
                    tender_id
                );
            }
        }
    }

    // ========================================================================
    // Award Handling
    // ========================================================================

    /// Processes Award messages announcing tender winners.
    ///
    /// Awards are published by the tender owner after the bid collection period
    /// expires. Winners are selected by highest score (§5.2). If this node is
    /// among the winners, it awaits manifest transfer via RequestResponse.
    ///
    /// # Spec Reference
    ///
    /// - §5.2: Awards MUST contain winner list ordered by score
    /// - §5.3: Manifest transfer follows award via RequestResponse
    async fn handle_award(&self, award: &Award, message: &gossipsub::Message) {
        // Timestamp freshness check
        if !is_timestamp_fresh(award.timestamp) {
            error!(
                "Discarding award for tender {}: timestamp outside skew",
                award.tender_id
            );
            eprintln!(
                "award:error tender_id={} reason=stale_timestamp",
                award.tender_id
            );
            return;
        }

        // Signature verification
        let public_key = match message
            .source
            .as_ref()
            .and_then(|peer_id| peer_id_to_public_key(peer_id))
        {
            Some(key) => key,
            None => {
                error!(
                    "Discarding award for tender {}: unable to derive public key",
                    award.tender_id
                );
                eprintln!(
                    "award:error tender_id={} reason=no_public_key",
                    award.tender_id
                );
                return;
            }
        };

        if !signatures::verify_sign_award(award, &public_key) {
            error!(
                "Discarding award for tender {}: invalid signature",
                award.tender_id
            );
            eprintln!(
                "award:error tender_id={} reason=invalid_signature",
                award.tender_id
            );
            return;
        }

        // Replay protection
        if !self.mark_award_seen(&award.tender_id, award.nonce) {
            info!(
                "Ignoring replayed award for tender {} from {:?}",
                award.tender_id, message.source
            );
            eprintln!(
                "award:replay tender_id={} nonce={}",
                award.tender_id, award.nonce
            );
            return;
        }

        // Check if this node is a winner
        if award.winners.contains(&self.local_node_id) {
            // Winner: expect manifest transfer via RequestResponse protocol
            info!(
                "Local node awarded tender {}; awaiting manifest transfer",
                award.tender_id
            );
            println!(
                "award:received tender_id={} outcome=winner awaiting_manifest=true",
                award.tender_id
            );
        } else {
            // Observer: not selected for this tender
            println!(
                "award:received tender_id={} outcome=observer winners={:?}",
                award.tender_id, award.winners
            );
        }
    }

    // ========================================================================
    // Event Publishing
    // ========================================================================

    /// Publishes a scheduler event to the fabric.
    ///
    /// Events are informational messages about deployment lifecycle changes.
    /// This method handles signing and publishing the event via gossipsub.
    ///
    /// # Arguments
    ///
    /// * `tender_id` - The tender this event relates to
    /// * `event_type` - The type of event (Deployed, Failed, etc.)
    /// * `reason` - Human-readable reason for the event
    pub fn publish_event(&self, tender_id: &str, event_type: EventType, reason: &str) {
        // Create unsigned event with fresh timestamp and random nonce
        let mut event = SchedulerEvent {
            tender_id: tender_id.to_string(),
            node_id: self.local_node_id.clone(),
            event_type,
            reason: reason.to_string(),
            timestamp: now_ms(),
            nonce: rand::thread_rng().next_u64(),
            signature: Vec::new(),
        };

        // Sign and encode the event
        let payload = match signatures::sign_scheduler_event(&mut event, &self.keypair) {
            Ok(_) => machine::encode_scheduler_message(SchedulerMessage::Event(event)),
            Err(e) => {
                error!(
                    "Failed to sign scheduler event for tender {}: {}",
                    tender_id, e
                );
                return;
            }
        };

        // Queue for publication via gossipsub
        if let Err(e) = self.outbound_tx.send(SchedulerCommand::Publish {
            topic: BEEMESH_FABRIC.to_string(),
            payload,
        }) {
            error!(
                "Failed to queue scheduler event publication for tender {}: {}",
                tender_id, e
            );
        }
    }
}

// ============================================================================
// Winner Selection
// ============================================================================

/// Intermediate structure for winner selection.
///
/// Used during `select_winners()` to track the best score per bidding node.
#[derive(Clone)]
struct BidOutcome {
    /// The node ID of the bidder
    bidder_id: String,
    /// The highest score submitted by this bidder
    score: f64,
}

/// Selects winners from collected bids based on highest score.
///
/// Implements §5.2 of the spec: winners are ordered by score descending,
/// limited to `replicas` count. If multiple bids exist from the same node,
/// only the highest-scoring bid is considered.
///
/// # Arguments
///
/// * `context` - The bid collection context containing all received bids
///
/// # Returns
///
/// A vector of `BidOutcome` structs representing the winners, ordered by score.
fn select_winners(context: &BidContext) -> Vec<BidOutcome> {
    // De-duplicate bids by node, keeping highest score per node
    let mut best_by_node: HashMap<String, BidEntry> = HashMap::new();

    for bid in &context.bids {
        let entry = best_by_node
            .entry(bid.bidder_id.clone())
            .or_insert_with(|| bid.clone());

        if bid.score > entry.score {
            *entry = bid.clone();
        }
    }

    let mut bids: Vec<BidEntry> = best_by_node.into_values().collect();
    bids.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.bidder_id.cmp(&b.bidder_id))
    });

    bids.into_iter()
        .map(|bid| BidOutcome {
            bidder_id: bid.bidder_id,
            score: bid.score,
        })
        .collect()
}

// ============================================================================
// Public Manifest Cache API
// ============================================================================

/// Registers a manifest in the local cache for later retrieval.
///
/// Called when creating a tender to store the manifest JSON so it can be
/// transferred to winners via the RequestResponse protocol.
pub fn register_local_manifest(tender_id: &str, manifest_json: &str) {
    let mut cache = LOCAL_MANIFEST_CACHE.lock().unwrap();
    cache.insert(tender_id.to_string(), manifest_json.to_string());
}

/// Retrieves a manifest from the local cache.
///
/// Returns `None` if the tender_id is not found in the cache.
fn get_local_manifest(tender_id: &str) -> Option<String> {
    LOCAL_MANIFEST_CACHE.lock().unwrap().get(tender_id).cloned()
}

/// Records the user-facing manifest identifier associated with a tender.
///
/// This mapping allows the system to track which user-provided manifest ID
/// corresponds to each internal tender ID.
pub fn record_tender_manifest_id(tender_id: &str, manifest_id: &str) {
    let mut map = TENDER_MANIFEST_IDS.lock().unwrap();
    map.insert(tender_id.to_string(), manifest_id.to_string());
}

/// Retrieves the manifest identifier for a tender without removing it.
pub fn get_tender_manifest_id(tender_id: &str) -> Option<String> {
    TENDER_MANIFEST_IDS.lock().unwrap().get(tender_id).cloned()
}

/// Removes and returns the manifest identifier for a tender.
///
/// Use this when the tender is complete and the mapping is no longer needed.
pub fn take_tender_manifest_id(tender_id: &str) -> Option<String> {
    TENDER_MANIFEST_IDS.lock().unwrap().remove(tender_id)
}

// ============================================================================
// Time and Replay Utilities
// ============================================================================

/// Returns the current time in milliseconds since UNIX epoch.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// Records a key in a replay-protection map with automatic expiry.
///
/// This function maintains a sliding window of seen keys. Keys older than
/// `REPLAY_WINDOW_MS` are automatically pruned on each call.
///
/// # Returns
///
/// - `true` if the key was newly recorded (not a replay)
/// - `false` if the key was already seen within the replay window
fn record_replay<K: Eq + Hash + Clone>(map: &mut HashMap<K, u64>, key: K) -> bool {
    let now = now_ms();

    // Prune expired entries
    map.retain(|_, ts| now.saturating_sub(*ts) <= REPLAY_WINDOW_MS);

    // Check if already seen
    if let Some(ts) = map.get(&key) {
        if now.saturating_sub(*ts) <= REPLAY_WINDOW_MS {
            return false;
        }
    }

    map.insert(key, now);
    true
}

/// Checks if a timestamp is within the acceptable clock skew window.
///
/// Messages with timestamps outside the `MAX_CLOCK_SKEW_MS` window are
/// rejected to prevent replay attacks and stale message processing.
fn is_timestamp_fresh(timestamp_ms: u64) -> bool {
    let now = now_ms();
    let (min, max) = if now > timestamp_ms {
        (timestamp_ms, now)
    } else {
        (now, timestamp_ms)
    };

    max - min <= MAX_CLOCK_SKEW_MS
}

// ============================================================================
// Runtime Integration Module
// ============================================================================

pub use runtime_integration::*;

/// Runtime integration and apply handling (merged from run.rs).
///
/// This module provides integration between the existing libp2p apply message
/// handling and the workload manager system. It updates the apply message
/// handler to use the runtime engines and provider announcement system.
mod runtime_integration {
    use crate::messages::machine;
    use crate::messages::types::ApplyRequest;
    use crate::network::behaviour::MyBehaviour;
    use crate::runtimes::{DeploymentConfig, RuntimeRegistry, create_default_registry};
    use base64::Engine;
    use libp2p::Swarm;
    use libp2p::kad;
    use log::{debug, error, info, warn};
    use std::collections::HashMap;
    use std::sync::{Arc, LazyLock};
    use tokio::sync::RwLock;

    /// Global runtime registry for all available container engines.
    ///
    /// Lazily initialized on first access. Contains Podman and potentially
    /// other container runtime adapters.
    static RUNTIME_REGISTRY: LazyLock<Arc<RwLock<Option<RuntimeRegistry>>>> =
        LazyLock::new(|| Arc::new(RwLock::new(None)));

    /// Node-local cache mapping manifest IDs to owner public keys.
    ///
    /// Used to verify delete requests come from the original manifest owner.
    static MANIFEST_OWNER_MAP: LazyLock<RwLock<HashMap<String, Vec<u8>>>> =
        LazyLock::new(|| RwLock::new(HashMap::new()));

    /// Initializes the runtime registry and probes available engines.
    ///
    /// This should be called during node startup to prepare the container
    /// runtime system. Currently defaults to Podman.
    pub async fn initialize_podman_manager() -> Result<(), Box<dyn std::error::Error>> {
        info!("Initializing runtime registry for manifest deployment");

        let registry = create_default_registry().await;
        let available_engines = registry.check_available_engines().await;

        info!("Available runtime engines: {:?}", available_engines);

        // Store the registry globally
        {
            let mut global_registry = RUNTIME_REGISTRY.write().await;
            *global_registry = Some(registry);
        }

        info!("Runtime registry initialized successfully");
        Ok(())
    }

    /// Records the owner public key for a manifest on this node.
    ///
    /// Used for authorization when processing delete requests.
    pub async fn record_manifest_owner(manifest_id: &str, owner_pubkey: &[u8]) {
        let mut map = MANIFEST_OWNER_MAP.write().await;
        map.insert(manifest_id.to_string(), owner_pubkey.to_vec());
        info!(
            "record_manifest_owner: stored owner_pubkey len={} for manifest_id={}",
            owner_pubkey.len(),
            manifest_id
        );
    }

    /// Retrieves the owner public key for a manifest if known.
    pub async fn get_manifest_owner(manifest_id: &str) -> Option<Vec<u8>> {
        let map = MANIFEST_OWNER_MAP.read().await;
        map.get(manifest_id).cloned()
    }

    /// Removes the owner mapping for a manifest.
    pub async fn remove_manifest_owner(manifest_id: &str) -> Option<Vec<u8>> {
        let mut map = MANIFEST_OWNER_MAP.write().await;
        map.remove(manifest_id)
    }

    /// Provides access to the global runtime registry (for testing/debug).
    pub async fn get_global_runtime_registry()
    -> Option<tokio::sync::RwLockReadGuard<'static, Option<RuntimeRegistry>>> {
        let registry_guard = RUNTIME_REGISTRY.read().await;
        if registry_guard.is_some() {
            Some(registry_guard)
        } else {
            None
        }
    }

    /// Processes manifest deployment using the workload manager.
    ///
    /// This is the main entry point for deploying workloads received via
    /// the tender/bid/award protocol. It:
    ///
    /// 1. Decodes the manifest content (handles base64 if needed)
    /// 2. Modifies replicas to 1 for single-node deployment
    /// 3. Selects appropriate runtime engine
    /// 4. Deploys via the runtime adapter
    /// 5. Publishes workload info to DHT
    pub async fn process_manifest_deployment(
        swarm: &mut Swarm<MyBehaviour>,
        apply_req: &ApplyRequest,
        manifest_json: &str,
        owner_pubkey: &[u8],
    ) -> Result<String, Box<dyn std::error::Error>> {
        info!("Processing manifest deployment");

        // Parse the manifest content; decode from base64 if needed
        let manifest_content = decode_manifest_content(manifest_json);

        // Use the manifest_id from the apply request for placement announcements
        // This ensures consistency between apply and delete operations
        let manifest_id = if apply_req.manifest_id.is_empty() {
            "unknown".to_string()
        } else {
            apply_req.manifest_id.clone()
        };

        info!(
            "Processing manifest deployment for manifest_id: {}",
            manifest_id
        );

        // Store owner pubkey for later delete authorization
        if owner_pubkey.is_empty() {
            debug!(
                "process_manifest_deployment: missing owner pubkey for manifest_id={}",
                manifest_id
            );
        } else {
            record_manifest_owner(&manifest_id, owner_pubkey).await;
        }

        // Modify manifest to set replicas=1 for this node deployment
        // The original manifest is stored in DHT, but each node deploys with replicas=1
        let modified_manifest_content = modify_manifest_replicas(&manifest_content)?;

        // Create deployment configuration
        let deployment_config = create_deployment_config(apply_req);

        // Select appropriate runtime engine based on manifest type
        let engine_name = select_runtime_engine(&modified_manifest_content).await?;
        info!(
            "Selected runtime engine '{}' for manifest_id: {}",
            engine_name, manifest_id
        );

        // Get runtime registry
        let registry_guard = RUNTIME_REGISTRY.read().await;
        let registry = registry_guard
            .as_ref()
            .ok_or("Runtime registry not initialized")?;

        // Get the selected engine
        let engine = registry
            .get_engine(&engine_name)
            .ok_or(format!("Runtime engine '{}' not available", engine_name))?;

        let local_peer_id = swarm.local_peer_id().clone();

        // Deploy the workload with modified manifest (replicas=1) and track the
        // local peer ID so debug endpoints can attribute workloads correctly.
        let workload_info = engine
            .deploy_workload_with_peer(
                &manifest_id,
                &modified_manifest_content,
                &deployment_config,
                local_peer_id,
            )
            .await?;

        // Record the workload-peer mapping for multi-node test scenarios
        super::record_workload_peer(&workload_info.id, &local_peer_id.to_string()).await;

        info!(
            "Workload deployed successfully: {} using engine '{}', status: {:?}",
            workload_info.id, engine_name, workload_info.status
        );

        publish_workload_to_dht(swarm, &workload_info.id);

        Ok(workload_info.id)
    }

    /// Modify the manifest to set replicas=1 for single-node deployment
    /// Each node in the fabric will deploy one replica of the workload
    fn modify_manifest_replicas(
        manifest_content: &[u8],
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let manifest_str = String::from_utf8_lossy(manifest_content);

        // Try to parse as YAML/JSON and modify replicas field
        if let Ok(mut doc) = serde_yaml::from_str::<serde_yaml::Value>(&manifest_str) {
            // Check for spec.replicas field (Kubernetes-style)
            if let Some(spec) = doc.get_mut("spec") {
                if let Some(spec_map) = spec.as_mapping_mut() {
                    spec_map.insert(
                        serde_yaml::Value::String("replicas".to_string()),
                        serde_yaml::Value::Number(serde_yaml::Number::from(1)),
                    );
                }
            }
            // Check for top-level replicas field
            else if doc.get("replicas").is_some() {
                if let Some(doc_map) = doc.as_mapping_mut() {
                    doc_map.insert(
                        serde_yaml::Value::String("replicas".to_string()),
                        serde_yaml::Value::Number(serde_yaml::Number::from(1)),
                    );
                }
            }

            // Convert back to YAML bytes
            let modified_yaml = serde_yaml::to_string(&doc)?;
            info!("Modified manifest to set replicas=1 for single-node deployment");
            Ok(modified_yaml.into_bytes())
        } else {
            // If parsing fails, return original content unchanged
            warn!("Failed to parse manifest for replica modification, using original");
            Ok(manifest_content.to_vec())
        }
    }

    /// Decode manifest content that may be base64-encoded.
    /// Falls back to treating the string as plain text if decoding fails or produces non-UTF8.
    pub(super) fn decode_manifest_content(manifest_json: &str) -> Vec<u8> {
        match base64::engine::general_purpose::STANDARD.decode(manifest_json) {
            Ok(decoded) => {
                if String::from_utf8(decoded.clone()).is_ok() {
                    debug!("Decoded base64 manifest content ({} bytes)", decoded.len());
                    decoded
                } else {
                    debug!(
                        "Base64 manifest content was not valid UTF-8; using raw manifest string"
                    );
                    manifest_json.as_bytes().to_vec()
                }
            }
            Err(_) => manifest_json.as_bytes().to_vec(),
        }
    }

    /// Select the appropriate runtime engine based on manifest content and annotations
    async fn select_runtime_engine(
        manifest_content: &[u8],
    ) -> Result<String, Box<dyn std::error::Error>> {
        let manifest_str = String::from_utf8_lossy(manifest_content);

        // Try to parse as YAML and look for annotations
        if let Ok(doc) = serde_yaml::from_str::<serde_yaml::Value>(&manifest_str) {
            // Check for runtime engine annotation
            if let Some(metadata) = doc.get("metadata") {
                if let Some(annotations) = metadata.get("annotations") {
                    if let Some(engine) = annotations
                        .get("beemesh.io/runtime-engine")
                        .and_then(|v| v.as_str())
                    {
                        info!("Found runtime engine annotation: {}", engine);
                        return Ok(engine.to_string());
                    }
                }
            }

            // Check manifest type - note preferences, but don't hardcode
            if let Some(kind) = doc.get("kind").and_then(|k| k.as_str()) {
                match kind {
                    "Pod" | "Deployment" | "Service" | "ConfigMap" | "Secret" => {
                        // Kubernetes resources - will prefer Podman below if available
                        debug!("Detected Kubernetes manifest kind: {}", kind);
                    }
                    _ => {}
                }
            }

            // Check for Docker Compose format - note preference, but don't hardcode
            if doc.get("services").is_some() && doc.get("version").is_some() {
                debug!("Detected Docker Compose manifest");
                // Will prefer Docker below if available
            }
        }

        // Get available engines and select the best one
        if let Some(registry) = RUNTIME_REGISTRY.read().await.as_ref() {
            let available = registry.check_available_engines().await;

            if *available.get("podman").unwrap_or(&false) {
                return Ok("podman".to_string());
            }

            if let Some(default_engine) = registry.get_default_engine() {
                warn!(
                    "Preferred Podman runtime unavailable; falling back to default engine '{}'",
                    default_engine.name()
                );
                return Ok(default_engine.name().to_string());
            }
        }

        Err("No suitable runtime engine available".into())
    }

    /// Create deployment configuration from apply request
    fn create_deployment_config(apply_req: &ApplyRequest) -> DeploymentConfig {
        let mut config = DeploymentConfig::default();

        // Set replicas
        config.replicas = apply_req.replicas;

        // Metadata from apply request can be added here if needed
        if !apply_req.operation_id.is_empty() {
            config.env.insert(
                "BEEMESH_OPERATION_ID".to_string(),
                apply_req.operation_id.clone(),
            );
        }

        config
    }

    /// Store the workload id in the machine DHT using a simple key/value record
    fn publish_workload_to_dht(swarm: &mut Swarm<MyBehaviour>, workload_id: &str) {
        let record_key = kad::RecordKey::new(&format!("workload:{}", workload_id));
        let record = kad::Record {
            key: record_key,
            value: workload_id.as_bytes().to_vec(),
            publisher: None,
            expires: None,
        };

        match swarm
            .behaviour_mut()
            .kademlia
            .put_record(record, kad::Quorum::One)
        {
            Ok(query_id) => {
                info!(
                    "DHT: stored workload {} with query_id {:?} as single source of truth",
                    workload_id, query_id
                );
            }
            Err(e) => {
                warn!(
                    "Failed to store workload {} in machine DHT: {:?}",
                    workload_id, e
                );
            }
        }
    }

    /// Enhanced self-apply processing with workload manager
    pub async fn process_enhanced_self_apply_request(
        manifest: &[u8],
        swarm: &mut Swarm<MyBehaviour>,
    ) {
        debug!(
            "Processing enhanced self-apply request (manifest len={})",
            manifest.len()
        );

        match machine::decode_apply_request(manifest) {
            Ok(apply_req) => {
                debug!(
                    "Enhanced self-apply request - operation_id={:?} replicas={}",
                    apply_req.operation_id, apply_req.replicas
                );

                if !apply_req.manifest_json.is_empty() {
                    let manifest_json = apply_req.manifest_json.clone();
                    let owner_pubkey = Vec::new();

                    match process_manifest_deployment(
                        swarm,
                        &apply_req,
                        &manifest_json,
                        &owner_pubkey,
                    )
                    .await
                    {
                        Ok(workload_id) => {
                            info!(
                                "Successfully deployed self-applied workload: {}",
                                workload_id
                            );
                        }
                        Err(e) => {
                            error!("Failed to deploy self-applied workload: {}", e);
                        }
                    }
                } else {
                    warn!("Self-apply request missing manifest JSON");
                }
            }
            Err(e) => {
                error!("Failed to parse self-apply request: {}", e);
            }
        }
    }

    // ========================================================================
    // Runtime Registry Utilities
    // ========================================================================

    /// Returns availability statistics for all registered runtime engines.
    ///
    /// Used by debug endpoints to report which container runtimes are available.
    pub async fn get_runtime_registry_stats() -> HashMap<String, bool> {
        if let Some(registry) = RUNTIME_REGISTRY.read().await.as_ref() {
            registry.check_available_engines().await
        } else {
            HashMap::new()
        }
    }

    /// Lists all registered runtime engine names.
    pub async fn list_available_engines() -> Vec<String> {
        if let Some(registry) = RUNTIME_REGISTRY.read().await.as_ref() {
            registry
                .list_engines()
                .iter()
                .map(|s| s.to_string())
                .collect()
        } else {
            Vec::new()
        }
    }

    // ========================================================================
    // Workload Management Operations
    // ========================================================================

    /// Removes a workload by ID using a specific engine.
    ///
    /// # Arguments
    ///
    /// * `workload_id` - The container/workload ID to remove
    /// * `engine_name` - The runtime engine managing this workload
    pub async fn remove_workload_by_id(
        workload_id: &str,
        engine_name: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let registry_guard = RUNTIME_REGISTRY.read().await;
        let registry = registry_guard
            .as_ref()
            .ok_or("Runtime registry not initialized")?;

        let engine = registry
            .get_engine(engine_name)
            .ok_or(format!("Runtime engine '{}' not available", engine_name))?;

        engine.remove_workload(workload_id).await?;
        info!(
            "Successfully removed workload: {} from engine: {}",
            workload_id, engine_name
        );
        Ok(())
    }

    /// Removes all workloads associated with a manifest ID across all engines.
    ///
    /// This is used during delete operations to clean up all containers that
    /// were deployed as part of a single manifest. Searches through all
    /// registered runtime engines.
    ///
    /// # Returns
    ///
    /// A vector of workload IDs that were successfully removed.
    pub async fn remove_workloads_by_manifest_id(
        manifest_id: &str,
    ) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        info!(
            "remove_workloads_by_manifest_id: manifest_id={}",
            manifest_id
        );

        let registry_guard = RUNTIME_REGISTRY.read().await;
        let registry = registry_guard
            .as_ref()
            .ok_or("Runtime registry not initialized")?;

        let mut removed_workloads = Vec::new();
        let mut errors = Vec::new();
        let engine_names = registry.list_engines();

        // Search all engines for matching workloads
        for engine_name in &engine_names {
            if let Some(engine) = registry.get_engine(engine_name) {
                info!(
                    "Checking engine '{}' for workloads with manifest_id '{}'",
                    engine_name, manifest_id
                );

                // List workloads from this engine
                match engine.list_workloads().await {
                    Ok(workloads) => {
                        // Find workloads that match the manifest_id
                        for workload in workloads {
                            if workload.manifest_id == manifest_id {
                                info!(
                                    "Found matching workload: {} in engine '{}'",
                                    workload.id, engine_name
                                );

                                // Remove the workload
                                match engine.remove_workload(&workload.id).await {
                                    Ok(()) => {
                                        info!(
                                            "Successfully removed workload: {} from engine '{}'",
                                            workload.id, engine_name
                                        );
                                        removed_workloads.push(workload.id);
                                    }
                                    Err(e) => {
                                        error!(
                                            "Failed to remove workload {} from engine '{}': {}",
                                            workload.id, engine_name, e
                                        );
                                        errors.push(format!("{}:{}", engine_name, workload.id));
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!(
                            "Failed to list workloads from engine '{}': {}",
                            engine_name, e
                        );
                        errors.push(format!("list:{}", engine_name));
                    }
                }
            }
        }

        // Clear owner mapping only if all removals succeeded
        if errors.is_empty() {
            if remove_manifest_owner(manifest_id).await.is_some() {
                info!(
                    "remove_workloads_by_manifest_id: cleared owner mapping for manifest_id={}",
                    manifest_id
                );
            }
        } else {
            warn!(
                "remove_workloads_by_manifest_id: retaining owner mapping for manifest_id={} due to errors {:?}",
                manifest_id, errors
            );
        }

        info!(
            "remove_workloads_by_manifest_id completed: removed {} workloads for manifest_id '{}'",
            removed_workloads.len(),
            manifest_id
        );
        Ok(removed_workloads)
    }

    /// Retrieves logs from a running workload.
    ///
    /// # Arguments
    ///
    /// * `workload_id` - The container/workload ID
    /// * `engine_name` - The runtime engine managing this workload
    /// * `tail` - Optional number of lines to retrieve from the end
    pub async fn get_workload_logs_by_id(
        workload_id: &str,
        engine_name: &str,
        tail: Option<usize>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let registry_guard = RUNTIME_REGISTRY.read().await;
        let registry = registry_guard
            .as_ref()
            .ok_or("Runtime registry not initialized")?;

        let engine = registry
            .get_engine(engine_name)
            .ok_or(format!("Runtime engine '{}' not available", engine_name))?;

        let logs = engine.get_workload_logs(workload_id, tail).await?;
        Ok(logs)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[tokio::test]
        async fn test_runtime_registry_initialization() {
            let result = initialize_podman_manager().await;
            assert!(result.is_ok());

            let stats = get_runtime_registry_stats().await;
            assert!(!stats.is_empty());

            let engines = list_available_engines().await;
            assert!(!engines.is_empty());
            assert!(engines.contains(&"podman".to_string()));
        }

        #[tokio::test]
        async fn test_runtime_engine_selection() {
            // Initialize registry for testing
            let _ = initialize_podman_manager().await;

            // Test Kubernetes manifest
            let k8s_manifest = r#"
    apiVersion: v1
    kind: Pod
    metadata:
      name: test-pod
    spec:
      containers:
      - name: nginx
        image: nginx:latest
    "#;
            let engine = select_runtime_engine(k8s_manifest.as_bytes()).await;
            assert!(engine.is_ok());
            let engine_name = engine.unwrap();
            assert_eq!(engine_name, "podman");

            // Test Docker Compose manifest
            let docker_manifest = r#"
    version: '3.8'
    services:
      web:
        image: nginx:latest
        ports:
          - "80:80"
    "#;
            let engine = select_runtime_engine(docker_manifest.as_bytes()).await;
            assert!(engine.is_ok());
            // Should prefer docker for compose files, but fall back to available engines
        }

        #[test]
        fn test_deployment_config_creation() {
            // This would require creating a mock ApplyRequest
            let config = DeploymentConfig::default();
            assert_eq!(config.replicas, 1);
            assert!(config.env.is_empty());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn decode_manifest_content_handles_base64_and_plain() {
        let manifest = "apiVersion: v1\nkind: Pod";
        let encoded = base64::engine::general_purpose::STANDARD.encode(manifest);

        let decoded = decode_manifest_content(&encoded);
        assert_eq!(decoded, manifest.as_bytes());

        let plaintext = decode_manifest_content(manifest);
        assert_eq!(plaintext, manifest.as_bytes());
    }

    #[test]
    fn selects_top_unique_winner() {
        let context = BidContext {
            manifest_id: "tender-a".to_string(),
            manifest_digest: "digest-a".to_string(),
            bids: vec![
                BidEntry {
                    bidder_id: "node1".to_string(),
                    score: 0.9,
                },
                BidEntry {
                    bidder_id: "node2".to_string(),
                    score: 0.8,
                },
                BidEntry {
                    bidder_id: "node2".to_string(),
                    score: 0.95,
                },
            ],
        };

        let winners = select_winners(&context);
        assert_eq!(winners.len(), 2);
        assert_eq!(winners[0].bidder_id, "node2");
        assert_eq!(winners[1].bidder_id, "node1");
    }

    #[test]
    fn selects_single_local_winner_when_highest() {
        let context = BidContext {
            manifest_id: "tender-b".to_string(),
            manifest_digest: "digest-b".to_string(),
            bids: vec![
                BidEntry {
                    bidder_id: "local".to_string(),
                    score: 0.99,
                },
                BidEntry {
                    bidder_id: "local".to_string(),
                    score: 0.98,
                },
                BidEntry {
                    bidder_id: "remote".to_string(),
                    score: 0.5,
                },
            ],
        };

        let winners = select_winners(&context);
        assert_eq!(winners.len(), 2);
        assert_eq!(winners[0].bidder_id, "local");
    }
}
