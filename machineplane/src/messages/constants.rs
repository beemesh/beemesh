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

/// Topic used for scheduler proposals
pub const SCHEDULER_PROPOSALS: &str = "scheduler-proposals";
/// Alias for backwards compatibility
pub const TOPIC_PROPOSALS: &str = SCHEDULER_PROPOSALS;

// === MESSAGE PREFIXES ===

/// Prefix used for handshake messages exchanged on the gossip topic.
pub const HANDSHAKE_PREFIX: &str = "beemesh-handshake";

/// Prefix used for lease-related operations
pub const LEASE_PREFIX: &str = "lease/";

// === TIMEOUTS AND TIMING ===

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
