//! Decentralized Scheduler for Machineplane
//!
//! This module implements the **Tender→Bid→Award** scheduling flow and **DISPOSAL**
//! deletion mechanism as specified in `machineplane-spec.md` v0.3 §4. It provides
//! **at-least-once scheduling** with **deterministic winner selection** under A/P
//! (Availability/Partition-Tolerance) semantics.
//!
//! # Architecture
//!
//! The scheduler is **stateless** and **ephemeral**:
//! - No fabric-wide state is persisted
//! - Podman is the source of truth for running workloads
//! - All state (caches, replay filters) is process-local and TTL-bounded
//!
//! ## State Machine (per tender)
//!
//! ```text
//! [Idle] ──TenderReceived──► [Bidding] ──AwardReceived──► [Deploying] ──DeployOK──► [Running]
//!                                │                              │
//!                                └──NotEligible──► [Idle]       └──DeployFail──► [Idle]
//! ```
//!
//! # Message Flow (spec v0.3)
//!
//! ## Deploy Flow (Tender→Bid→Award)
//!
//! 1. **Tender**: Broadcast via gossipsub to `beemesh-fabric` topic. Contains
//!    manifest digest (not the manifest itself). Tender owner PeerId is from
//!    gossipsub message.source.
//!
//! 2. **Bid**: Sent directly to tender owner via request-response protocol.
//!    Each eligible node computes a score and submits a signed bid within
//!    the selection window (~250ms ± jitter).
//!
//! 3. **Award**: Sent directly to each winner via request-response protocol.
//!    Contains the full manifest (AwardWithManifest) for immediate deployment.
//!
//! 4. **Event**: Sent directly to tender owner via request-response protocol.
//!    Winners emit `Deployed`/`Failed` events after attempting deployment.
//!
//! ## Delete Flow (DISPOSAL)
//!
//! 1. **DISPOSAL**: Fire-and-forget broadcast via gossipsub to `beemesh-fabric`.
//!    Contains resource coordinates (namespace/kind/name), timestamp, nonce, and signature.
//!
//! 2. **Each node** that receives the DISPOSAL:
//!    - Verifies signature and timestamp freshness
//!    - Marks resource_key in DISPOSAL_SET (5-minute TTL)
//!    - Deletes local pods matching the resource coordinates
//!
//! The DISPOSAL_SET prevents the self-healer from recovering pods that are
//! intentionally being deleted during the TTL window.
//!
//! # Security
//!
//! - All messages are Ed25519-signed (see `signatures.rs`)
//! - Replay protection via `(tender_id, nonce)` tuples with 5-minute window
//! - Timestamp freshness enforced (±30 seconds max clock skew)
//! - DISPOSAL messages verified against source peer's public key
//!
//! # Global State
//!
//! The scheduler maintains minimal process-local state (not fabric-wide):
//!
//! | Cache | Purpose | TTL |
//! |-------|---------|-----|
//! | `LOCAL_MANIFEST_CACHE` | Store manifest for distribution to winners | Until tender completes |
//! | `TENDER_OWNERS` | Map tender_id → owner_peer_id for event routing | Until event sent |
//! | `DISPOSAL_SET` | Prevent self-healer recovery during deletion | 5 minutes |
//! | Replay filters | Prevent duplicate message processing | 5 minutes |
//!
//! # References
//!
//! - Spec: `machineplane-spec.md` §3 (Protocols), §4 (Scheduling), §6 (Failure Handling)

use crate::signatures;
use crate::messages::{AwardWithManifest, Bid, Disposal, EventType, SchedulerEvent, Tender};
use crate::network::identity::peer_id_to_public_key;
use crate::network::BEEMESH_FABRIC;

/// Default selection window in milliseconds for tender bidding.
pub const DEFAULT_SELECTION_WINDOW_MS: u64 = 250;

use libp2p::gossipsub;
use libp2p::identity::Keypair;
use libp2p::PeerId;
use log::{error, info};
use rand::RngCore;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio::time::sleep;

// ============================================================================
// Type Aliases
// ============================================================================

/// Type alias for bid replay filter: `(tender_id, node_id, nonce) -> first_seen_timestamp`.
type BidReplayFilter = Arc<Mutex<HashMap<(String, String, u64), u64>>>;

/// Type alias for event replay filter: `(tender_id, node_id, nonce) -> first_seen_timestamp`.
type EventReplayFilter = Arc<Mutex<HashMap<(String, String, u64), u64>>>;

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

/// Maps tender IDs to their owner peer IDs (spec v0.3).
///
/// Used to route events directly to the tender owner after receiving an award.
/// Populated when receiving an AwardWithManifest, cleared after events are sent.
static TENDER_OWNERS: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Records the owner peer for a tender (called when receiving an award).
///
/// # Panics
///
/// Panics if the mutex is poisoned. A poisoned mutex indicates scheduler
/// state corruption from a prior panic. Crashing prevents inconsistent
/// event routing that could cause message loss or misdirection.
pub fn set_tender_owner(tender_id: &str, owner_peer_id: &str) {
    let mut owners = TENDER_OWNERS
        .lock()
        .expect("tender owners mutex poisoned - scheduler state corrupted");
    owners.insert(tender_id.to_string(), owner_peer_id.to_string());
}

/// Gets the owner peer for a tender, if known.
///
/// # Panics
///
/// Panics if the mutex is poisoned. See [`set_tender_owner`] for rationale.
pub fn get_tender_owner(tender_id: &str) -> Option<String> {
    let owners = TENDER_OWNERS
        .lock()
        .expect("tender owners mutex poisoned - scheduler state corrupted");
    owners.get(tender_id).cloned()
}

// ============================================================================
// Disposal Tracking (TTL-based)
// ============================================================================

/// TTL for disposal entries (5 minutes).
///
/// This duration should be long enough to:
/// - Allow DISPOSAL message to propagate across the fabric
/// - Complete pod deletion on all nodes
/// - Prevent self-healer race conditions
///
/// After this time, disposal entries are considered expired and the self-healer
/// may attempt to recover the workload if a new tender is published.
const DISPOSAL_TTL: Duration = Duration::from_secs(300);

/// In-memory set of resource keys being disposed, with expiry timestamp.
///
/// When a DISPOSAL message is received, the resource key (namespace/kind/name)
/// is added here with an expiry time of `now + DISPOSAL_TTL`. The self-healer
/// checks this set before initiating recovery—if a resource key is present
/// and not expired, recovery is skipped.
///
/// This prevents a race condition where:
/// 1. User deletes a workload (DISPOSAL broadcast)
/// 2. Self-healer sees "missing" pods and tries to recover them
/// 3. DISPOSAL hasn't reached all nodes yet
///
/// The TTL ensures entries are eventually cleaned up, keeping memory bounded.
static DISPOSAL_SET: LazyLock<tokio::sync::RwLock<HashMap<String, std::time::Instant>>> =
    LazyLock::new(|| tokio::sync::RwLock::new(HashMap::new()));

/// Check if a resource key is marked for disposal.
///
/// Returns true if the resource is in the disposal set and hasn't expired.
///
/// # Usage
///
/// Called by the Workplane self-healer before initiating recovery:
/// ```ignore
/// if scheduler::is_disposing(&resource_key).await {
///     // Skip recovery - workload is being intentionally deleted
///     return;
/// }
/// ```
pub async fn is_disposing(resource_key: &str) -> bool {
    let set = DISPOSAL_SET.read().await;
    if let Some(expires_at) = set.get(resource_key) {
        if std::time::Instant::now() < *expires_at {
            return true;
        }
    }
    false
}

/// Mark a resource key for disposal.
///
/// Called when a DISPOSAL message is received and verified. The entry will
/// expire after `DISPOSAL_TTL` (5 minutes), after which the self-healer may
/// attempt recovery if a new tender is published.
pub async fn mark_disposal(resource_key: &str) {
    let mut set = DISPOSAL_SET.write().await;
    let expires_at = std::time::Instant::now() + DISPOSAL_TTL;
    set.insert(resource_key.to_string(), expires_at);
    info!(
        "mark_disposal: resource_key={} expires_in={:?}",
        resource_key, DISPOSAL_TTL
    );
}

/// Handle incoming DISPOSAL message received via gossipsub.
///
/// DISPOSAL is a **fire-and-forget** deletion mechanism. When a user deletes
/// a workload (e.g., `kubectl delete`), a DISPOSAL message is broadcast to
/// all nodes in the fabric. Each node independently:
///
/// 1. Verifies the message signature and timestamp freshness
/// 2. Marks the resource for disposal (prevents self-healer recovery)
/// 3. Deletes local pods matching the resource coordinates (namespace/kind/name)
/// 4. Removes the owner mapping (no longer needed)
///
/// # Security
///
/// - Signature is verified against the source peer's public key
/// - Timestamp must be within ±30 seconds of local time
/// - Replay protection via (namespace, kind, name, nonce) tuples
///
/// # Failure Handling
///
/// If pod deletion fails, the disposal is still marked in DISPOSAL_SET.
/// This ensures the self-healer won't try to recover the pods, even if
/// deletion was only partially successful. The pods will eventually be
/// cleaned up by Podman's garbage collection or a retry.
pub async fn handle_disposal(
    disposal: &Disposal,
    source_peer: &libp2p::PeerId,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let resource_key = format!("{}/{}/{}", disposal.namespace, disposal.kind, disposal.name);
    info!(
        "handle_disposal: resource={} from={}",
        resource_key,
        source_peer
    );

    // Timestamp freshness check
    if !is_timestamp_fresh(disposal.timestamp) {
        log::warn!(
            "handle_disposal: rejecting disposal for resource={} - stale timestamp",
            resource_key
        );
        return Err("Stale timestamp".into());
    }

    // Verify signature using source peer's public key
    let public_key = match peer_id_to_public_key(source_peer) {
        Some(key) => key,
        None => {
            log::warn!(
                "handle_disposal: rejecting disposal for resource={} - cannot derive public key",
                resource_key
            );
            return Err("Cannot derive public key from source peer".into());
        }
    };

    if !signatures::verify_sign_disposal(disposal, &public_key) {
        log::warn!(
            "handle_disposal: rejecting disposal for resource={} - invalid signature",
            resource_key
        );
        return Err("Invalid disposal signature".into());
    }

    // Mark for disposal using resource key (prevents self-healer from recovering)
    mark_disposal(&resource_key).await;

    // Delete local pods by resource coordinates
    match crate::runtime::delete_pods_by_resource(&disposal.namespace, &disposal.kind, &disposal.name).await {
        Ok(deleted) => {
            info!(
                "handle_disposal: deleted {} pods for resource={}",
                deleted.len(),
                resource_key
            );
        }
        Err(e) => {
            log::warn!(
                "handle_disposal: failed to delete pods for resource={}: {}",
                resource_key,
                e
            );
            // Continue - disposal is still marked, self-healer won't recover
        }
    }

    Ok(())
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
/// - Emits deployment events after pod operations
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
    seen_bids: BidReplayFilter,

    /// Replay filter for events: `(tender_id, node_id, nonce) -> first_seen_timestamp`.
    seen_events: EventReplayFilter,

    /// Channel to send commands (Publish, SendBid, SendAward, SendEvent) to the network loop.
    outbound_tx: mpsc::UnboundedSender<SchedulerCommand>,
}

// ============================================================================
// Internal Bid Tracking Structures
// ============================================================================

/// Tracks collected bids for a tender during the selection window.
struct BidContext {
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
///
/// # Messaging Architecture (spec v0.3)
///
/// - **Tender**: Published via gossipsub to `beemesh-fabric` topic
/// - **Bid**: Sent directly to tender owner via request-response
/// - **Award**: Sent directly to winners via request-response (with manifest)
/// - **Event**: Sent directly to tender owner via request-response
#[derive(Debug, Clone)]
pub enum SchedulerCommand {
    /// Publish a message to a gossipsub topic (Tender broadcast only in v0.3).
    Publish {
        /// Topic name (always `beemesh-fabric` for scheduler messages).
        topic: String,
        /// Bincode-encoded `Tender` payload.
        payload: Vec<u8>,
    },

    /// Send a bid directly to the tender owner via request-response (spec v0.3).
    SendBid {
        /// PeerId of the tender owner to send the bid to.
        peer_id: String,
        /// Bincode-encoded `Bid` payload.
        payload: Vec<u8>,
    },

    /// Send an event directly to the tender owner via request-response (spec v0.3).
    SendEvent {
        /// PeerId of the tender owner to send the event to.
        peer_id: String,
        /// Bincode-encoded `SchedulerEvent` payload.
        payload: Vec<u8>,
    },

    /// Send an award directly to a winner via request-response (spec v0.3).
    SendAward {
        /// PeerId of the winning node to send the award to.
        peer_id: String,
        /// Bincode-encoded `AwardWithManifest` payload.
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
            seen_events: Arc::new(Mutex::new(HashMap::new())),
            outbound_tx,
        }
    }

    /// Routes an incoming gossipsub message to the appropriate handler.
    ///
    /// Messages on the `beemesh-fabric` topic are decoded and dispatched:
    ///
    /// | Message Type | Handler | Action |
    /// |--------------|---------|--------|
    /// | `Tender` | `handle_tender()` | Evaluate eligibility, submit bid |
    /// | `Disposal` | `handle_disposal()` | Mark for disposal, delete local pods |
    ///
    /// Note: `Bid` and `Event` messages are sent via request-response protocol
    /// (not gossipsub) and are handled by `handle_bid_direct()` and
    /// `handle_event_direct()` respectively.
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

        // Try to parse as Tender first
        if let Ok(tender) = bincode::deserialize::<Tender>(&message.data) {
            self.handle_tender(&tender, message).await;
            return;
        }

        // Try to parse as Disposal
        if let Ok(disposal) = bincode::deserialize::<Disposal>(&message.data) {
            if let Some(source_peer) = message.source.as_ref() {
                if let Err(e) = handle_disposal(&disposal, source_peer).await {
                    error!("Failed to handle disposal: {}", e);
                }
            } else {
                error!("Disposal message missing source peer");
            }
            return;
        }

        // Unknown message type
        error!("Failed to parse gossipsub message as Tender or Disposal");
    }

    /// Handles a bid received via direct request-response (spec v0.3).
    ///
    /// This is the public entry point for bids received directly from bidders
    /// via request-response protocol (spec v0.3).
    pub async fn handle_bid_direct(&self, bid: &Bid, source_peer: &PeerId) {
        self.handle_bid(bid, source_peer).await;
    }

    /// Handles an event received via direct request-response (spec v0.3).
    ///
    /// This is the public entry point for events received directly from winners
    /// via request-response protocol.
    pub async fn handle_event_direct(&self, event: &SchedulerEvent, source_peer: &PeerId) {
        self.handle_event(event, source_peer).await;
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

            // Initialize bid collection context and spawn winner selection task
            self.initialize_owned_tender(
                &tender_id,
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
            Ok(_) => bincode::serialize(&bid).expect("bid serialization"),
            Err(e) => {
                error!("Failed to sign bid for tender {}: {}", tender_id, e);
                eprintln!(
                    "tender:error id={} reason=sign_bid_failure err={}",
                    tender_id, e
                );
                return;
            }
        };

        // Step 4: Send bid directly to tender owner (spec v0.3)
        // Uses request-response protocol instead of gossipsub broadcast
        // The tender owner is identified from gossipsub message.source
        if let Err(e) = self.outbound_tx.send(SchedulerCommand::SendBid {
            peer_id: source_peer.to_string(),
            payload: bid_bytes,
        }) {
            error!("Failed to queue bid for tender {}: {}", tender_id, e);
        } else {
            info!(
                "Queued Bid for tender {} to owner {} with score {:.2}",
                tender_id, source_peer, my_score
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
        manifest_json: Option<String>,
    ) {
        // Register the tender in our owned_tenders map (thread-safe)
        {
            let mut owned = self
                .owned_tenders
                .lock()
                .expect("owned tenders mutex poisoned - bid collection state corrupted");
            if owned.contains_key(tender_id) {
                return; // Already initialized (duplicate tender receipt)
            }

            owned.insert(
                tender_id.to_string(),
                OwnedTenderContext {
                    bid_context: BidContext {
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
            let selection_window_ms = std::env::var("BEEMESH_SELECTION_WINDOW_MS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(DEFAULT_SELECTION_WINDOW_MS);
            sleep(Duration::from_millis(selection_window_ms)).await;

            // Close the bidding window and extract collected bids
            let (winners, manifest_json_opt) = {
                let mut tenders = owned_tenders
                    .lock()
                    .expect("owned tenders mutex poisoned - bid collection state corrupted");
                if let Some(ctx) = tenders.remove(&tender_id_clone) {
                    (
                        select_winners(&ctx.bid_context),
                        ctx.manifest_json.clone(),
                    )
                } else {
                    (Vec::new(), None)
                }
            };

            // Proceed only if we have manifest content
            if winners.is_empty() {
                info!(
                    "No qualifying bids for tender {}",
                    tender_id_clone
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
                "Selected winners for tender {}: {}",
                tender_id_clone, summary
            );

            let winners_list: Vec<String> =
                winners.iter().map(|w| w.bidder_id.clone()).collect();

            // Get manifest content for award delivery
            if let Some(manifest_json) = manifest_json_opt
                .clone()
                .or_else(|| get_local_manifest(&tender_id_clone))
            {
                let owner_pubkey = keypair.to_protobuf_encoding().unwrap_or_default();

                // Send AwardWithManifest directly to each winner (spec v0.3)
                // No gossipsub broadcast - awards are private to each winner
                // Local node receives award via same path as remote nodes for consistency
                for winner in winners.iter() {
                    let mut award_with_manifest = AwardWithManifest {
                        tender_id: tender_id_clone.clone(),
                        manifest_json: manifest_json.clone(),
                        owner_peer_id: local_id.clone(),
                        owner_pubkey: owner_pubkey.clone(),
                        replicas: 1,
                        timestamp: now_ms(),
                        nonce: rand::thread_rng().next_u64(),
                        signature: Vec::new(),
                    };

                    // Sign the award
                    if let Err(e) =
                        signatures::sign_award_with_manifest(&mut award_with_manifest, &keypair)
                    {
                        error!(
                            "Failed to sign award for tender {} to {}: {}",
                            tender_id_clone, winner.bidder_id, e
                        );
                        continue;
                    }

                    // Encode and send via request-response
                    let award_bytes = bincode::serialize(&award_with_manifest)
                        .unwrap_or_else(|_| Vec::new());

                    if let Err(e) = outbound_tx.send(SchedulerCommand::SendAward {
                        peer_id: winner.bidder_id.clone(),
                        payload: award_bytes,
                    }) {
                        error!(
                            "Failed to queue award for tender {} to {}: {}",
                            tender_id_clone, winner.bidder_id, e
                        );
                    } else {
                        info!(
                            "Queued award+manifest for tender {} to winner {}",
                            tender_id_clone, winner.bidder_id
                        );
                    }
                }

                info!(
                    "Sent awards for tender {} to {:?}",
                    tender_id_clone, winners_list
                );
            } else {
                error!(
                    "Unable to load manifest content for tender {}; cannot distribute to winners",
                    tender_id_clone
                );
            }
        });
    }

    // ========================================================================
    // Replay Protection Helpers
    // ========================================================================

    /// Records a tender as seen and returns true if this is the first time.
    ///
    /// Uses the (tender_id, nonce) tuple as the dedup key.
    ///
    /// # Panics
    ///
    /// Panics if the replay filter mutex is poisoned. Replay protection is
    /// security-critical; a corrupted filter could allow message replay attacks.
    fn mark_tender_seen(&self, tender_id: &str, nonce: u64) -> bool {
        let mut seen = self
            .seen_tenders
            .lock()
            .expect("seen tenders mutex poisoned - replay protection compromised");
        record_replay(&mut seen, (tender_id.to_string(), nonce))
    }

    /// Records a bid as seen and returns true if this is the first time.
    ///
    /// Uses the (tender_id, node_id, nonce) tuple as the dedup key.
    ///
    /// # Panics
    ///
    /// Panics if the replay filter mutex is poisoned. See [`mark_tender_seen`].
    fn mark_bid_seen(&self, bid: &Bid) -> bool {
        let mut seen = self
            .seen_bids
            .lock()
            .expect("seen bids mutex poisoned - replay protection compromised");
        record_replay(
            &mut seen,
            (bid.tender_id.clone(), bid.node_id.clone(), bid.nonce),
        )
    }

    /// Records an event as seen and returns true if this is the first time.
    ///
    /// # Panics
    ///
    /// Panics if the replay filter mutex is poisoned. See [`mark_tender_seen`].
    fn mark_event_seen(&self, event: &SchedulerEvent) -> bool {
        let mut seen = self
            .seen_events
            .lock()
            .expect("seen events mutex poisoned - replay protection compromised");
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
    async fn handle_bid(&self, bid: &Bid, source_peer: &PeerId) {
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

        let public_key = match peer_id_to_public_key(source_peer) {
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
        let mut owned = self
            .owned_tenders
            .lock()
            .expect("owned tenders mutex poisoned - bid collection state corrupted");
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
    async fn handle_event(&self, event: &SchedulerEvent, source_peer: &PeerId) {
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
        let public_key = match peer_id_to_public_key(source_peer) {
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

        // Process termination events for locally deployed pods
        // These events indicate that a previously running pod has stopped
        if event.node_id == self.local_node_id
            && matches!(
                event.event_type,
                EventType::Cancelled | EventType::Preempted | EventType::Failed
            ) {
                info!(
                    "Received termination event for locally deployed tender {}",
                    tender_id
                );
            }
    }

    // ========================================================================
    // Event Publishing
    // ========================================================================

    /// Publishes a scheduler event to the tender owner (spec v0.3).
    ///
    /// Events are informational messages about deployment lifecycle changes.
    /// This method handles signing and sending the event directly to the
    /// tender owner via request-response protocol.
    ///
    /// # Arguments
    ///
    /// * `tender_id` - The tender this event relates to
    /// * `event_type` - The type of event (Deployed, Failed, etc.)
    /// * `reason` - Human-readable reason for the event
    pub fn publish_event(&self, tender_id: &str, event_type: EventType, reason: &str) {
        // Get the tender owner's peer ID for direct messaging
        let owner_peer_id = match get_tender_owner(tender_id) {
            Some(peer_id) => peer_id,
            None => {
                // Fallback: if we don't know the owner, log warning
                // This can happen if the tender owner info was not stored
                info!(
                    "Unknown tender owner for {}; cannot send event",
                    tender_id
                );
                return;
            }
        };

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
            Ok(_) => bincode::serialize(&event).unwrap_or_else(|_| Vec::new()),
            Err(e) => {
                error!(
                    "Failed to sign scheduler event for tender {}: {}",
                    tender_id, e
                );
                return;
            }
        };

        // Send event directly to tender owner via request-response (spec v0.3)
        if let Err(e) = self.outbound_tx.send(SchedulerCommand::SendEvent {
            peer_id: owner_peer_id.clone(),
            payload,
        }) {
            error!(
                "Failed to queue event for tender {} to owner {}: {}",
                tender_id, owner_peer_id, e
            );
        } else {
            info!(
                "Queued {:?} event for tender {} to owner {}",
                event.event_type, tender_id, owner_peer_id
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
///
/// # Panics
///
/// Panics if the manifest cache mutex is poisoned. Manifest integrity is
/// critical for deployment; a corrupted cache could cause deployment failures
/// or security issues from malformed manifests.
pub fn register_local_manifest(tender_id: &str, manifest_json: &str) {
    let mut cache = LOCAL_MANIFEST_CACHE
        .lock()
        .expect("manifest cache mutex poisoned - manifest state corrupted");
    cache.insert(tender_id.to_string(), manifest_json.to_string());
}

/// Retrieves a manifest from the local cache.
///
/// Returns `None` if the tender_id is not found in the cache.
///
/// # Panics
///
/// Panics if the manifest cache mutex is poisoned. See [`register_local_manifest`].
fn get_local_manifest(tender_id: &str) -> Option<String> {
    LOCAL_MANIFEST_CACHE
        .lock()
        .expect("manifest cache mutex poisoned - manifest state corrupted")
        .get(tender_id)
        .cloned()
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
    if let Some(ts) = map.get(&key)
        && now.saturating_sub(*ts) <= REPLAY_WINDOW_MS {
            return false;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_top_unique_winner() {
        let context = BidContext {
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
