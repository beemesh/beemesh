# Beemesh Technical Specification

> **Version:** 0.2.0  
> **Last Updated:** November 2025  
> **Status:** Implementation Reference

This document provides a comprehensive technical specification for the Beemesh distributed workload orchestration system, derived from the codebase and inline documentation.

---

## Table of Contents

1. [Overview](#1-overview)
2. [Architecture](#2-architecture)
3. [Machineplane](#3-machineplane)
4. [Workplane](#4-workplane)
5. [Wire Protocols](#5-wire-protocols)
6. [Security Model](#6-security-model)
7. [Data Structures](#7-data-structures)
8. [API Reference](#8-api-reference)
9. [Configuration](#9-configuration)
10. [Failure Handling](#10-failure-handling)

---

## 1. Overview

### 1.1 Purpose

Beemesh is a decentralized, scale-out workload orchestration fabric that eliminates centralized control planes. It consists of two primary components:

| Component | Purpose | CAP Trade-off |
|-----------|---------|---------------|
| **Machineplane** | Infrastructure layer: node discovery, scheduling, deployment | A/P (Availability + Partition Tolerance) |
| **Workplane** | Workload layer: service discovery, health monitoring, self-healing | C/P (Consistency + Partition Tolerance) |

### 1.2 Design Principles

1. **Statelessness**: Machineplane persists nothing; all state is derived from:
   - **Runtime state**: Podman provides the source of truth for running workloads
   - **Ephemeral tracking**: `tender_tracker` holds transient operation state (not persisted)
   - **K8s API**: All responses are reconstructed from Podman pod labels
2. **Separation of Concerns**: Machine and workload identities are cryptographically disjoint
3. **Consumer-Scoped Consistency**: Each workload manages its own consensus (Raft for stateful)
4. **Ephemeral Scheduling**: Tender→Bid→Award protocol without global state
5. **Fire-and-Forget Mutations**: CREATE/DELETE operations publish tenders and return immediately

### 1.3 Technology Stack

```toml
[workspace.dependencies]
libp2p = "0.56"        # Networking (QUIC, Kademlia, Gossipsub)
tokio = "1"            # Async runtime
serde = "1.0"          # Serialization
bincode = "1.3"        # Binary encoding for messages
reqwest = "0.11"       # HTTP client
```

---

## 2. Architecture

### 2.1 High-Level Components

```
┌────────────────────────────────────────────────────────────────────────────┐
│                              BEEMESH FABRIC                                │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │                         MACHINEPLANE                                │   │
│  │                                                                     │   │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐               │   │
│  │  │  Machine A   │  │  Machine B   │  │  Machine C   │               │   │
│  │  │  (Daemon)    │◄─┤  (Daemon)    │◄─┤  (Daemon)    │               │   │
│  │  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘               │   │
│  │         │                 │                 │                       │   │
│  │         └────────────┬────┴────────────┬────┘                       │   │
│  │                      │                 │                            │   │
│  │               ┌──────▼──────┐   ┌──────▼──────┐                     │   │
│  │               │    MDHT     │   │  Gossipsub  │                     │   │
│  │               │ (Kademlia)  │   │ (Pub/Sub)   │                     │   │
│  │               └─────────────┘   └─────────────┘                     │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                                                            │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │                          WORKPLANE                                  │   │
│  │                                                                     │   │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐               │   │
│  │  │  Workload A  │  │  Workload B  │  │  Workload C  │               │   │
│  │  │  (Agent)     │◄─┤  (Agent)     │◄─┤  (Agent)     │               │   │
│  │  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘               │   │
│  │         │                 │                 │                       │   │
│  │         └────────────┬────┴────────────┬────┘                       │   │
│  │                      │                 │                            │   │
│  │               ┌──────▼──────┐   ┌──────▼──────┐                     │   │
│  │               │    WDHT     │   │  RPC Streams│                     │   │
│  │               │ (Kademlia)  │   │ (Req/Resp)  │                     │   │
│  │               └─────────────┘   └─────────────┘                     │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                                                            │
└────────────────────────────────────────────────────────────────────────────┘
```

### 2.2 Module Structure

```
beemesh/
├── machineplane/
│   ├── src/
│   │   ├── lib.rs          # Entry point, CLI config, daemon startup
│   │   ├── main.rs         # Binary entry point
│   │   ├── scheduler.rs    # Tender/Bid/Award scheduling logic
│   │   ├── api/            # REST API (Kubernetes-compatible)
│   │   │   ├── mod.rs      # Router, tender/apply handlers, tender_tracker
│   │   │   └── kube.rs     # Stateless K8s API (derives state from Podman)
│   │   ├── messages/       # Wire protocol types
│   │   │   ├── types.rs    # SchedulerMessage, Tender, Bid, Award, Event
│   │   │   ├── signatures.rs # Ed25519 signing/verification
│   │   │   └── constants.rs  # Protocol constants
│   │   ├── network/        # libp2p networking
│   │   │   ├── mod.rs      # Swarm setup, event loop
│   │   │   ├── behaviour.rs # Gossipsub + Kademlia + Request/Response
│   │   │   ├── control.rs  # Control channel for external commands
│   │   │   └── dht_manager.rs # DHT operations and workload announcements
│   │   └── runtimes/       # Container runtime adapters
│   │       ├── mod.rs      # RuntimeEngine trait, registry
│   │       ├── podman.rs   # Podman implementation
│   │       └── podman_api.rs # Podman REST API client
│   └── tests/              # Integration tests
│
└── workplane/
    ├── src/
    │   ├── lib.rs          # Crate root, module exports
    │   ├── main.rs         # Binary entry point
    │   ├── config.rs       # Agent configuration
    │   ├── workload.rs     # Main orchestrator
    │   ├── network.rs      # libp2p networking (WDHT + RPC)
    │   ├── discovery.rs    # In-memory WDHT cache
    │   ├── raft.rs         # Leader election (stateful workloads)
    │   ├── selfheal.rs     # Replica management, health probing
    │   ├── rpc.rs          # RPC request/response types
    │   └── metrics.rs      # Internal metrics macros
    └── tests/
```

---

## 3. Machineplane

### 3.1 Overview

The Machineplane daemon (`machineplane`) runs on each node and provides:

- **Node Discovery**: Kademlia DHT for peer discovery
- **Decentralized Scheduling**: Tender→Bid→Award protocol via Gossipsub
- **Workload Deployment**: Container lifecycle via Podman
- **REST API**: Kubernetes-compatible endpoints

### 3.2 Startup Sequence

```rust
pub async fn start_machineplane(cli: DaemonConfig) -> Result<Vec<JoinHandle<()>>> {
    // 1. Initialize logging
    env_logger::Builder::from_env(...).try_init();

    // 2. Configure Podman runtime socket
    let podman_socket = resolve_and_configure_podman_socket(cli.podman_socket)?;

    // 3. Generate ephemeral Ed25519 keypair (transient identity)
    let keypair = Keypair::generate_ed25519();
    let local_peer_id = keypair.public().to_peer_id();

    // 4. Set up libp2p swarm (QUIC + Gossipsub + Kademlia + Request/Response)
    let (swarm, topic, peer_rx, peer_tx) = setup_libp2p_node(...)?;

    // 5. Dial bootstrap peers
    for addr in cli.bootstrap_peer { swarm.dial(addr)?; }

    // 6. Create control channel for REST→libp2p communication
    let (control_tx, control_rx) = mpsc::unbounded_channel();

    // 7. Spawn libp2p event loop
    tokio::spawn(start_libp2p_node(swarm, topic, peer_tx, control_rx));

    // 8. Initialize runtime registry
    scheduler::initialize_podman_manager().await?;

    // 9. Start REST API server
    let app = api::build_router(peer_rx, control_tx, public_bytes);
    axum::serve(listener, app).await;
}
```

### 3.3 Scheduler Module

The scheduler implements the **Tender→Bid→Award** protocol:

```
State Machine (per tender):

[Idle] ──TenderReceived──► [Bidding] ──AwardReceived──► [Deploying] ──DeployOK──► [Running]
              │                 │                              │
              └──NotEligible──► [Idle]                         └──DeployFail──► [Idle]
```

#### 3.3.1 Message Types

```rust
// All messages are bincode-encoded and signed with Ed25519
pub enum SchedulerMessage {
    Tender(Tender),      // Scheduling request
    Bid(Bid),            // Node's proposal
    Award(Award),        // Winner announcement
    Event(SchedulerEvent), // Deployment status
}

pub struct Tender {
    pub id: String,              // ULID, globally unique
    pub manifest_digest: String, // SHA256 of manifest (content stays with owner)
    pub requirements: ResourceRequirements,
    pub qos: QoSHints,
    pub timestamp: u64,          // ms since epoch
    pub nonce: u64,              // Replay protection
    pub signature: Vec<u8>,      // Ed25519 signature
}

pub struct Bid {
    pub tender_id: String,
    pub node_id: String,         // Machine Peer ID
    pub score: f64,              // Composite score (0.0-1.0)
    pub resource_fit_score: f64,
    pub network_locality_score: f64,
    pub timestamp: u64,
    pub nonce: u64,
    pub signature: Vec<u8>,
}

pub struct Award {
    pub tender_id: String,
    pub winners: Vec<String>,    // Ordered by score descending
    pub manifest_digest: String,
    pub timestamp: u64,
    pub nonce: u64,
    pub signature: Vec<u8>,
}
```

#### 3.3.2 Scheduling Flow

```rust
// Tender owner workflow
fn initialize_owned_tender(&self, tender_id: &str, ...) {
    // 1. Store in owned_tenders map
    // 2. Spawn async task for selection window
    tokio::spawn(async move {
        // Wait for bid collection window (~250ms ± jitter)
        sleep(Duration::from_millis(DEFAULT_SELECTION_WINDOW_MS)).await;

        // Select winners deterministically
        let winners = select_winners(&bid_context);

        // Publish Award to fabric
        outbound_tx.send(SchedulerCommand::Publish { topic, payload: award_bytes });

        // Send manifest to winners via secure stream
        for winner in winners {
            outbound_tx.send(SchedulerCommand::SendManifest { peer_id, payload });
        }

        // If local node won, deploy locally
        if winners.contains(&local_id) {
            outbound_tx.send(SchedulerCommand::DeployWorkload { ... });
        }
    });
}
```

#### 3.3.3 Replay Protection

```rust
const MAX_CLOCK_SKEW_MS: u64 = 30_000;    // ±30 seconds
const REPLAY_WINDOW_MS: u64 = 300_000;    // 5 minutes

// Replay filter: (tender_id, nonce) → first_seen_timestamp
fn mark_tender_seen(&self, tender_id: &str, nonce: u64) -> bool {
    let mut seen = self.seen_tenders.lock().unwrap();
    record_replay(&mut seen, (tender_id.to_string(), nonce))
}

fn is_timestamp_fresh(timestamp_ms: u64) -> bool {
    let now = now_ms();
    (now.abs_diff(timestamp_ms)) <= MAX_CLOCK_SKEW_MS
}
```

### 3.4 Network Module

#### 3.4.1 Behaviour Composition

```rust
#[derive(NetworkBehaviour)]
struct MyBehaviour {
    /// Pub/Sub for scheduler messages (Tender, Bid, Award, Event)
    gossipsub: gossipsub::Behaviour,

    /// DHT for peer discovery and workload record storage
    kademlia: kad::Behaviour<kad::store::MemoryStore>,

    /// Point-to-point for manifest transfer
    manifest_fetch_rr: request_response::Behaviour<ByteCodec>,
}
```

#### 3.4.2 Gossipsub Configuration

```rust
let gossipsub_config = gossipsub::ConfigBuilder::default()
    .heartbeat_interval(Duration::from_secs(10))
    .validation_mode(gossipsub::ValidationMode::Strict)
    .mesh_n_low(1)           // minimum peers in mesh
    .mesh_n(3)               // target mesh size
    .mesh_n_high(6)          // maximum peers in mesh
    .mesh_outbound_min(1)    // minimum outbound connections
    .message_id_fn(content_hash_message_id)  // Content-addressed dedup
    .allow_self_origin(true)
    .build()?;
```

#### 3.4.3 DHT Operations

```rust
// Periodic DHT refresh (configurable interval, default 60s)
async fn dht_refresh_tick(swarm: &mut Swarm<MyBehaviour>) {
    // Bootstrap DHT on first connection
    if !dht_bootstrapped {
        swarm.behaviour_mut().kademlia.bootstrap()?;
        dht_bootstrapped = true;
    }

    // Random walk for peer discovery
    let random_peer_id = PeerId::random();
    swarm.behaviour_mut().kademlia.get_closest_peers(random_peer_id);
}

// Store workload placement in DHT
fn publish_workload_to_dht(swarm: &mut Swarm<MyBehaviour>, workload_id: &str) {
    let record_key = kad::RecordKey::new(&format!("workload:{}", workload_id));
    let record = kad::Record {
        key: record_key,
        value: workload_id.as_bytes().to_vec(),
        publisher: None,
        expires: None,
    };
    swarm.behaviour_mut().kademlia.put_record(record, kad::Quorum::One)?;
}
```

### 3.5 Runtime Adapter

```rust
/// Runtime engine trait for container lifecycle management
#[async_trait]
pub trait RuntimeEngine: Send + Sync {
    fn name(&self) -> &str;
    async fn check_available(&self) -> bool;
    async fn deploy_workload(&self, manifest_id: &str, content: &[u8], config: &DeploymentConfig) -> Result<WorkloadInfo>;
    async fn remove_workload(&self, workload_id: &str) -> Result<()>;
    async fn list_workloads(&self) -> Result<Vec<WorkloadInfo>>;
    async fn get_workload_logs(&self, workload_id: &str, tail: Option<usize>) -> Result<String>;
}

/// Podman implementation
impl RuntimeEngine for PodmanEngine {
    async fn deploy_workload(&self, manifest_id: &str, content: &[u8], ...) -> Result<WorkloadInfo> {
        // 1. Write manifest to temp file
        // 2. Run: podman play kube <manifest> --replace
        // 3. Parse output for container IDs
        // 4. Return WorkloadInfo with status
    }
}
```

#### Pod Labels Injected During Deployment

When deploying a manifest, Beemesh injects the following labels for tracking and K8s API compatibility:

| Label | Description |
|-------|-------------|
| `beemesh.workload_id` | Unique workload identifier (manifest_id + entropy) |
| `beemesh.manifest_id` | Manifest identifier for grouping replicas |
| `beemesh.local_peer_id` | Peer ID of the node running this pod |
| `io.kubernetes.pod.namespace` | K8s namespace (extracted from manifest, defaults to "default") |
| `app.kubernetes.io/name` | App name (extracted from manifest metadata.name) |

### 3.6 Kubernetes API (Stateless)

The K8s API module (`kube.rs`) provides kubectl-compatible endpoints while maintaining Beemesh's
stateless architecture. **No K8s resource state is stored in memory**—all responses are derived
from the Podman runtime.

#### Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                    kubectl / K8s Client                          │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                    Machineplane REST API                         │
│              /apis/apps/v1/namespaces/{ns}/deployments           │
└─────────────────────────────────────────────────────────────────┘
                              │
              ┌───────────────┴───────────────┐
              │                               │
         GET/LIST                       POST/DELETE
              │                               │
              ▼                               ▼
┌─────────────────────────┐     ┌─────────────────────────┐
│   Podman Runtime        │     │   Gossipsub Tender      │
│   (local pods only)     │     │   (fire-and-forget)     │
└─────────────────────────┘     └─────────────────────────┘
```

#### Workload Aggregation

Deployments and StatefulSets are reconstructed by aggregating pods with matching labels:

```rust
// Pods with these labels are grouped into workloads:
// - app.kubernetes.io/name → workload name
// - io.kubernetes.pod.namespace → namespace filtering
// - beemesh.io/workload-kind → "Deployment" or "StatefulSet"

fn aggregate_workloads(pods: &[PodListEntry], namespace: &str, kind: Option<&str>) -> Vec<WorkloadInfo> {
    // Group pods by (namespace, kind, app_name)
    // Count replicas and ready_replicas
    // Return K8s-formatted responses
}
```

#### Limitations

| Limitation | Reason |
|------------|--------|
| Local node only | No mesh-wide aggregation; each node shows only its own pods |
| No UPDATE/PATCH | Only CREATE and DELETE supported (republish for updates) |
| Best-effort status | Status reflects Podman state, may have propagation delay |

---

## 4. Workplane

### 4.1 Overview

The Workplane agent (`workplane`) runs as a sidecar in each workload pod and provides:

- **Service Discovery**: WDHT (Workload DHT) for replica discovery
- **Health Monitoring**: HTTP liveness/readiness probes
- **Self-Healing**: Automatic scale-up/down via machineplane API
- **Leader Election**: Raft consensus for stateful workloads
- **RPC Streams**: Inter-replica communication

### 4.2 Agent Lifecycle

```rust
impl Workload {
    pub fn new(config: Config) -> Result<Self> {
        // 1. Generate workload keypair (from config or random)
        // 2. Initialize Network with libp2p swarm
        // 3. Create SelfHealer for replica management
        // 4. For stateful workloads, create RaftManager
    }

    pub fn start(&mut self) {
        // 1. Start network heartbeat (WDHT record publication)
        self.network.start();

        // 2. Start self-healer (health probes + reconciliation)
        self.self_healer.start();

        // 3. Register RPC handlers
        rpc::register_stream_handler(self.create_handler());

        // 4. For stateful workloads, start Raft tick loop
        if self.config.is_stateful() {
            self.start_raft_loop();
        }
    }
}
```

### 4.3 Service Discovery (WDHT)

#### 4.3.1 ServiceRecord

```rust
/// A service record published to the WDHT
#[derive(Serialize, Deserialize)]
pub struct ServiceRecord {
    pub workload_id: String,     // "{namespace}/{workload_name}"
    pub namespace: String,
    pub workload_name: String,
    pub peer_id: String,         // libp2p PeerId
    pub pod_name: Option<String>,
    pub ordinal: Option<u32>,    // For StatefulSets
    pub addrs: Vec<String>,      // libp2p multiaddrs
    pub caps: Map<String, Value>, // Capabilities (leader, ordinal, etc.)
    pub version: u64,            // Monotonic, for conflict resolution
    pub ts: i64,                 // Unix ms timestamp
    pub nonce: String,           // UUID for uniqueness
    pub ready: bool,             // Readiness probe result
    pub healthy: bool,           // Liveness probe result
}
```

#### 4.3.2 Conflict Resolution

```rust
/// Order: version > timestamp > peer_id (lexicographic)
fn should_replace(existing: &ServiceRecord, incoming: &ServiceRecord) -> bool {
    if incoming.version > existing.version { return true; }
    if incoming.version < existing.version { return false; }
    if incoming.ts > existing.ts { return true; }
    if incoming.ts < existing.ts { return false; }
    incoming.peer_id > existing.peer_id  // Deterministic tiebreaker
}
```

#### 4.3.3 TTL-Based Expiration

```rust
const MAX_CLOCK_SKEW_MS: i64 = 30_000;  // ±30 seconds freshness check

pub fn put(record: ServiceRecord, ttl: Duration) -> bool {
    // 1. Reject stale/future timestamps
    if !is_fresh(&record) { return false; }

    // 2. Purge expired entries
    purge_expired(namespace_map);

    // 3. Conflict resolution
    if should_replace(&existing, &record) {
        // Insert with expiration time
        entries.insert(peer_id, RecordEntry {
            record,
            expires_at: Instant::now() + ttl,
        });
    }
}
```

### 4.4 Raft Consensus (Stateful Workloads)

```rust
pub struct RaftManager {
    node_id: String,
    workload_id: String,
    term: u64,
    state: RaftState,        // Follower | Candidate | Leader
    voted_for: Option<String>,
    votes_received: HashSet<String>,
    peers: HashSet<String>,
    leader_id: Option<String>,
    last_heartbeat: Instant,
    election_timeout: Duration,  // Random 150-300ms
}

impl RaftManager {
    /// Periodic tick - check timeouts, send heartbeats
    pub fn tick(&mut self) -> Vec<RaftAction> {
        match self.state {
            RaftState::Leader => {
                // Send heartbeat every 50ms
                if elapsed >= Duration::from_millis(50) {
                    actions.push(RaftAction::SendHeartbeat(...));
                }
            }
            RaftState::Follower | RaftState::Candidate => {
                // Check election timeout (150-300ms random)
                if elapsed >= self.election_timeout {
                    self.start_election(&mut actions);
                }
            }
        }
    }

    /// Handle incoming heartbeat from leader
    pub fn handle_heartbeat(&mut self, heartbeat: Heartbeat) -> Vec<RaftAction> {
        if heartbeat.term >= self.term {
            self.term = heartbeat.term;
            self.state = RaftState::Follower;
            self.leader_id = Some(heartbeat.leader_id);
            self.last_heartbeat = Instant::now();
        }
    }
}
```

### 4.5 Self-Healing

```rust
pub struct SelfHealer {
    network: Network,
    config: Config,
    http: Client,  // For HTTP health probes
}

impl SelfHealer {
    pub fn start(&mut self) {
        tokio::spawn(async move {
            let mut ticker = interval(min(health_interval, replica_interval));
            loop {
                // Health probe cycle
                if now.duration_since(last_health) >= health_interval {
                    update_local_health(&network, &client, &cfg).await?;
                }

                // Replica reconciliation cycle
                if now.duration_since(last_replica) >= replica_interval {
                    reconcile_replicas(&network, &client, &cfg).await?;
                }
            }
        });
    }
}

async fn reconcile_replicas(network: &Network, client: &Client, cfg: &Config) -> Result<()> {
    // 1. Query WDHT for all workload replicas
    let records = network.find_service_peers();

    // 2. Filter healthy replicas (via RPC healthz calls)
    let healthy_replicas = verify_health(network, &records).await;

    // 3. Scale up if needed
    if healthy_replicas.len() < cfg.replicas {
        let missing = cfg.replicas - healthy_replicas.len();
        for _ in 0..missing {
            publish_clone_tender(cfg, client, ordinal).await?;
        }
    }

    // 4. Scale down if needed (prioritize unhealthy)
    if healthy_replicas.len() > cfg.replicas {
        let excess = healthy_replicas.len() - cfg.replicas;
        for record in excess_replicas.iter().take(excess) {
            remove_replica(cfg, client, record).await?;
        }
    }
}
```

### 4.6 RPC Streams

```rust
/// RPC request between workload replicas
#[derive(Serialize, Deserialize)]
pub struct RPCRequest {
    pub method: String,       // "healthz", "leader", custom
    pub body: Map<String, Value>,
    pub leader_only: bool,    // Proxy to leader if true
}

#[derive(Serialize, Deserialize)]
pub struct RPCResponse {
    pub ok: bool,
    pub error: Option<String>,
    pub body: Map<String, Value>,
}

// Built-in method handlers
async fn handle_rpc(peer: &ServiceRecord, request: RPCRequest) -> RPCResponse {
    match request.method.as_str() {
        "healthz" => RPCResponse { ok: true, ..Default::default() },
        "leader" => {
            let leader_id = raft_manager.leader_id();
            RPCResponse {
                ok: true,
                body: json!({ "leader_id": leader_id }),
            }
        }
        _ if request.leader_only && !raft_manager.is_leader() => {
            // Proxy to leader
            network.send_request(&leader_id, request).await
        }
        _ => custom_handler(peer, request).await,
    }
}
```

---

## 5. Wire Protocols

### 5.1 Transport Layer

| Protocol | Port | Description |
|----------|------|-------------|
| QUIC/UDP | Dynamic | libp2p transport (encrypted, multiplexed) |
| HTTP | 3000 (default) | REST API (Kubernetes-compatible) |

### 5.2 libp2p Protocols

| Protocol ID | Purpose | Encoding |
|-------------|---------|----------|
| `/meshsub/1.1.0` | Gossipsub messaging | Bincode |
| `/ipfs/kad/1.0.0` | Kademlia DHT | Protobuf |
| `/beemesh/manifest-fetch/1.0.0` | Manifest transfer | Raw bytes |
| `/workplane/rpc/1.0.0` | Inter-replica RPC | JSON |

### 5.3 Message Encoding

```rust
// All scheduler messages use bincode
fn serialize<T: Serialize>(value: &T) -> Vec<u8> {
    bincode::serialize(value).expect("serialize")
}

fn deserialize<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> bincode::Result<T> {
    bincode::deserialize(bytes)
}

// Workplane RPC uses JSON over request/response streams
async fn write_request(&mut self, io: &mut T, req: RPCRequest) -> io::Result<()> {
    let data = serde_json::to_vec(&req)?;
    io.write_all(&data).await
}
```

### 5.4 Gossipsub Topic

All scheduler messages flow through a single topic:

```rust
const BEEMESH_FABRIC: &str = "beemesh-fabric";

// Message types on this topic:
enum SchedulerMessage {
    Tender(Tender),        // New workload request
    Bid(Bid),              // Node's scheduling proposal
    Award(Award),          // Winner announcement
    Event(SchedulerEvent), // Deployed/Failed/Preempted/Cancelled
}
```

---

## 6. Security Model

### 6.1 Identity Separation

| Entity | Identity Type | Key Storage | Scope |
|--------|--------------|-------------|-------|
| Machine | Machine Peer ID | Ephemeral (per-run) | Infrastructure only |
| Workload | Workload Peer ID | Per-replica | Workload only |

**Critical**: Machine and workload identities are cryptographically disjoint. Machine credentials cannot access workload streams, and vice versa.

### 6.2 Message Authentication

```rust
// Ed25519 signing for all scheduler messages
pub fn sign_tender(tender: &mut Tender, keypair: &Keypair) -> Result<()> {
    let msg = tender_signing_payload(tender);
    tender.signature = keypair.sign(&msg)?;
    Ok(())
}

pub fn verify_sign_tender(tender: &Tender, public_key: &PublicKey) -> bool {
    let msg = tender_signing_payload(tender);
    public_key.verify(&msg, &tender.signature).is_ok()
}

fn tender_signing_payload(tender: &Tender) -> Vec<u8> {
    // Deterministic serialization of signable fields
    bincode::serialize(&(
        &tender.id,
        &tender.manifest_digest,
        tender.timestamp,
        tender.nonce,
    )).unwrap()
}
```

### 6.3 Replay Protection

```rust
// Two-tier protection:
// 1. Timestamp freshness (±30s window)
// 2. (tender_id, nonce) tuple deduplication (5-minute window)

fn is_message_valid(tender: &Tender) -> bool {
    // Check timestamp freshness
    if !is_timestamp_fresh(tender.timestamp) {
        return false;
    }

    // Check replay filter
    if !mark_tender_seen(&tender.id, tender.nonce) {
        return false;  // Already seen
    }

    // Verify signature
    verify_sign_tender(tender, &public_key)
}
```

### 6.4 Transport Encryption

All libp2p connections use **Noise protocol** or **TLS** for:
- Mutual authentication (both peers verify each other)
- Encryption (all data encrypted in transit)
- Perfect forward secrecy

---

## 7. Data Structures

### 7.1 Tender

```rust
pub struct Tender {
    pub id: String,                   // ULID (unique, sortable)
    pub manifest_digest: String,      // SHA256 hash (base64)
    pub requirements: ResourceRequirements,
    pub qos: QoSHints,
    pub timestamp: u64,               // ms since epoch
    pub nonce: u64,                   // Random for replay protection
    pub signature: Vec<u8>,           // Ed25519 signature
}

pub struct ResourceRequirements {
    pub cpu_millicores: Option<u32>,
    pub memory_bytes: Option<u64>,
    pub gpu_count: Option<u32>,
}

pub struct QoSHints {
    pub priority: i32,                // Higher = more important
    pub preemptible: bool,            // Can be evicted
}
```

### 7.2 ApplyRequest (REST API)

```rust
pub struct ApplyRequest {
    pub replicas: u32,
    pub operation_id: String,         // Idempotency key
    pub manifest_json: String,        // Full manifest content
    pub origin_peer: String,          // Requesting node
    pub manifest_id: String,          // User-facing identifier
    pub signature: Vec<u8>,
}
```

### 7.3 ManifestTransfer (Award Winner)

```rust
pub struct ManifestTransfer {
    pub tender_id: String,
    pub manifest_id: String,
    pub manifest_digest: String,
    pub manifest_json: String,        // Full manifest content
    pub owner_peer_id: String,
    pub owner_pubkey: Vec<u8>,
    pub replicas: u32,
}
```

### 7.4 ServiceRecord (WDHT)

```rust
pub struct ServiceRecord {
    pub workload_id: String,          // "{namespace}/{workload_name}"
    pub namespace: String,
    pub workload_name: String,
    pub peer_id: String,
    pub pod_name: Option<String>,
    pub ordinal: Option<u32>,
    pub addrs: Vec<String>,
    pub caps: Map<String, Value>,
    pub version: u64,
    pub ts: i64,
    pub nonce: String,
    pub ready: bool,
    pub healthy: bool,
}
```

---

## 8. API Reference

### 8.1 Machineplane REST API

#### Core Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/version` | GET | API version info |
| `/apis` | GET | API group list |
| `/health` | GET | Health check |
| `/api/v1/pubkey` | GET | Get node public key |
| `/nodes` | GET | List cluster nodes |

#### Kubernetes-Compatible API (Stateless)

```mermaid
classDiagram

    %% Core Concepts
    class Cluster {
        +API Server
        +etcd
        +Controller Manager
        +Scheduler
    }

    class Namespace {
        +Logical isolation
    }

    class Node {
        +Kubelet
        +Kube-proxy
        +Pods
    }

    class Pod {
        +Containers
        +Volumes
        +Networking
    }

    class Container {
        +Image
        +Environment
        +Ports
    }

    %% Workloads
    class Deployment {
        +ReplicaSets
        +Scaling
        +Rolling Updates
    }

    class ReplicaSet {
        +Ensures Pod count
    }

    class StatefulSet {
        +Stable network identity
        +Persistent volumes
    }

    class DaemonSet {
        +One Pod per Node
    }

    class Job {
        +Run to completion
    }

    class CronJob {
        +Scheduled jobs
    }

    %% Services & Networking
    class Service {
        +Stable DNS/ClusterIP
        +Load balancing
    }

    class Ingress {
        +HTTP routing
    }

    %% Config & Storage
    class ConfigMap {
        +Key-value configuration
    }

    class Secret {
        +Sensitive data
    }

    class Volume {
        +Ephemeral or Persistent
    }

    class PersistentVolume {
        +Cluster-wide storage
    }

    class PersistentVolumeClaim {
        +Pod storage request
    }

    %% Relationships

    %% Cluster & Namespaces
    Cluster --> Namespace : manages
    Cluster --> Node : manages
    Cluster --> PersistentVolume : provides

    %% Namespaced resources
    Namespace --> Pod : contains
    Namespace --> Service : contains
    Namespace --> Deployment : contains
    Namespace --> ReplicaSet : contains
    Namespace --> StatefulSet : contains
    Namespace --> DaemonSet : contains
    Namespace --> Job : contains
    Namespace --> CronJob : contains
    Namespace --> Ingress : contains
    Namespace --> ConfigMap : contains
    Namespace --> Secret : contains
    Namespace --> PersistentVolumeClaim : contains

    %% Workload relationships
    Node --> Pod : runs
    Deployment --> ReplicaSet : controls
    ReplicaSet --> Pod : manages
    StatefulSet --> Pod : manages
    DaemonSet --> Pod : schedules
    Job --> Pod : runs
    CronJob --> Job : schedules

    %% Networking
    Ingress --> Service : routes to
    Service --> Pod : targets

    %% Pod internals & config
    Pod --> Container : contains
    Pod --> ConfigMap : consumes
    Pod --> Secret : consumes
    Pod --> Volume : mounts

    %% Storage chain
    Volume --> PersistentVolumeClaim : references
    PersistentVolumeClaim --> PersistentVolume : binds
```


The K8s API is **stateless by design**: all read operations derive state from the local Podman runtime,
and all write operations publish tenders via Gossipsub (fire-and-forget).

| Endpoint | Method | Description | Source |
|----------|--------|-------------|--------|
| `/api/v1/namespaces/{ns}/pods` | GET | List pods | Podman runtime |
| `/api/v1/namespaces/{ns}/pods/{name}` | GET | Get pod | Podman runtime |
| `/api/v1/namespaces/{ns}/pods/{name}/log` | GET | Get pod logs | Podman runtime |
| `/apis/apps/v1/namespaces/{ns}/deployments` | GET | List deployments | Podman pods (aggregated) |
| `/apis/apps/v1/namespaces/{ns}/deployments/{name}` | GET | Get deployment | Podman pods (aggregated) |
| `/apis/apps/v1/namespaces/{ns}/deployments` | POST | Create deployment | Gossipsub tender |
| `/apis/apps/v1/namespaces/{ns}/deployments/{name}` | DELETE | Delete deployment | Gossipsub tender |
| `/apis/apps/v1/namespaces/{ns}/statefulsets` | GET | List statefulsets | Podman pods (aggregated) |
| `/apis/apps/v1/namespaces/{ns}/statefulsets/{name}` | GET | Get statefulset | Podman pods (aggregated) |
| `/apis/apps/v1/namespaces/{ns}/statefulsets` | POST | Create statefulset | Gossipsub tender |
| `/apis/apps/v1/namespaces/{ns}/statefulsets/{name}` | DELETE | Delete statefulset | Gossipsub tender |
| `/apis/apps/v1/namespaces/{ns}/replicasets` | GET | List replicasets | Derived from deployments |
| `/apis/apps/v1/namespaces/{ns}/replicasets/{name}` | GET | Get replicaset | Derived from deployments |

**Note**: K8s API responses show only workloads running on the **local node**. Mesh-wide aggregation
requires querying multiple nodes or using DHT-based discovery.

#### Tender Management

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/tenders` | POST | Create tender |
| `/tenders/{tender_id}` | GET | Get tender status |
| `/tenders/{tender_id}` | DELETE | Cancel tender |
| `/tenders/{tender_id}/candidates` | POST | Get scheduling candidates |
| `/tenders/{tender_id}/manifest_id` | GET | Get manifest ID for tender |
| `/apply_direct/{peer_id}` | POST | Apply manifest to specific peer |

#### Debug Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/debug/peers` | GET | List connected peers |
| `/debug/tenders` | GET | List tracked tenders |
| `/debug/local_peer_id` | GET | Get local peer ID |
| `/debug/dht/peers` | GET | List DHT peers |
| `/debug/dht/active_announces` | GET | List active DHT announcements |
| `/debug/decrypted_manifests` | GET | List decrypted manifests |
| `/debug/workloads_by_peer/{peer_id}` | GET | List workloads for peer |

### 8.2 Workplane RPC Methods

| Method | Description | Leader Only |
|--------|-------------|-------------|
| `healthz` | Health check | No |
| `leader` | Query current Raft leader | No |
| Custom | Application-defined | Configurable |

---

## 9. Configuration

### 9.1 Machineplane CLI

```rust
pub struct Cli {
    /// Host address for REST API
    #[arg(long, default_value = "0.0.0.0")]
    pub rest_api_host: String,

    /// Port for REST API
    #[arg(long, default_value = "3000")]
    pub rest_api_port: u16,

    /// Custom node name
    #[arg(long)]
    pub node_name: Option<String>,

    /// Podman socket URL
    #[arg(long, env = "CONTAINER_HOST")]
    pub podman_socket: Option<String>,

    /// Bootstrap peer addresses
    #[arg(long)]
    pub bootstrap_peer: Vec<String>,

    /// Port for libp2p QUIC transport
    #[arg(long, default_value = "0")]
    pub libp2p_quic_port: u16,

    /// DHT refresh interval (seconds)
    #[arg(long, default_value = "60")]
    pub dht_refresh_interval_secs: u64,
}
```

### 9.2 Workplane Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `BEE_NAMESPACE` | Kubernetes namespace | `default` |
| `BEE_WORKLOAD_NAME` | Workload name | Required |
| `BEE_POD_NAME` | Pod name | Required |
| `BEE_WORKLOAD_KIND` | `Deployment` or `StatefulSet` | `Deployment` |
| `BEE_REPLICAS` | Desired replica count | `1` |
| `BEE_LIVENESS_URL` | HTTP liveness probe URL | None |
| `BEE_READINESS_URL` | HTTP readiness probe URL | None |
| `BEE_BEEMESH_API` | Machineplane API URL | `http://127.0.0.1:3000` |
| `BEE_PRIVATE_KEY` | libp2p private key (base64) | Generated |
| `BEE_LISTEN_ADDRS` | libp2p listen addresses | Auto |
| `BEE_BOOTSTRAP_PEERS` | Bootstrap peer multiaddrs | None |
| `BEE_HEALTH_PROBE_INTERVAL` | Health check interval | `10s` |
| `BEE_HEALTH_PROBE_TIMEOUT` | Health check timeout | `5s` |
| `BEE_REPLICA_CHECK_INTERVAL` | Reconciliation interval | `30s` |
| `BEE_HEARTBEAT_INTERVAL` | WDHT heartbeat interval | `30s` |
| `BEE_DHT_TTL` | WDHT record TTL | `60s` |

---

## 10. Failure Handling

### 10.1 Network Partition

| Scenario | Machineplane Behavior | Workplane Behavior |
|----------|----------------------|-------------------|
| Partial partition | Nodes continue bidding locally | WDHT records expire; replicas re-resolve |
| Full partition | No global coordination; local deploys continue | Minority partition refuses writes (Raft) |
| Partition heals | Conflicting awards tolerated; freshest wins | Raft leader re-elected; records refresh |

### 10.2 Node Failure

| Scenario | Detection | Recovery |
|----------|-----------|----------|
| Machine crash | MDHT record expires | Workplane requests new replica |
| Workplane crash | WDHT record expires | SelfHealer on other replicas requests replacement |
| Runtime crash | Container runtime monitoring | Daemon restarts container or requests replacement |

### 10.3 Deployment Failure

```rust
// Machineplane emits Event{Failed} on deployment failure
match process_manifest_deployment(...).await {
    Ok(workload_id) => {
        scheduler.publish_event(&tender_id, EventType::Deployed, &msg);
    }
    Err(e) => {
        scheduler.publish_event(&tender_id, EventType::Failed, &msg);
        // Other nodes may re-evaluate tender after backoff
    }
}
```

### 10.4 Consistency Guarantees

| Workload Type | Write Consistency | Read Consistency |
|---------------|------------------|------------------|
| Stateless | None (all replicas equal) | Eventual (WDHT TTL) |
| Stateful | Leader-only (Raft quorum) | Read-your-writes (via leader) |

### 10.5 Bootstrap Paradox

> **Critical**: If all machines AND all workloads are lost simultaneously, Beemesh cannot self-recover. An external bootstrap mechanism (installers, image registries, GitOps) is required.

---

## References

- [Beemesh README](README.md) - Project overview
- [Machineplane Spec](machineplane/machineplane-spec.md) - Detailed machineplane specification
- [Workplane Spec](workplane/workplane-spec.md) - Detailed workplane specification
- [libp2p Documentation](https://docs.libp2p.io/) - Networking primitives
- [Raft Consensus Algorithm](https://raft.github.io/) - Leader election reference
