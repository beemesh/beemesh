// libp2p-related constants for topics, message prefixes, protocol names, etc.

/// Ident topic used for gossipsub in the beemesh cluster.
pub const BEEMESH_CLUSTER: &str = "beemesh-cluster";

/// Prefix used for handshake messages exchanged on the gossip topic.
pub const HANDSHAKE_PREFIX: &str = "beemesh-handshake";

/// Prefix used when querying peers for free capacity (gossipsub message topic payload prefix).
pub const FREE_CAPACITY_PREFIX: &str = "beemesh-free-capacity";

/// Prefix used for replies to free-capacity queries.
pub const FREE_CAPACITY_REPLY_PREFIX: &str = "beemesh-free-capacity-reply";

/// Timeout, in seconds, to wait for free-capacity responses from peers.
pub const FREE_CAPACITY_TIMEOUT_SECS: u64 = 3;

/// Timeout, in seconds, to wait for request-response RPCs (ApplyRequest/ApplyResponse)
pub const REQUEST_RESPONSE_TIMEOUT_SECS: u64 = 3;

/// JSON field name used for replica count in manifests (top-level `replicas`).
pub const REPLICAS_FIELD: &str = "replicas";

/// JSON path field used for replica count in manifests under `spec.replicas`.
pub const SPEC_REPLICAS_FIELD: &str = "spec";

/// Version byte used in the compact binary envelope for capreq/capreply messages.
pub const BINARY_ENVELOPE_VERSION: u8 = 1;
