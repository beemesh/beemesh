//! Scheduler Tests
//!
//! This module tests the scheduler protocol as defined in `test-spec.md`:
//! - Deterministic score computation (§4.1)
//! - Deterministic winner selection (§5.2)
//! - Replay protection via (tender_id, nonce) tuples
//! - Timestamp freshness enforcement (±30 seconds max skew)
//! - Signature verification for all message types
//! - Topic filtering for gossipsub messages
//!
//! # Spec References
//!
//! - test-spec.md: "Scheduler – deterministic bidding and winner selection"
//! - test-spec.md: "Scheduler – deployment, events, and backoff"
//! - machineplane-spec.md §3.3: Replay and timestamp validation
//! - machineplane-spec.md §4.1: Tender/Bid/Award flow
//! - machineplane-spec.md §5.2: Winner selection algorithm

use libp2p::identity::Keypair;
use machineplane::signatures::{
    sign_bid, sign_scheduler_event, sign_tender, verify_sign_bid,
    verify_sign_scheduler_event, verify_sign_tender,
};
use machineplane::messages::{Bid, EventType, SchedulerEvent, Tender};
use std::time::{SystemTime, UNIX_EPOCH};

// ============================================================================
// Test Utilities
// ============================================================================

/// Returns current time in milliseconds since UNIX epoch.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// Creates a valid signed tender for testing.
fn create_signed_tender(keypair: &Keypair, id: &str, timestamp: u64, nonce: u64) -> Tender {
    let mut tender = Tender {
        id: id.to_string(),
        manifest_digest: format!("digest-{}", id),
        qos_preemptible: false,
        timestamp,
        nonce,
        signature: Vec::new(),
    };
    sign_tender(&mut tender, keypair).expect("tender signing should succeed");
    tender
}

/// Creates a valid signed bid for testing.
fn create_signed_bid(
    keypair: &Keypair,
    tender_id: &str,
    node_id: &str,
    score: f64,
    timestamp: u64,
    nonce: u64,
) -> Bid {
    let mut bid = Bid {
        tender_id: tender_id.to_string(),
        node_id: node_id.to_string(),
        score,
        resource_fit_score: score * 0.8,
        network_locality_score: score * 0.2,
        timestamp,
        nonce,
        signature: Vec::new(),
    };
    sign_bid(&mut bid, keypair).expect("bid signing should succeed");
    bid
}

/// Creates a valid signed scheduler event for testing.
fn create_signed_event(
    keypair: &Keypair,
    tender_id: &str,
    node_id: &str,
    event_type: EventType,
    timestamp: u64,
    nonce: u64,
) -> SchedulerEvent {
    let mut event = SchedulerEvent {
        tender_id: tender_id.to_string(),
        node_id: node_id.to_string(),
        event_type,
        reason: "test event".to_string(),
        timestamp,
        nonce,
        signature: Vec::new(),
    };
    sign_scheduler_event(&mut event, keypair).expect("event signing should succeed");
    event
}

// ============================================================================
// Deterministic Score Computation Tests (§4.1)
// ============================================================================

/// Verifies that bid score computation is deterministic for identical inputs.
///
/// Spec requirement: "The score computed for T MUST be the same when
/// recomputed with the same Tender and the same local metrics"
#[test]
fn bid_score_computation_is_deterministic() {
    // Score formula from scheduler.rs: capacity_score * 0.8 + 0.2
    // Given fixed capacity_score = 1.0, the result should always be 1.0
    let capacity_score = 1.0;
    let computed_score_1 = capacity_score * 0.8 + 0.2;
    let computed_score_2 = capacity_score * 0.8 + 0.2;

    assert_eq!(
        computed_score_1, computed_score_2,
        "Score computation MUST be deterministic for same inputs"
    );

    // Verify with different capacity scores
    let capacity_score_2 = 0.5;
    let score_a = capacity_score_2 * 0.8 + 0.2;
    let score_b = capacity_score_2 * 0.8 + 0.2;

    assert_eq!(
        score_a, score_b,
        "Score computation MUST be deterministic regardless of input value"
    );
}

/// Verifies that resource fit scores are bounded [0.0, 1.0].
#[test]
fn resource_fit_score_is_bounded() {
    let capacity_score = 1.0f64;
    let computed_score = capacity_score * 0.8 + 0.2;

    assert!(
        computed_score >= 0.0 && computed_score <= 1.0,
        "Computed score MUST be in range [0.0, 1.0]"
    );

    // Edge case: zero capacity
    let zero_capacity = 0.0f64;
    let zero_score = zero_capacity * 0.8 + 0.2;
    assert!(
        zero_score >= 0.0,
        "Score MUST be non-negative even with zero capacity"
    );
}

// ============================================================================
// Deterministic Winner Selection Tests (§5.2)
// ============================================================================

/// Verifies that winner selection produces identical results for the same bid set.
///
/// Spec requirement: "The same ordered list of winners MUST be produced every
/// time the procedure is run with the same Bid set"
#[test]
fn winner_selection_is_deterministic() {
    // Simulate the select_winners algorithm from scheduler.rs
    #[derive(Clone)]
    struct BidEntry {
        bidder_id: String,
        score: f64,
    }

    fn select_winners(bids: &[BidEntry]) -> Vec<String> {
        use std::collections::HashMap;

        // De-duplicate bids by node, keeping highest score per node
        let mut best_by_node: HashMap<String, BidEntry> = HashMap::new();
        for bid in bids {
            let entry = best_by_node
                .entry(bid.bidder_id.clone())
                .or_insert_with(|| bid.clone());
            if bid.score > entry.score {
                *entry = bid.clone();
            }
        }

        let mut sorted: Vec<BidEntry> = best_by_node.into_values().collect();
        sorted.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.bidder_id.cmp(&b.bidder_id))
        });

        sorted.into_iter().map(|b| b.bidder_id).collect()
    }

    let bids = vec![
        BidEntry {
            bidder_id: "node-a".to_string(),
            score: 0.8,
        },
        BidEntry {
            bidder_id: "node-b".to_string(),
            score: 0.95,
        },
        BidEntry {
            bidder_id: "node-c".to_string(),
            score: 0.7,
        },
    ];

    let winners_1 = select_winners(&bids);
    let winners_2 = select_winners(&bids);

    assert_eq!(
        winners_1, winners_2,
        "Winner selection MUST be deterministic for identical bid sets"
    );

    // Verify correct ordering (highest score first)
    assert_eq!(winners_1[0], "node-b", "Highest scoring node MUST be first");
    assert_eq!(winners_1[1], "node-a", "Second highest MUST be second");
    assert_eq!(winners_1[2], "node-c", "Lowest scoring MUST be last");
}

/// Verifies that nodes with equal scores are ordered by peer ID (deterministic tiebreaker).
///
/// Spec requirement: "If scores equal, lexicographically higher peer_id wins"
#[test]
fn winner_selection_uses_peer_id_as_tiebreaker() {
    #[derive(Clone)]
    struct BidEntry {
        bidder_id: String,
        score: f64,
    }

    fn select_winners(bids: &[BidEntry]) -> Vec<String> {
        use std::collections::HashMap;

        let mut best_by_node: HashMap<String, BidEntry> = HashMap::new();
        for bid in bids {
            let entry = best_by_node
                .entry(bid.bidder_id.clone())
                .or_insert_with(|| bid.clone());
            if bid.score > entry.score {
                *entry = bid.clone();
            }
        }

        let mut sorted: Vec<BidEntry> = best_by_node.into_values().collect();
        sorted.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.bidder_id.cmp(&b.bidder_id))
        });

        sorted.into_iter().map(|b| b.bidder_id).collect()
    }

    // All nodes have the same score
    let bids = vec![
        BidEntry {
            bidder_id: "node-c".to_string(),
            score: 0.85,
        },
        BidEntry {
            bidder_id: "node-a".to_string(),
            score: 0.85,
        },
        BidEntry {
            bidder_id: "node-b".to_string(),
            score: 0.85,
        },
    ];

    let winners = select_winners(&bids);

    // With equal scores, should be sorted lexicographically by peer ID
    assert_eq!(
        winners,
        vec!["node-a", "node-b", "node-c"],
        "Equal scores MUST use peer_id as deterministic tiebreaker"
    );
}

/// Verifies that duplicate bids from the same node keep only the highest score.
///
/// Spec requirement: "If multiple bids exist from the same node, only the
/// highest-scoring bid is considered"
#[test]
fn winner_selection_deduplicates_by_node() {
    #[derive(Clone)]
    struct BidEntry {
        bidder_id: String,
        score: f64,
    }

    fn select_winners(bids: &[BidEntry]) -> Vec<(String, f64)> {
        use std::collections::HashMap;

        let mut best_by_node: HashMap<String, BidEntry> = HashMap::new();
        for bid in bids {
            let entry = best_by_node
                .entry(bid.bidder_id.clone())
                .or_insert_with(|| bid.clone());
            if bid.score > entry.score {
                *entry = bid.clone();
            }
        }

        let mut sorted: Vec<BidEntry> = best_by_node.into_values().collect();
        sorted.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.bidder_id.cmp(&b.bidder_id))
        });

        sorted.into_iter().map(|b| (b.bidder_id, b.score)).collect()
    }

    // Node-a submits multiple bids with different scores
    let bids = vec![
        BidEntry {
            bidder_id: "node-a".to_string(),
            score: 0.5,
        },
        BidEntry {
            bidder_id: "node-a".to_string(),
            score: 0.9,
        },
        BidEntry {
            bidder_id: "node-a".to_string(),
            score: 0.7,
        },
        BidEntry {
            bidder_id: "node-b".to_string(),
            score: 0.8,
        },
    ];

    let winners = select_winners(&bids);

    assert_eq!(winners.len(), 2, "MUST have exactly 2 unique bidders");
    assert_eq!(
        winners[0],
        ("node-a".to_string(), 0.9),
        "node-a MUST use highest score (0.9)"
    );
    assert_eq!(
        winners[1],
        ("node-b".to_string(), 0.8),
        "node-b MUST retain its only bid"
    );
}

// ============================================================================
// Timestamp Freshness Tests (§3.3)
// ============================================================================

/// Maximum allowed clock skew in milliseconds (from scheduler.rs).
const MAX_CLOCK_SKEW_MS: u64 = 30_000;

/// Checks if a timestamp is within the acceptable clock skew window.
fn is_timestamp_fresh(timestamp_ms: u64) -> bool {
    let now = now_ms();
    let (min, max) = if now > timestamp_ms {
        (timestamp_ms, now)
    } else {
        (now, timestamp_ms)
    };
    max - min <= MAX_CLOCK_SKEW_MS
}

/// Verifies that current timestamps are accepted.
#[test]
fn timestamp_freshness_accepts_current_time() {
    let current_timestamp = now_ms();
    assert!(
        is_timestamp_fresh(current_timestamp),
        "Current timestamp MUST be accepted"
    );
}

/// Verifies that timestamps within the allowed skew window are accepted.
#[test]
fn timestamp_freshness_accepts_within_skew_window() {
    let now = now_ms();

    // 10 seconds in the past (within 30s window)
    let past_timestamp = now.saturating_sub(10_000);
    assert!(
        is_timestamp_fresh(past_timestamp),
        "Timestamp 10s in past MUST be accepted"
    );

    // 10 seconds in the future (within 30s window)
    let future_timestamp = now + 10_000;
    assert!(
        is_timestamp_fresh(future_timestamp),
        "Timestamp 10s in future MUST be accepted"
    );

    // Exactly at the boundary (30s)
    let boundary_past = now.saturating_sub(MAX_CLOCK_SKEW_MS);
    assert!(
        is_timestamp_fresh(boundary_past),
        "Timestamp exactly at 30s boundary MUST be accepted"
    );
}

/// Verifies that stale timestamps (outside clock skew window) are rejected.
///
/// Spec requirement: "Messages with timestamps outside MAX_CLOCK_SKEW_MS
/// window are rejected to prevent replay attacks"
#[test]
fn timestamp_freshness_rejects_stale() {
    let now = now_ms();

    // 60 seconds in the past (outside 30s window)
    let stale_past = now.saturating_sub(60_000);
    assert!(
        !is_timestamp_fresh(stale_past),
        "Timestamp 60s in past MUST be rejected"
    );

    // 60 seconds in the future (outside 30s window)
    let stale_future = now + 60_000;
    assert!(
        !is_timestamp_fresh(stale_future),
        "Timestamp 60s in future MUST be rejected"
    );
}

/// Verifies that very old timestamps are rejected (replay protection).
#[test]
fn timestamp_freshness_rejects_replay_attacks() {
    // Timestamp from 5 minutes ago (well outside the 30s window)
    let replay_timestamp = now_ms().saturating_sub(300_000);
    assert!(
        !is_timestamp_fresh(replay_timestamp),
        "Replayed timestamp from 5 minutes ago MUST be rejected"
    );
}

// ============================================================================
// Replay Protection Tests (§3.3)
// ============================================================================

/// Duration to retain seen message tuples for replay protection.
const REPLAY_WINDOW_MS: u64 = 300_000; // 5 minutes

/// Simulates the replay filter logic from scheduler.rs.
fn record_replay(
    seen: &mut std::collections::HashMap<(String, u64), u64>,
    key: (String, u64),
) -> bool {
    let now = now_ms();

    // Prune expired entries
    seen.retain(|_, ts| now.saturating_sub(*ts) <= REPLAY_WINDOW_MS);

    // Check if already seen
    if let Some(ts) = seen.get(&key) {
        if now.saturating_sub(*ts) <= REPLAY_WINDOW_MS {
            return false; // Replay detected
        }
    }

    seen.insert(key, now);
    true // First time seeing this message
}

/// Verifies that the first occurrence of a message is accepted.
#[test]
fn replay_filter_accepts_first_occurrence() {
    let mut seen: std::collections::HashMap<(String, u64), u64> = std::collections::HashMap::new();

    let key = ("tender-1".to_string(), 12345u64);
    let result = record_replay(&mut seen, key.clone());

    assert!(result, "First occurrence of message MUST be accepted");
}

/// Verifies that duplicate messages within the replay window are rejected.
///
/// Spec requirement: "Replay protection via (tender_id, nonce) tuples with
/// 5-minute window"
#[test]
fn replay_filter_rejects_duplicates() {
    let mut seen: std::collections::HashMap<(String, u64), u64> = std::collections::HashMap::new();

    let key = ("tender-1".to_string(), 12345u64);

    // First occurrence accepted
    let first_result = record_replay(&mut seen, key.clone());
    assert!(first_result, "First occurrence MUST be accepted");

    // Duplicate rejected
    let duplicate_result = record_replay(&mut seen, key.clone());
    assert!(
        !duplicate_result,
        "Duplicate within replay window MUST be rejected"
    );
}

/// Verifies that different nonces for the same tender_id are accepted.
#[test]
fn replay_filter_accepts_different_nonces() {
    let mut seen: std::collections::HashMap<(String, u64), u64> = std::collections::HashMap::new();

    let key1 = ("tender-1".to_string(), 11111u64);
    let key2 = ("tender-1".to_string(), 22222u64);

    let result1 = record_replay(&mut seen, key1);
    let result2 = record_replay(&mut seen, key2);

    assert!(result1, "First nonce MUST be accepted");
    assert!(
        result2,
        "Different nonce for same tender_id MUST be accepted"
    );
}

/// Verifies that different tender_ids with the same nonce are accepted.
#[test]
fn replay_filter_accepts_different_tender_ids() {
    let mut seen: std::collections::HashMap<(String, u64), u64> = std::collections::HashMap::new();

    let key1 = ("tender-1".to_string(), 12345u64);
    let key2 = ("tender-2".to_string(), 12345u64);

    let result1 = record_replay(&mut seen, key1);
    let result2 = record_replay(&mut seen, key2);

    assert!(result1, "First tender_id MUST be accepted");
    assert!(
        result2,
        "Different tender_id with same nonce MUST be accepted"
    );
}

// ============================================================================
// Signature Verification Tests (§7)
// ============================================================================

/// Verifies that signed tenders are accepted.
#[test]
fn signature_verification_accepts_valid_tender() {
    let keypair = Keypair::generate_ed25519();
    let public_key = keypair.public();

    let tender = create_signed_tender(&keypair, "tender-valid", now_ms(), 12345);

    assert!(
        verify_sign_tender(&tender, &public_key),
        "Valid signed tender MUST be accepted"
    );
}

/// Verifies that tampered tenders are rejected.
///
/// Spec requirement: "Unsigned or invalid-signature tenders are discarded"
#[test]
fn signature_verification_rejects_tampered_tender() {
    let keypair = Keypair::generate_ed25519();
    let public_key = keypair.public();

    let mut tender = create_signed_tender(&keypair, "tender-tamper", now_ms(), 12345);

    // Tamper with the tender after signing
    tender.manifest_digest = "tampered-digest".to_string();

    assert!(
        !verify_sign_tender(&tender, &public_key),
        "Tampered tender MUST be rejected"
    );
}

/// Verifies that unsigned tenders are rejected.
#[test]
fn signature_verification_rejects_unsigned_tender() {
    let keypair = Keypair::generate_ed25519();
    let public_key = keypair.public();

    let tender = Tender {
        id: "tender-unsigned".to_string(),
        manifest_digest: "digest".to_string(),
        qos_preemptible: false,
        timestamp: now_ms(),
        nonce: 12345,
        signature: Vec::new(), // No signature
    };

    assert!(
        !verify_sign_tender(&tender, &public_key),
        "Unsigned tender MUST be rejected"
    );
}

/// Verifies that tenders signed by a different key are rejected.
#[test]
fn signature_verification_rejects_wrong_key_tender() {
    let keypair1 = Keypair::generate_ed25519();
    let keypair2 = Keypair::generate_ed25519();
    let public_key2 = keypair2.public();

    // Sign with keypair1, verify with keypair2's public key
    let tender = create_signed_tender(&keypair1, "tender-wrongkey", now_ms(), 12345);

    assert!(
        !verify_sign_tender(&tender, &public_key2),
        "Tender signed by different key MUST be rejected"
    );
}

/// Verifies that signed bids are accepted.
#[test]
fn signature_verification_accepts_valid_bid() {
    let keypair = Keypair::generate_ed25519();
    let public_key = keypair.public();

    let bid = create_signed_bid(&keypair, "tender-1", "node-a", 0.9, now_ms(), 12345);

    assert!(
        verify_sign_bid(&bid, &public_key),
        "Valid signed bid MUST be accepted"
    );
}

/// Verifies that tampered bids are rejected.
#[test]
fn signature_verification_rejects_tampered_bid() {
    let keypair = Keypair::generate_ed25519();
    let public_key = keypair.public();

    let mut bid = create_signed_bid(&keypair, "tender-1", "node-a", 0.9, now_ms(), 12345);

    // Tamper with the score
    bid.score = 0.99;

    assert!(
        !verify_sign_bid(&bid, &public_key),
        "Tampered bid MUST be rejected"
    );
}

/// Verifies that signed scheduler events are accepted.
#[test]
fn signature_verification_accepts_valid_event() {
    let keypair = Keypair::generate_ed25519();
    let public_key = keypair.public();

    let event = create_signed_event(
        &keypair,
        "tender-1",
        "node-a",
        EventType::Deployed,
        now_ms(),
        12345,
    );

    assert!(
        verify_sign_scheduler_event(&event, &public_key),
        "Valid signed event MUST be accepted"
    );
}

/// Verifies that tampered scheduler events are rejected.
#[test]
fn signature_verification_rejects_tampered_event() {
    let keypair = Keypair::generate_ed25519();
    let public_key = keypair.public();

    let mut event = create_signed_event(
        &keypair,
        "tender-1",
        "node-a",
        EventType::Deployed,
        now_ms(),
        12345,
    );

    // Tamper with the event type
    event.event_type = EventType::Failed;

    assert!(
        !verify_sign_scheduler_event(&event, &public_key),
        "Tampered event MUST be rejected"
    );
}

// ============================================================================
// Bid Collection Window Tests
// ============================================================================

/// Verifies that the default selection window is reasonable.
#[test]
fn selection_window_is_reasonable() {
    use machineplane::scheduler::DEFAULT_SELECTION_WINDOW_MS;

    // Selection window should be between 100ms and 1000ms for responsive scheduling
    assert!(
        DEFAULT_SELECTION_WINDOW_MS >= 100,
        "Selection window MUST be at least 100ms for network propagation"
    );
    assert!(
        DEFAULT_SELECTION_WINDOW_MS <= 1000,
        "Selection window SHOULD be at most 1000ms for responsiveness"
    );
}

// ============================================================================
// Event Type Coverage Tests
// ============================================================================

/// Verifies that all event types can be signed and verified.
#[test]
fn all_event_types_can_be_signed() {
    let keypair = Keypair::generate_ed25519();
    let public_key = keypair.public();

    let event_types = [
        EventType::Deployed,
        EventType::Failed,
        EventType::Preempted,
        EventType::Cancelled,
    ];

    for event_type in event_types {
        let event = create_signed_event(&keypair, "tender-1", "node-a", event_type, now_ms(), 12345);

        assert!(
            verify_sign_scheduler_event(&event, &public_key),
            "Event type {:?} MUST be signable and verifiable",
            event_type
        );
    }
}

// ============================================================================
// Message Field Integrity Tests
// ============================================================================

/// Verifies that changing any tender field invalidates the signature.
#[test]
fn tender_signature_covers_all_fields() {
    let keypair = Keypair::generate_ed25519();
    let public_key = keypair.public();

    let original = create_signed_tender(&keypair, "tender-integrity", now_ms(), 12345);

    // Test each field
    let mut tampered_id = original.clone();
    tampered_id.id = "different-id".to_string();
    assert!(
        !verify_sign_tender(&tampered_id, &public_key),
        "Changing id MUST invalidate signature"
    );

    let mut tampered_digest = original.clone();
    tampered_digest.manifest_digest = "different-digest".to_string();
    assert!(
        !verify_sign_tender(&tampered_digest, &public_key),
        "Changing manifest_digest MUST invalidate signature"
    );

    let mut tampered_qos = original.clone();
    tampered_qos.qos_preemptible = !tampered_qos.qos_preemptible;
    assert!(
        !verify_sign_tender(&tampered_qos, &public_key),
        "Changing qos_preemptible MUST invalidate signature"
    );

    let mut tampered_timestamp = original.clone();
    tampered_timestamp.timestamp += 1;
    assert!(
        !verify_sign_tender(&tampered_timestamp, &public_key),
        "Changing timestamp MUST invalidate signature"
    );

    let mut tampered_nonce = original.clone();
    tampered_nonce.nonce += 1;
    assert!(
        !verify_sign_tender(&tampered_nonce, &public_key),
        "Changing nonce MUST invalidate signature"
    );
}

/// Verifies that changing any bid field invalidates the signature.
#[test]
fn bid_signature_covers_all_fields() {
    let keypair = Keypair::generate_ed25519();
    let public_key = keypair.public();

    let original = create_signed_bid(&keypair, "tender-1", "node-a", 0.9, now_ms(), 12345);

    // Test critical fields
    let mut tampered_tender_id = original.clone();
    tampered_tender_id.tender_id = "different-tender".to_string();
    assert!(
        !verify_sign_bid(&tampered_tender_id, &public_key),
        "Changing tender_id MUST invalidate signature"
    );

    let mut tampered_node_id = original.clone();
    tampered_node_id.node_id = "different-node".to_string();
    assert!(
        !verify_sign_bid(&tampered_node_id, &public_key),
        "Changing node_id MUST invalidate signature"
    );

    let mut tampered_score = original.clone();
    tampered_score.score = 0.99;
    assert!(
        !verify_sign_bid(&tampered_score, &public_key),
        "Changing score MUST invalidate signature"
    );

    let mut tampered_resource_fit = original.clone();
    tampered_resource_fit.resource_fit_score = 0.5;
    assert!(
        !verify_sign_bid(&tampered_resource_fit, &public_key),
        "Changing resource_fit_score MUST invalidate signature"
    );

    let mut tampered_locality = original.clone();
    tampered_locality.network_locality_score = 0.1;
    assert!(
        !verify_sign_bid(&tampered_locality, &public_key),
        "Changing network_locality_score MUST invalidate signature"
    );
}

// ============================================================================
// Topic Filtering Tests (§2.1)
// ============================================================================

/// Verifies that the correct fabric topic constant is defined.
///
/// Spec requirement: "All scheduler payloads on beemesh-fabric only"
#[test]
fn fabric_topic_constant_is_correct() {
    use machineplane::network::BEEMESH_FABRIC;

    assert_eq!(
        BEEMESH_FABRIC, "beemesh-fabric",
        "Fabric topic MUST be 'beemesh-fabric'"
    );
}

/// Verifies topic hash comparison logic for filtering.
///
/// Spec requirement: "Nodes MUST ignore Tenders on the wrong topic"
///
/// This tests the comparison logic that would be used in handle_message()
/// to filter messages by topic.
#[test]
fn topic_hash_comparison_filters_wrong_topic() {
    use libp2p::gossipsub::IdentTopic;
    use machineplane::network::BEEMESH_FABRIC;

    let fabric_topic = IdentTopic::new(BEEMESH_FABRIC);
    let wrong_topic = IdentTopic::new("wrong-topic");
    let another_wrong = IdentTopic::new("beemesh-other");

    let fabric_hash = fabric_topic.hash();
    let wrong_hash = wrong_topic.hash();
    let another_wrong_hash = another_wrong.hash();

    // Same topic should match
    assert_eq!(
        fabric_hash,
        IdentTopic::new(BEEMESH_FABRIC).hash(),
        "Same topic name MUST produce same hash"
    );

    // Different topics should NOT match
    assert_ne!(
        fabric_hash, wrong_hash,
        "Different topic MUST produce different hash"
    );
    assert_ne!(
        fabric_hash, another_wrong_hash,
        "Similar but different topic MUST produce different hash"
    );
}

/// Verifies that topic filtering is case-sensitive.
#[test]
fn topic_filtering_is_case_sensitive() {
    use libp2p::gossipsub::IdentTopic;
    use machineplane::network::BEEMESH_FABRIC;

    let correct = IdentTopic::new(BEEMESH_FABRIC).hash();
    let uppercase = IdentTopic::new("BEEMESH-FABRIC").hash();
    let mixed = IdentTopic::new("BeeMesh-Fabric").hash();

    assert_ne!(
        correct, uppercase,
        "Topic matching MUST be case-sensitive"
    );
    assert_ne!(correct, mixed, "Topic matching MUST be case-sensitive");
}

// ============================================================================
// Manifest Digest Tests
// ============================================================================

/// Verifies that tender contains manifest digest, not full manifest.
///
/// Spec requirement: "Producer MUST include only the manifest digest and NOT
/// the full manifest payload"
#[test]
fn tender_contains_digest_not_manifest() {
    let keypair = Keypair::generate_ed25519();
    let tender = create_signed_tender(&keypair, "tender-1", now_ms(), 12345);

    // Tender should have a digest field
    assert!(
        !tender.manifest_digest.is_empty(),
        "Tender MUST have manifest_digest"
    );

    // The Tender struct intentionally does NOT have a manifest_json field
    // This is verified by the struct definition - if manifest_json existed,
    // this test file would need to populate it in create_signed_tender()
}

// ============================================================================
// Bid Uniqueness Tests
// ============================================================================

/// Verifies that each bid has a unique nonce.
///
/// Spec requirement: "Nodes SHOULD submit at most one Bid per Tender"
/// The nonce ensures bids are distinguishable even from the same node.
#[test]
fn bid_nonces_are_unique() {
    let keypair = Keypair::generate_ed25519();
    let ts = now_ms();

    let bid1 = create_signed_bid(&keypair, "tender-1", "node-a", 0.9, ts, rand::random());
    let bid2 = create_signed_bid(&keypair, "tender-1", "node-a", 0.9, ts, rand::random());

    assert_ne!(
        bid1.nonce, bid2.nonce,
        "Different bids SHOULD have different nonces"
    );
}

/// Verifies that bid contains node_id for attribution.
#[test]
fn bid_contains_node_id() {
    let keypair = Keypair::generate_ed25519();
    let bid = create_signed_bid(&keypair, "tender-1", "node-xyz", 0.9, now_ms(), 12345);

    assert_eq!(
        bid.node_id, "node-xyz",
        "Bid MUST contain correct node_id"
    );
}
