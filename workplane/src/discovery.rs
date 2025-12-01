//! # Workload DHT (WDHT) – In-Memory Service Discovery Cache
//!
//! This module implements the per-workload service discovery cache described in the
//! workplane specification (§5). It maintains an in-memory view of all known replicas
//! for a workload, with TTL-based expiration and conflict resolution.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │                        WDHT Cache                                   │
//! │  ┌───────────────────────────────────────────────────────────────┐  │
//! │  │  workload_id: "default/nginx"                                 │  │
//! │  │  ┌─────────────────────────────────────────────────────────┐  │  │
//! │  │  │  peer_id: "12D3KooW..."                                 │  │  │
//! │  │  │  ├── record: ServiceRecord { ... }                      │  │  │
//! │  │  │  └── expires_at: Instant                                │  │  │
//! │  │  ├─────────────────────────────────────────────────────────┤  │  │
//! │  │  │  peer_id: "12D3KooX..."                                 │  │  │
//! │  │  │  └── ...                                                │  │  │
//! │  │  └─────────────────────────────────────────────────────────┘  │  │
//! │  └───────────────────────────────────────────────────────────────┘  │
//! └─────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Record Lifecycle
//!
//! 1. **Put**: New records are inserted via [`put`]. Records with timestamps outside
//!    ±30s of the current time are rejected to prevent clock skew attacks.
//!
//! 2. **TTL Expiration**: Each record has an expiration time. Expired records are
//!    purged lazily during [`put`] and [`list`] operations.
//!
//! 3. **Conflict Resolution**: When a record already exists for a peer, the incoming
//!    record replaces it if:
//!    - `incoming.version > existing.version`, OR
//!    - Same version AND `incoming.ts > existing.ts`, OR
//!    - Same version and timestamp AND `incoming.peer_id > existing.peer_id`
//!
//! 4. **Removal**: Explicit removal via [`remove`] when a replica is terminated.
//!
//! ## Thread Safety
//!
//! The cache is protected by a global `Mutex` and can be safely accessed from
//! multiple tokio tasks. The lock is held briefly during each operation.

use std::collections::{HashMap, hash_map::Entry};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// Maximum allowed clock skew in milliseconds (±30 seconds per spec §5.4).
///
/// Records with timestamps outside this window relative to the local clock are
/// rejected to prevent replay attacks or clock manipulation.
const MAX_CLOCK_SKEW_MS: i64 = 30_000;

/// A service record published by a workload replica to the WDHT.
///
/// Each replica periodically publishes its [`ServiceRecord`] to advertise its
/// presence and health status. Records are keyed by `(workload_id, peer_id)`.
///
/// ## Fields
///
/// - `workload_id`: Composite key `{namespace}/{kind}/{workload_name}`
/// - `peer_id`: libp2p peer ID of the publishing replica
/// - `version`: Monotonically increasing version for conflict resolution
/// - `ts`: Unix timestamp in milliseconds when record was created
/// - `ready`/`healthy`: Health probe results (see selfheal module)
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct ServiceRecord {
    /// Unique workload identifier: `{namespace}/{kind}/{workload_name}`.
    pub workload_id: String,

    /// Kubernetes-style namespace.
    pub namespace: String,

    /// Workload name (e.g., "nginx").
    pub workload_name: String,

    /// libp2p peer ID of this replica.
    pub peer_id: String,

    /// Pod name for stateful workloads (e.g., "redis-0").
    #[serde(default)]
    pub pod_name: Option<String>,

    /// Stateful workload ordinal (0, 1, 2, ...). Extracted from pod name.
    #[serde(default)]
    pub ordinal: Option<u32>,

    /// libp2p multiaddrs where this replica is reachable.
    #[serde(default)]
    pub addrs: Vec<String>,

    /// Capability metadata (custom key-value pairs).
    #[serde(default)]
    pub caps: serde_json::Map<String, serde_json::Value>,

    /// Record version. Higher version wins in conflict resolution.
    pub version: u64,

    /// Unix timestamp in milliseconds. Used for freshness and ordering.
    pub ts: i64,

    /// Random nonce for uniqueness (prevents replay).
    pub nonce: String,

    /// Readiness probe result. `true` if ready to receive traffic.
    pub ready: bool,

    /// Liveness probe result. `true` if the workload is alive.
    pub healthy: bool,
}

/// Internal entry wrapping a record with its expiration time.
struct RecordEntry {
    record: ServiceRecord,
    expires_at: Instant,
}

/// Global WDHT cache: `workload_id → (peer_id → RecordEntry)`.
static WDHT: LazyLock<Mutex<HashMap<String, HashMap<String, RecordEntry>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Insert or update a service record in the WDHT cache.
///
/// The record is accepted if:
/// 1. Its timestamp is within ±30s of the current time (freshness check)
/// 2. No existing record exists for this peer, OR the incoming record wins
///    conflict resolution (higher version, newer timestamp, or peer ID tiebreaker)
///
/// # Arguments
///
/// * `record` - The service record to insert
/// * `ttl` - Time-to-live before the record expires
///
/// # Returns
///
/// `true` if the record was inserted/updated, `false` if rejected.
pub fn put(record: ServiceRecord, ttl: Duration) -> bool {
    // Reject records with stale or future timestamps (clock skew protection)
    if !is_fresh(&record) {
        return false;
    }

    let mut dht = WDHT.lock().expect("wdht lock");
    let namespace_map = dht
        .entry(record.workload_id.clone())
        .or_default();

    // Lazily purge expired entries
    purge_expired(namespace_map);

    match namespace_map.entry(record.peer_id.clone()) {
        Entry::Occupied(mut occupied) => {
            // Conflict resolution: only replace if incoming record wins
            if should_replace(&occupied.get().record, &record) {
                occupied.insert(RecordEntry {
                    record,
                    expires_at: Instant::now() + ttl,
                });
                true
            } else {
                false
            }
        }
        Entry::Vacant(vacant) => {
            // New peer - always insert
            vacant.insert(RecordEntry {
                record,
                expires_at: Instant::now() + ttl,
            });
            true
        }
    }
}

/// Remove a service record from the WDHT cache.
///
/// Called when a replica is known to be terminating (e.g., after successful
/// removal request to machineplane) to immediately reflect the change.
///
/// # Arguments
///
/// * `workload_id` - The workload identifier (`{namespace}/{kind}/{workload_name}`)
/// * `peer_id` - The peer ID of the replica to remove
pub fn remove(workload_id: &str, peer_id: &str) {
    let mut dht = WDHT.lock().expect("wdht lock");
    if let Some(map) = dht.get_mut(workload_id) {
        map.remove(peer_id);
        // Clean up empty workload entries
        if map.is_empty() {
            dht.remove(workload_id);
        }
    }
}

/// List all active service records for a workload.
///
/// Returns all non-expired records for the specified workload, sorted by
/// timestamp (newest first). Expired records are purged as a side effect.
///
/// # Arguments
///
/// * `namespace` - Kubernetes-style namespace
/// * `kind` - Workload kind (e.g., "Deployment", "StatefulSet")
/// * `workload_name` - Workload name
///
/// # Returns
///
/// Vector of service records, sorted by descending timestamp.
pub fn list(namespace: &str, kind: &str, workload_name: &str) -> Vec<ServiceRecord> {
    let workload_id = format!("{namespace}/{kind}/{workload_name}");
    let mut dht = WDHT.lock().expect("wdht lock");
    let Some(map) = dht.get_mut(&workload_id) else {
        return Vec::new();
    };

    // Lazily purge expired entries
    purge_expired(map);

    let mut records: Vec<ServiceRecord> = map.values().map(|entry| entry.record.clone()).collect();
    // Sort by timestamp descending (newest first)
    records.sort_by(|a, b| b.ts.cmp(&a.ts));
    records
}

/// Remove all expired entries from the given peer map.
///
/// This is called lazily during [`put`] and [`list`] operations to keep
/// memory bounded without requiring a background cleanup task.
fn purge_expired(map: &mut HashMap<String, RecordEntry>) {
    let now = Instant::now();
    map.retain(|_, entry| entry.expires_at > now);
}

/// Determine if an incoming record should replace an existing one.
///
/// Conflict resolution order (per spec §5.3):
/// 1. Higher `version` wins
/// 2. If versions equal, higher `ts` (timestamp) wins
/// 3. If both equal, lexicographically higher `peer_id` wins (deterministic tiebreaker)
fn should_replace(existing: &ServiceRecord, incoming: &ServiceRecord) -> bool {
    if incoming.version > existing.version {
        return true;
    }
    if incoming.version < existing.version {
        return false;
    }
    // Versions equal - compare timestamps
    if incoming.ts > existing.ts {
        return true;
    }
    if incoming.ts < existing.ts {
        return false;
    }
    // Both version and timestamp equal - deterministic tiebreaker
    incoming.peer_id > existing.peer_id
}

/// Check if a record's timestamp is within the acceptable freshness window.
///
/// Rejects records whose timestamps differ from the local clock by more than
/// [`MAX_CLOCK_SKEW_MS`] (±30 seconds). This prevents:
/// - Replay attacks using old records
/// - Clock manipulation attacks with future timestamps
fn is_fresh(record: &ServiceRecord) -> bool {
    let now = current_millis();
    (record.ts - now).abs() <= MAX_CLOCK_SKEW_MS
}

/// Get current Unix time in milliseconds.
fn current_millis() -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    now.as_millis() as i64
}
