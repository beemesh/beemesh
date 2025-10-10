//! Consolidated libp2p-related constants for topics, message prefixes, protocol names, etc.
//!
//! This module contains all protocol constants to eliminate duplication across crates.

/// Ident topic used for gossipsub in the beemesh cluster.
pub const BEEMESH_CLUSTER: &str = "beemesh-cluster";

// === GOSSIPSUB TOPICS ===

/// Protocol name for request-response RPCs (ApplyRequest/ApplyResponse).
pub const SCHEDULER_TASKS_TOPIC: &str = "scheduler-tasks";
/// Alias for backwards compatibility
pub const TOPIC_TASKS: &str = SCHEDULER_TASKS_TOPIC;

/// Topic used for scheduler events
pub const SCHEDULER_EVENTS_TOPIC: &str = "scheduler-events";
/// Alias for backwards compatibility
pub const TOPIC_EVENTS: &str = SCHEDULER_EVENTS_TOPIC;

/// Topic used for scheduler proposals / capacity requests
pub const SCHEDULER_PROPOSALS_TOPIC: &str = "scheduler-proposal";
/// Alias for backwards compatibility
pub const TOPIC_PROPOSALS: &str = "scheduler-proposals";

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

/// Timeout, in seconds, to wait for free-capacity responses from peers.
pub const FREE_CAPACITY_TIMEOUT_SECS: u64 = 3;

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

// === PROTOCOL VERSIONING ===

/// Version byte used in the compact binary envelope for capreq/capreply messages.
pub const BINARY_ENVELOPE_VERSION: u8 = 1;
