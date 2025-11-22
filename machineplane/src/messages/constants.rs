// Consolidated libp2p-related constants for topics, message prefixes, protocol names, etc.

/// Ident topic used for gossipsub in the beemesh fabric.
pub const BEEMESH_FABRIC: &str = "beemesh-fabric";

// === GOSSIPSUB TOPICS ===

/// Topic used for scheduler tenders
pub const SCHEDULER_TENDERS: &str = "scheduler-tenders";

/// Topic used for scheduler events
pub const SCHEDULER_EVENTS: &str = "scheduler-events";
/// Alias for backwards compatibility
pub const TOPIC_EVENTS: &str = SCHEDULER_EVENTS;

/// Topic used for scheduler proposals / capacity requests
pub const SCHEDULER_PROPOSALS: &str = "scheduler-proposals";
/// Alias for backwards compatibility
pub const TOPIC_PROPOSALS: &str = SCHEDULER_PROPOSALS;

// === MESSAGE PREFIXES ===

/// Prefix used for handshake messages exchanged on the gossip topic.
pub const HANDSHAKE_PREFIX: &str = "beemesh-handshake";

/// Prefix used when querying peers for free capacity (gossipsub message topic payload prefix).
pub const FREE_CAPACITY_PREFIX: &str = "beemesh-free-capacity";

/// Prefix used for replies to free-capacity queries.
pub const FREE_CAPACITY_REPLY_PREFIX: &str = "beemesh-free-capacity-reply";

/// Prefix used for lease-related operations
pub const LEASE_PREFIX: &str = "lease/";

// === TIMEOUTS AND TIMING ===

/// Timeout, in milliseconds, to wait for free-capacity responses from peers.
///
/// A longer window gives slower nodes time to answer capacity queries so
/// replica scheduling (e.g., replicas=3) can gather enough candidates instead
/// of failing with HTTP 503 due to under-counted peers.
pub const FREE_CAPACITY_TIMEOUT_MS: u64 = 2000;

/// Timeout, in seconds, to wait for request-response RPCs (ApplyRequest/ApplyResponse)
pub const REQUEST_RESPONSE_TIMEOUT_SECS: u64 = 3;

/// Default selection window in milliseconds for scheduler operations
pub const DEFAULT_SELECTION_WINDOW_MS: u64 = 250;

/// Default lease TTL in milliseconds
pub const DEFAULT_LEASE_TTL_MS: u64 = 3000;

// === MANIFEST FIELDS ===

/// JSON field name used for replica count in manifests (top-level `replicas`).
pub const REPLICAS_FIELD: &str = "replicas";

/// JSON path field used for replica count in manifests under `spec.replicas`.
pub const SPEC_REPLICAS_FIELD: &str = "spec";

// === RESOURCE MANAGEMENT CONSTANTS ===

/// Maximum percentage of CPU that can be allocated to workloads (90% to leave headroom)
pub const MAX_CPU_ALLOCATION_PERCENT: u8 = 90;

/// Maximum percentage of memory that can be allocated to workloads (90% to leave headroom)
pub const MAX_MEMORY_ALLOCATION_PERCENT: u8 = 90;

/// Maximum percentage of storage that can be allocated to workloads (90% to leave headroom)
pub const MAX_STORAGE_ALLOCATION_PERCENT: u8 = 90;

/// Minimum free memory to keep available in bytes (512 MB)
pub const MIN_FREE_MEMORY_BYTES: u64 = 512 * 1024 * 1024;

/// Minimum free storage to keep available in bytes (1 GB)
pub const MIN_FREE_STORAGE_BYTES: u64 = 1024 * 1024 * 1024;

/// Maximum number of workloads per node (0 = unlimited)
pub const MAX_WORKLOADS_PER_NODE: u32 = 0;

/// Timeout for resource availability checks in milliseconds
pub const RESOURCE_CHECK_TIMEOUT_MS: u64 = 1000;

/// Default CPU request in millicores if not specified in manifest (100m = 0.1 core)
pub const DEFAULT_CPU_REQUEST_MILLI: u32 = 100;

/// Default memory request in bytes if not specified in manifest (128 MB)
pub const DEFAULT_MEMORY_REQUEST_BYTES: u64 = 128 * 1024 * 1024;

/// Default storage request in bytes if not specified in manifest (1 GB)
pub const DEFAULT_STORAGE_REQUEST_BYTES: u64 = 1024 * 1024 * 1024;
