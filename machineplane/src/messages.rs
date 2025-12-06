//! Message type definitions for BeeMesh network communication
//!
//! All messages use Bincode for serialization, replacing the previous FlatBuffers implementation.
//! These types are used for peer-to-peer communication via libp2p request-response protocols.

use serde::{Deserialize, Serialize};

// ============================================================================
// Core Request/Response Messages
// ============================================================================

/// Request to apply/deploy a Kubernetes manifest
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApplyRequest {
    pub replicas: u32,
    pub operation_id: String,
    pub manifest_json: String,
    pub origin_peer: String,
    pub signature: Vec<u8>,
}

/// Response to an apply request
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApplyResponse {
    pub ok: bool,
    pub operation_id: String,
    pub message: String,
    pub signature: Vec<u8>,
}

/// Request to delete a deployed manifest by resource coordinates
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[derive(Default)]
pub struct DeleteRequest {
    /// Kubernetes namespace (e.g., "default")
    pub namespace: String,
    /// Kubernetes resource kind (e.g., "Pod", "Deployment")
    pub kind: String,
    /// Kubernetes resource name (from metadata.name)
    pub name: String,
    pub operation_id: String,
    pub origin_peer: String,
    /// Force deletion even if verification fails
    pub force: bool,
}

/// Response to a delete request
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeleteResponse {
    pub ok: bool,
    pub operation_id: String,
    pub message: String,
    /// Resource coordinates that were deleted (namespace/kind/name)
    pub resource: String,
    /// List of pod IDs that were removed
    pub removed_pods: Vec<String>,
}

// ============================================================================
// Health
// ============================================================================

/// Health check message
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Health {
    pub ok: bool,
    pub status: String,
}

// ============================================================================
// Scheduler Messages
// ============================================================================

/// Tender definition for scheduling (published via Gossipsub)
///
/// The tender is the only scheduler message broadcast to all nodes.
/// The tender owner's peer ID is available from the gossipsub message source field
/// (guaranteed present when using MessageAuthenticity::Signed).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[derive(Default)]
pub struct Tender {
    pub id: String,
    /// Reference digest for the tender (currently uses tender_id)
    pub manifest_digest: String,
    pub qos_preemptible: bool,
    pub timestamp: u64,
    pub nonce: u64,
    pub signature: Vec<u8>,
}

/// Bidding message for tender assignment (sent via Request-Response to tender owner)
///
/// Bids are sent directly to the tender owner, not broadcast via gossipsub.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Bid {
    pub tender_id: String,
    pub node_id: String,
    pub score: f64,
    pub resource_fit_score: f64,
    pub network_locality_score: f64,
    pub timestamp: u64,
    pub nonce: u64,
    /// Signature of the bid content
    pub signature: Vec<u8>,
}

/// Response to a bid submission
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BidResponse {
    pub tender_id: String,
    pub accepted: bool,
    pub message: String,
}

/// Scheduler event types
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[repr(u8)]
pub enum EventType {
    Deployed = 0,
    Failed = 1,
    Preempted = 2,
    Cancelled = 3,
}

/// Scheduler event notification (sent via Request-Response to tender owner)
///
/// Events are sent directly to the tender owner after deployment attempt.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SchedulerEvent {
    pub tender_id: String,
    pub node_id: String,
    pub event_type: EventType,
    pub reason: String,
    pub timestamp: u64,
    pub nonce: u64,
    pub signature: Vec<u8>,
}

/// Response to a scheduler event
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventResponse {
    pub tender_id: String,
    pub acknowledged: bool,
}

/// Disposal message for workload deletion (published via Gossipsub)
///
/// Fire-and-forget message that instructs all nodes to delete pods matching
/// the specified resource coordinates. Uses namespace/kind/name to identify
/// workloads instead of content-based hashes, enabling updates and renames.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Disposal {
    /// Kubernetes namespace (e.g., "default")
    pub namespace: String,
    /// Kubernetes resource kind (e.g., "Pod", "Deployment")
    pub kind: String,
    /// Kubernetes resource name (from metadata.name)
    pub name: String,
    /// Timestamp (ms since UNIX epoch)
    pub timestamp: u64,
    /// Random nonce for uniqueness/replay protection
    pub nonce: u64,
    /// Signature over (namespace || kind || name || timestamp || nonce)
    pub signature: Vec<u8>,
}

/// Award message with manifest (sent via Request-Response to each winner)
///
/// Combines the award notification with manifest delivery in a single message.
/// This replaces the separate Award broadcast + ManifestTransfer pattern.
///
/// The manifest digest (for integrity verification) is computed implicitly
/// from `manifest_json` by the receiver using SHA256. This eliminates redundancy
/// and ensures the digest always matches the content.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AwardWithManifest {
    pub tender_id: String,
    pub manifest_json: String,
    pub owner_peer_id: String,
    pub owner_pubkey: Vec<u8>,
    pub replicas: u32,
    pub timestamp: u64,
    pub nonce: u64,
    pub signature: Vec<u8>,
}

/// Response to an award message
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AwardResponse {
    pub tender_id: String,
    pub accepted: bool,
    pub message: String,
}

// ============================================================================
// Tender Response Messages
// ============================================================================

/// Response to tender creation request
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TenderCreateResponse {
    pub ok: bool,
    pub tender_id: String,
    pub manifest_ref: String,
    pub selection_window_ms: u64,
    pub message: String,
}

/// Response to tender status query
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TenderStatusResponse {
    pub tender_id: String,
    pub state: String,
    pub assigned_peers: Vec<String>,
}

// ============================================================================
// Node and Candidate Messages
// ============================================================================

/// Candidate node with public key for encrypted key share distribution
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CandidateNode {
    pub peer_id: String,
    /// Base64-encoded KEM public key for encrypting key shares
    pub public_key: String,
}

/// Response with list of candidate nodes
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CandidatesResponse {
    pub ok: bool,
    /// List of candidate nodes with their public keys
    pub candidates: Vec<CandidateNode>,
}

/// Response with list of peer nodes
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodesResponse {
    pub peers: Vec<String>,
}

// ============================================================================
// Applied Manifest (DHT Storage)
// ============================================================================

/// Signature scheme used to sign the manifest record
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[repr(u8)]
pub enum SignatureScheme {
    None = 0,
    Ed25519 = 1,
    RsaPss = 2,
}

/// Operation performed with this manifest record
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[repr(u8)]
pub enum OperationType {
    Apply = 0,
    Update = 1,
    Delete = 2,
}

/// Simple key/value pair for labeling or small indexes
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KeyValue {
    pub key: String,
    pub value: String,
}

/// AppliedManifest is the object stored in the DHT representing an applied
/// Kubernetes manifest. It is designed to be verifiable (signature + pubkey),
/// multi-tenant friendly (tenant + owner_pubkey) and retrievable via a stable
/// identifier (`id` / `content_hash`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppliedManifest {
    /// A canonical identifier for this record. Recommended: hex-encoded SHA-256
    /// over canonicalized payload (SHA256(owner_pubkey + ':' + operation_id + ':' + canonical_manifest))
    pub id: String,

    /// Operation id supplied by the originator (correlates with ApplyRequest)
    pub operation_id: String,

    /// libp2p peer id of the origin that applied the manifest
    pub origin_peer: String,

    /// Raw public key bytes of the owner (for signature verification)
    pub owner_pubkey: Vec<u8>,

    /// Algorithm/scheme used for the signature
    pub signature_scheme: SignatureScheme,

    /// Binary signature over the canonicalized data
    pub signature: Vec<u8>,

    /// The manifest itself as JSON string
    pub manifest_json: String,

    /// Optional convenience field for indexing
    pub manifest_kind: String,

    /// Optional labels for indexing and quick queries
    pub labels: Vec<KeyValue>,

    /// Unix timestamp in milliseconds when manifest was applied/signed
    pub timestamp: u64,

    /// What kind of operation this record represents
    pub operation: OperationType,

    /// Optional TTL (in seconds) to help nodes expire records from DHT
    pub ttl_secs: u32,

    /// Hex-encoded SHA-256 of the canonicalized manifest_json (content id)
    pub content_hash: String,
}

// ============================================================================
// Default Implementations
// ============================================================================

impl Default for ApplyRequest {
    fn default() -> Self {
        Self {
            replicas: 1,
            operation_id: String::new(),
            manifest_json: String::new(),
            origin_peer: String::new(),
            signature: Vec::new(),
        }
    }
}



impl Default for AwardWithManifest {
    fn default() -> Self {
        Self {
            tender_id: String::new(),
            manifest_json: String::new(),
            owner_peer_id: String::new(),
            owner_pubkey: Vec::new(),
            replicas: 1,
            timestamp: 0,
            nonce: 0,
            signature: Vec::new(),
        }
    }
}

impl Default for AppliedManifest {
    fn default() -> Self {
        Self {
            id: String::new(),
            operation_id: String::new(),
            origin_peer: String::new(),
            owner_pubkey: Vec::new(),
            signature_scheme: SignatureScheme::None,
            signature: Vec::new(),
            manifest_json: String::new(),
            manifest_kind: String::new(),
            labels: Vec::new(),
            timestamp: 0,
            operation: OperationType::Apply,
            ttl_secs: 0,
            content_hash: String::new(),
        }
    }
}

// ============================================================================
// Safe Bincode Deserialization
// ============================================================================

/// Maximum size for bincode deserialization (16 MiB).
///
/// This limit prevents OOM attacks from malicious peers sending payloads
/// with inflated size prefixes. 16 MiB is sufficient for any legitimate
/// message while protecting against memory exhaustion.
pub const MAX_BINCODE_SIZE: u64 = 16 * 1024 * 1024;

/// Safely deserialize a bincode payload with size limits.
///
/// Uses bincode's `Options` API to enforce a maximum deserialization size,
/// preventing OOM attacks from malicious payloads that claim to have
/// extremely large sizes in their length prefixes.
///
/// # Arguments
///
/// * `bytes` - The serialized bincode data
///
/// # Returns
///
/// The deserialized value, or an error if deserialization fails or
/// the payload would exceed the size limit.
///
/// # Security
///
/// This function MUST be used for all network-received bincode data
/// instead of `bincode::deserialize()` to prevent OOM attacks.
pub fn deserialize_safe<'a, T>(bytes: &'a [u8]) -> Result<T, bincode::Error>
where
    T: serde::Deserialize<'a>,
{
    use bincode::Options;
    
    bincode::DefaultOptions::new()
        .with_limit(MAX_BINCODE_SIZE)
        .with_fixint_encoding()
        .allow_trailing_bytes()
        .deserialize(bytes)
}
