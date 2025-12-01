# **Beemesh Workplane — Normative Specification (v0.3)**

> **Scope:** This document specifies the **Workplane** of Beemesh: per-workload service discovery, workload identity, secure workload-to-workload connectivity, and self-healing for both stateful and stateless workloads. It uses RFC 2119 terminology and provides executable-style Gherkin scenarios.
>
> **Semantics:** The Workplane is **Consistency/Partition-Tolerant (C/P)**. It scopes consistency to each workload's own trust domain (for example, a Raft quorum). The Machineplane is A/P and schedules via a **tender → bid → award** process; the Workplane **MUST** enforce correctness for replicas that are awarded.
>
> **Profile:** Rust implementation using `libp2p` (Kademlia DHT + Request/Response RPC + QUIC transport).

---

## **1. Overview & Goals**

The Workplane is a **per-workload control and data plane** that runs inside each Pod or workload instance (as a sidecar or infra container). It:

* Manages a **unique Workload Peer ID per replica**.
* Uses **libp2p** for:

  * A **Workload DHT (WDHT)** used for service discovery and workload metadata.
  * **Mutually authenticated, encrypted workload-to-workload streams**.
* Publishes **signed, TTL’d ServiceRecords** in WDHT as **hints** for discovery.
* Implements **self-healing** for:

  * **Stateful workloads** by enforcing:

    * **Leader-only** and **majority-only** write semantics based on workload Raft consensus.
    * **Minority partition write refusal**, even for previously elected leaders.
  * **Stateless workloads** based on **replica counts**, requesting more replicas from the Machineplane when needed.

The Workplane **MUST** assume the Machineplane can:

* Start and restart replicas according to **tender → bid → award** outcomes.
* Lose or reassign machines without warning.
* Maintain the **manifest** for the requesting workload (the Workplane **MUST NOT** own workload manifests).

The Workplane **MUST** keep workloads **self-contained** and resilient to rescheduling, while delegating replica instantiation to the Machineplane.

### **1.1 Non-Goals**

The Workplane **MUST NOT**:

* Perform infrastructure-level node discovery, bidding, or scheduling (Machineplane responsibility).
* Interact directly with container runtimes (pull images, start containers, configure cgroups).
* Maintain global fabric state, multi-workload transactions, or cross-workload consensus.
* Store or manage workload manifests (these live in and are interpreted by the Machineplane).

---

## **2. Glossary (Selected)**

* **Workload**: Logical application unit (for example, `payments-api`, `orders-db`) consisting of one or more replicas.
* **Replica**: A single running instance (pod/container set) of a workload.
* **Workload Peer ID**: Unique cryptographic identity for a replica, managed by the Workplane Agent and used with libp2p.
* **Service ID**: Stable identifier for a logical service exposed by a workload.
* **Workload DHT (WDHT)**: libp2p Kademlia-based distributed key-value store for **workload-level** service discovery and metadata (separate from the Machine DHT).
* **Workload ID**: Unique identifier in format `{namespace}/{kind}/{name}` (e.g., `default/Deployment/nginx`).
* **Role**: Logical role of a replica, for example `leader | follower | read-only | standby | stateless`.
* **Ordinal**: For StatefulSet workloads, the replica index extracted from pod name (e.g., `redis-2` → ordinal `2`).
* **Desired Replica Count**: Target number of replicas, interpreted as an **"at least"** value at runtime (`desired_replicas_at_least`).

---

## **3. Architecture**

### **3.1 Components**

* **Workplane Agent** (sidecar or in-process library):

  * Generates or loads a **unique Workload Peer ID per replica** and manages associated keys.
  * Uses **libp2p** to participate in the **WDHT** and maintain connections.
  * Registers and refreshes **ServiceRecord** entries in WDHT with TTL-based expiration.
  * Observes local health via HTTP probes (liveness/readiness URLs).
  * For stateful workloads: participates in **Raft leader election** (simplified, election-only implementation).
  * Implements **self-healing** via:
    * Observed **replica counts** for stateless workloads.
    * **Raft-based status** and ordinal management for stateful workloads.
  * Triggers **Machineplane interactions** via REST API when replicas need to be added or removed.

* **Agent Subsystems**:
  * **Network** (`network.rs`): libp2p Kademlia DHT + Request/Response RPC over QUIC transport.
  * **RaftManager** (`raft.rs`): Simplified Raft for leader election (no log replication).
  * **SelfHealer** (`selfheal.rs`): Health monitoring and replica count reconciliation.
  * **Discovery** (`discovery.rs`): In-memory WDHT cache with TTL expiration and conflict resolution.

* **Workload DHT (WDHT)**:

  * Implemented using **libp2p Kademlia** (`/ipfs/kad/1.0.0` protocol).
  * Stores **ServiceRecord** entries (service endpoints, roles, health, version, etc.).
  * Stores **transient** health and role information with TTL (default: 15 seconds).
  * **MUST NOT** store machine-level data (that belongs to the Machine DHT).

* **RPC Protocol**:

  * **libp2p Request/Response** protocol at `/workplane/rpc/1.0.0`.
  * JSON-encoded `RPCRequest` and `RPCResponse` messages.
  * Built-in methods: `healthz`, `raft.heartbeat`, `raft.vote`.
  * Supports `leader_only` flag for automatic follower→leader proxying.

* **Secure Workload Streams**:

  * **QUIC transport** (UDP-based, encrypted, multiplexed).
  * **Mutually authenticated** using libp2p peer identity.
  * Isolated from machine-to-machine streams and credentials.

### **3.2 Diagram**

```mermaid
flowchart LR
  subgraph Node
    MD[Machine Daemon]
    RT[(Runtime - Podman)]
    subgraph Pod
      WA[Workplane Agent]
      APP[Workload Container(s)]
    end
  end

  WDHT[(Workload DHT - libp2p)]
  NET{{Secure Workload Streams (libp2p)}}

  MD --> RT
  RT -.hosts.-> Pod
  WA -->|libp2p DHT register/lookup| WDHT
  APP <-->|mTLS / encrypted| NET
  NET <-->|mTLS / encrypted| APP
```

---

## **4. Identities & Trust**

* Workload identities **MUST** be disjoint from machine identities:

  * Workload Peer IDs **MUST NOT** reuse Machine Peer ID keys.
  * Machineplane topics/streams **MUST NOT** accept Workplane credentials, and vice versa.

* Each replica **MUST** have a unique **Workload Peer ID**, backed by a public/private key pair.

* All workload-to-workload streams **MUST** be mutually authenticated and encrypted using libp2p-secured channels:

  * Handshakes **MUST** verify both peers’ Workload Peer IDs.
  * Unauthenticated or mismatched-ID streams **MUST** be rejected.

---

## **5. Protocols & Data Models**

### **5.1 Identifiers**

* **Workload ID**: Unique identifier for a logical workload, format: `{namespace}/{kind}/{name}` (for example, `default/Deployment/nginx`). This aligns with Kubernetes resource coordinates and the Machineplane's resource identification scheme.
* **Workload Peer ID**: Unique per replica (libp2p `PeerId`, base58-encoded).
* **Ordinal**: For StatefulSet workloads, extracted from pod name suffix (e.g., `redis-2` → `2`).

### **5.2 Workload DHT Records**

The WDHT **MUST** support at least the following logical record type.

#### **5.2.1 ServiceRecord**

```rust
pub struct ServiceRecord {
    pub workload_id: String,      // "{namespace}/{kind}/{workload_name}"
    pub namespace: String,         // Kubernetes-style namespace
    pub workload_name: String,     // Workload name (e.g., "nginx")
    pub peer_id: String,           // libp2p peer ID of this replica
    pub pod_name: Option<String>,  // Pod name for stateful workloads
    pub ordinal: Option<u32>,      // StatefulSet ordinal (0, 1, 2, ...)
    pub addrs: Vec<String>,        // libp2p multiaddrs
    pub caps: Map<String, Value>,  // Capability metadata
    pub version: u64,              // Record version (conflict resolution)
    pub ts: i64,                   // Unix timestamp in milliseconds
    pub nonce: String,             // Random nonce (replay protection)
    pub ready: bool,               // Readiness probe result
    pub healthy: bool,             // Liveness probe result
}
```

**Interpretation:**

* ServiceRecords are **ephemeral, TTL'd hints** stored in the WDHT cache.
* Consumers **MUST**:
  * Treat records as **hints**, not ground truth.
  * Ignore expired records (default TTL: 15 seconds, configurable via `BEE_DHT_TTL`).
  * Ignore unknown fields for forward compatibility.
* The `caps` map **MAY** contain:
  * `namespace`, `workload`, `pod`, `ordinal`
  * `leader`: Boolean indicating leader status (stateful workloads)
  * `leader_epoch`: Raft term/epoch for consistency

### **5.3 Conflict Resolution**

When inserting records into the WDHT cache, conflicts are resolved by:

1. **Higher `version`** wins
2. If versions equal, **higher `ts`** (timestamp) wins
3. If both equal, **lexicographically higher `peer_id`** wins (deterministic tiebreaker)

Records with timestamps outside ±30 seconds of the local clock are rejected (`MAX_CLOCK_SKEW_MS = 30_000`).

### **5.4 RPC Protocol**

Inter-replica communication uses JSON-encoded messages over libp2p (`/workplane/rpc/1.0.0`):

| Type | Fields |
|------|--------|
| `RPCRequest` | `method: String`, `body: Map`, `leader_only: bool` |
| `RPCResponse` | `ok: bool`, `error: Option<String>`, `body: Map` |

**Built-in RPC Methods:**

| Method | Description | Leader Only |
|--------|-------------|-------------|
| `healthz` | Health check (returns `{ok: true, remote, pod}`) | No |
| `raft.heartbeat` | Raft leader heartbeat to followers | No |
| `raft.vote` | Raft vote request during election | No |

### **5.5 Cryptography**

* All WDHT records **MUST** be keyed by `workload_id`.
* libp2p peer identity provides mutual authentication via QUIC TLS.
* Workplane Agents **MUST** attach `ts` and `nonce` to records for replay mitigation.

---

## **6. Consistency & Role Semantics**

The Workplane relies on **workload-owned Raft consensus** for stateful workloads and enforces policies derived from it.

### **6.1 Stateless Workloads**

For workloads marked `stateful = false` (or `role = stateless`):

* Any **healthy** replica **MAY** serve both reads and writes.
* Clients **SHOULD** use WDHT to discover all healthy replicas and **MAY** load-balance across them.
* Self-healing for stateless workloads is based **only on replica counts** relative to `desired_replicas_at_least`.

### **6.2 Stateful Workloads**

For workloads with `workload_kind = "StatefulSet"` or `"StatefulWorkload"`:

* The Workplane Agent implements **simplified Raft** for leader election (no log replication).
* The Agent **MUST** enforce **leader-only** semantics for requests with `leader_only: true`.

#### **6.2.1 Raft Implementation**

The `RaftManager` handles leader election with these parameters:

| Parameter | Value | Description |
|-----------|-------|-------------|
| `ELECTION_TIMEOUT_MIN_MS` | 150 | Minimum election timeout |
| `ELECTION_TIMEOUT_MAX_MS` | 300 | Maximum election timeout (randomized) |
| `HEARTBEAT_INTERVAL_MS` | 50 | Leader heartbeat interval |

**States:**
* `Follower`: Receiving heartbeats from leader
* `Candidate`: Requesting votes for election
* `Leader`: Sending heartbeats, handling leader-only requests

**Roles (advertised in WDHT):**
* `Leader`: Current Raft leader
* `Follower`: Connected to leader
* `Detached`: No leader known (election in progress)

#### **6.2.2 Leader Election Flow**

1. Node starts as `Follower` with randomized election timeout
2. If timeout expires without heartbeat, node becomes `Candidate`
3. `Candidate` increments term, votes for self, sends `VoteRequest` to all peers
4. If majority votes received, becomes `Leader`
5. `Leader` sends heartbeats every 50ms to maintain leadership

#### **6.2.3 Leader-Only Request Handling**

When a follower receives a request with `leader_only: true`:

1. Check local Raft state for current leader
2. If leader known: proxy request via `Network::send_request`
3. If leader unknown: return error `"not leader"` or `"leader unknown"`

A replica that cannot contact a majority of peers (minority partition) **MUST NOT** become leader.

### **6.3 Replica Targets and Machineplane Awards**

* Workplane Agents **MUST** interpret `desired_replicas_at_least` as a **minimum** healthy replica count.

* When the number of **healthy** replicas (stateless or stateful) observed via:

  * Local health checks, and
  * WDHT ServiceRecords

  falls below `desired_replicas_at_least`, an Agent **MAY**:

  * Call into a **local Machineplane API** to request additional replicas.

* The Machineplane:

  * **MUST** use the **manifest it already possesses** for the requesting workload when creating new replicas.
  * Retains full control over scheduling, placement, and restart policy.

* Leader/write gating rules in **6.2** apply **regardless** of how or why replicas were awarded or overprovisioned.

---

## **7. Self-Healing & Replica Management**

The `SelfHealer` component handles health monitoring and replica reconciliation within the workload's trust domain, delegating replica creation to the Machineplane.

### **7.1. Health Monitoring**

```rust
pub struct SelfHealer {
    pub liveness_url: Option<String>,   // from BEE_LIVENESS_URL
    pub readiness_url: Option<String>,  // from BEE_READINESS_URL
    pub health_probe_interval: Duration, // default: 10s
    // ...internal state...
}
```

* Agents probe health endpoints periodically (default every 10 seconds):

  * **Liveness:** HTTP GET to `liveness_url` — failure indicates restart needed.
  * **Readiness:** HTTP GET to `readiness_url` — failure indicates workload not ready for traffic.

* The `healthy` and `ready` fields in `ServiceRecord` reflect probe results.

### **7.2. Reconciliation Phases**

The `SelfHealer::check_once()` method performs:

1. **Local Health Probe:** Query local liveness/readiness endpoints.
2. **Peer Verification:** RPC `healthz` calls to all peers in WDHT to verify reachability.
3. **Disposal Detection:** For stateful workloads, query Machineplane via `GET /v1/disposal/{ns}/{kind}/{name}` to detect if the workload is being removed.
4. **Replica Count Reconciliation:**
   * Count healthy replicas (local + verified peers).
   * Scale up: Emit **Tender** to Machineplane if `healthy_count < desired_replicas`.
   * Scale down: Request `POST /v1/remove_replica` to Machineplane if `healthy_count > desired_replicas`.

### **7.3. Tender Emission**

When scaling up is needed, the leader (or any agent for stateless workloads) emits:

```rust
POST /v1/publish_tender
{
    "namespace": "default",
    "kind": "Deployment",
    "name": "nginx",
    "current_replicas": 2,
    "desired_replicas": 3,
    "constraints": { /* optional placement hints */ }
}
```

The Machineplane receives tenders and decides where to schedule new replicas.

### **7.4. Retry & Backoff**

* API calls use **exponential backoff**: 200ms initial delay, max 5 seconds, up to 5 retries.
* On repeated failures, agents emit structured log events with reason codes.

### **7.5. Consistency Rules**

* For **stateless workloads**: Any healthy agent may emit tenders.
* For **stateful workloads**: Only the Raft leader may emit tenders or request replica removal (enforced via `leader_only` RPC gating).
* Agents **MUST NOT** relax consistency rules to compensate for low replica counts.

---

## **8. Service Discovery & Routing**

* Agents **MUST** publish a `ServiceRecord` for each service a workload exposes.
* Agents **MUST** refresh records before `ttl_ms` expires and **MUST** withdraw or let expire records when replicas become unhealthy.

Clients (Workplane-aware callers) **MUST**:

* Resolve `service_id` → set of `ServiceRecord`s from WDHT.
* Filter out expired or unhealthy records.
* For **stateful services**, prefer or restrict writes to `role = leader` records.
* Be prepared for hints to be stale and **SHOULD** handle failures with retries and re-resolution.

Implementations **SHOULD** support:

* Client-side load balancing across stateless replicas.
* Routing policies such as locality or version-based routing (for example, prefer `version = canary`).

### **8.1 Leader Traffic Routing for Stateful Workloads**

For stateful workloads, the Workplane provides **automatic leader routing** so that clients can send requests to any replica and have leader-bound requests transparently forwarded.

#### **8.1.1 Request Routing Model**

```
┌────────────────────────────────────────────────────────────────┐
│                    Stateful Workload Cluster                    │
├────────────────────────────────────────────────────────────────┤
│                                                                 │
│   Client Request                                                │
│        │                                                        │
│        ▼                                                        │
│   ┌─────────┐        ┌─────────┐        ┌─────────┐           │
│   │ Pod-0   │◄──────►│ Pod-1   │◄──────►│ Pod-2   │           │
│   │(Leader) │  Raft  │(Follower)  Raft  │(Follower)│           │
│   └────┬────┘        └────┬────┘        └────┬────┘           │
│        │                  │                  │                 │
│        └──────────────────┴──────────────────┘                 │
│                    Heartbeats (50ms)                           │
│                                                                 │
│   Request hits follower with leader_only=true:                 │
│   ┌──────────────────────────────────────────────────────┐    │
│   │  1. Follower receives request                         │    │
│   │  2. Checks leader_only flag                           │    │
│   │  3. Looks up current leader from Raft state           │    │
│   │  4. Proxies request to leader via libp2p              │    │
│   │  5. Returns leader's response to client               │    │
│   └──────────────────────────────────────────────────────┘    │
│                                                                 │
└────────────────────────────────────────────────────────────────┘
```

#### **8.1.2 Request Types**

Requests to stateful workloads **MUST** include a `leader_only` flag:

| `leader_only` | Behavior |
|---------------|----------|
| `true` | Request **MUST** be handled by the Raft leader. Followers **MUST** proxy to leader or reject. |
| `false` | Request **MAY** be handled by any healthy replica (suitable for reads). |

#### **8.1.3 Follower Proxy Behavior**

When a follower replica receives a request with `leader_only = true`:

1. The follower **MUST** check its local Raft state for the current leader.
2. If a leader is known:
   * The follower **MUST** proxy the request to the leader via libp2p secure streams.
   * The follower **MUST** return the leader's response to the original client.
   * The follower **SHOULD** increment a `follower_proxy_count` metric.
3. If no leader is known (election in progress or partitioned):
   * The follower **MUST** reject the request with an appropriate error (e.g., `"not leader"` or `"leader unknown"`).
   * The follower **SHOULD** increment a `follower_rejections` metric.
4. If the leader is unreachable:
   * The follower **MUST** return an error (e.g., `"leader unreachable"`).
   * The follower **SHOULD NOT** retry indefinitely; timeout and fail fast.

#### **8.1.4 Leader Behavior**

When the leader replica receives a request (directly or proxied):

1. The leader **MUST** verify it still holds leadership (has quorum, term is current).
2. If still leader: process the request and return the response.
3. If no longer leader (detected via Raft state): reject with error and **SHOULD** indicate the new leader if known.

#### **8.1.5 Benefits of This Model**

| Benefit | Description |
|---------|-------------|
| **Client simplicity** | Clients can send requests to any replica without tracking leader state. |
| **No external coordinator** | Unlike Kubernetes (which depends on etcd), Beemesh embeds Raft directly in the workload cluster. |
| **Automatic failover** | When leader fails, Raft elects a new leader; subsequent requests route correctly. |
| **Partition safety** | Minority partitions refuse writes (per §6.2), preventing split-brain. |

#### **8.1.6 Comparison with Other Systems**

| System | Leader Discovery | Write Routing |
|--------|------------------|---------------|
| **Kubernetes + etcd** | etcd Raft (centralized) | Apps must implement their own |
| **MySQL + Orchestrator** | External orchestrator | Proxy or app-level routing |
| **Redis Cluster** | Gossip + MOVED redirects | Client follows redirects |
| **MongoDB Replica Set** | Built-in Raft | Driver tracks primary |
| **Beemesh Workplane** | Embedded Raft per workload | Automatic follower→leader proxy |

Beemesh eliminates the need for a central consensus cluster (like etcd) by embedding Raft directly in each stateful workload's replica set, enabling fully decentralized leader election and traffic routing.

### **8.2 Load-Balanced Traffic Routing for Stateless Workloads**

For stateless workloads, the Workplane provides **distributed load balancing** where any healthy replica can handle any request.

#### **8.2.1 Request Routing Model**

```
┌────────────────────────────────────────────────────────────────┐
│                   Stateless Workload Cluster                    │
├────────────────────────────────────────────────────────────────┤
│                                                                 │
│   Client Request                                                │
│        │                                                        │
│        ▼  (any healthy replica)                                 │
│   ┌─────────┐        ┌─────────┐        ┌─────────┐           │
│   │ Pod-0   │        │ Pod-1   │        │ Pod-2   │           │
│   │(healthy)│        │(healthy)│        │(healthy)│           │
│   └─────────┘        └─────────┘        └─────────┘           │
│        ▲                  ▲                  ▲                 │
│        │                  │                  │                 │
│        └──────────────────┴──────────────────┘                 │
│              All publish ServiceRecords to WDHT                │
│              with role=stateless, health=healthy               │
│                                                                 │
│   Request handling:                                            │
│   ┌──────────────────────────────────────────────────────┐    │
│   │  1. Client resolves service_id from WDHT              │    │
│   │  2. Client receives list of healthy replicas          │    │
│   │  3. Client selects one (round-robin, random, etc.)    │    │
│   │  4. Selected replica handles request locally          │    │
│   │  5. No proxying needed - all replicas are equivalent  │    │
│   └──────────────────────────────────────────────────────┘    │
│                                                                 │
└────────────────────────────────────────────────────────────────┘
```

#### **8.2.2 Key Differences from Stateful**

| Aspect | Stateless | Stateful |
|--------|-----------|----------|
| **Consensus** | None required | Raft leader election |
| **Request routing** | Any healthy replica | Leader for writes, any for reads |
| **Proxying** | Never needed | Follower→Leader proxy |
| **ServiceRecord role** | `role = stateless` | `role = leader \| follower` |
| **Failure impact** | Lose capacity only | May lose write availability |

#### **8.2.3 Load Balancing Strategies**

Clients resolving stateless services from WDHT **SHOULD** implement one of:

| Strategy | Description | Use Case |
|----------|-------------|----------|
| **Round-robin** | Rotate through healthy replicas sequentially | Default, even distribution |
| **Random** | Select randomly from healthy set | Simple, avoids coordination |
| **Least-connections** | Prefer replicas with fewer active requests | Connection-heavy workloads |
| **Locality-aware** | Prefer replicas in same zone/region | Latency-sensitive workloads |
| **Version-based** | Filter by `version` field (e.g., canary) | Progressive rollouts |

#### **8.2.4 Replica Behavior**

Each stateless replica **MUST**:

1. Publish a `ServiceRecord` with `role = stateless` and current `health` status.
2. Handle all incoming requests locally (no proxying).
3. Update `health` field when readiness changes (e.g., `healthy → degraded → unhealthy`).
4. Withdraw or expire its `ServiceRecord` when shutting down or unhealthy.

Each stateless replica **MUST NOT**:

1. Assume any coordination with other replicas for request handling.
2. Depend on request ordering across replicas.
3. Store state that must be consistent across replicas (use external storage or convert to stateful).

#### **8.2.5 Self-Healing for Stateless**

Unlike stateful workloads (which use Raft quorum), stateless self-healing is purely count-based:

```
┌─────────────────────────────────────────────────────────────┐
│                Stateless Self-Healing Flow                   │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│   desired_replicas_at_least = 3                             │
│                                                              │
│   ┌─────────┐   ┌─────────┐   ┌─────────┐                  │
│   │ Pod-0   │   │ Pod-1   │   │ Pod-2   │  ✓ 3 healthy     │
│   │(healthy)│   │(healthy)│   │(healthy)│                  │
│   └─────────┘   └─────────┘   └─────────┘                  │
│                                                              │
│        │ Pod-1 crashes                                       │
│        ▼                                                     │
│                                                              │
│   ┌─────────┐   ┌─────────┐   ┌─────────┐                  │
│   │ Pod-0   │   │ Pod-1   │   │ Pod-2   │  ✗ 2 healthy     │
│   │(healthy)│   │  (dead) │   │(healthy)│                  │
│   └─────────┘   └─────────┘   └─────────┘                  │
│                                                              │
│        │ Agent detects shortfall via WDHT                   │
│        │ Agent requests replica from Machineplane           │
│        ▼                                                     │
│                                                              │
│   ┌─────────┐   ┌─────────┐   ┌─────────┐   ┌─────────┐   │
│   │ Pod-0   │   │ Pod-1   │   │ Pod-2   │   │ Pod-3   │    │
│   │(healthy)│   │  (dead) │   │(healthy)│   │(healthy)│    │
│   └─────────┘   └─────────┘   └─────────┘   └─────────┘   │
│                                              ▲              │
│                                              │ new replica  │
│                                    ✓ 3 healthy again        │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

#### **8.2.6 Benefits of Stateless Model**

| Benefit | Description |
|---------|-------------|
| **Horizontal scalability** | Add replicas for more capacity; no consensus overhead. |
| **Simple failover** | Failed replica removed from WDHT; clients route to others. |
| **No single point of failure** | All replicas equivalent; no leader bottleneck. |
| **Fast startup** | New replicas serve traffic immediately; no election needed. |

---

## **9. Failure Handling**

| Failure Scenario                        | Required Workplane Behavior                                                                                                                                                                                 |
| --------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Replica crash**                       | Local Agent **MUST** mark the replica unhealthy and/or remove its `ServiceRecord` from WDHT. It **MAY** request restart or additional replicas via Machineplane APIs, which use the existing manifest.      |
| **Network partition (within workload)** | Agents on both sides **MUST** enforce statefulness rules: minority-side replicas **MUST NOT** accept writes; leaders **MUST** revalidate quorum with Raft before serving writes.                            |
| **Stale leader record in WDHT**         | Clients **MUST** tolerate write failures and **SHOULD** retry, resolving WDHT again. Agents **MUST** refresh role information; expired or invalid records **MUST** be ignored.                              |
| **Surplus replicas**                    | When healthy replicas exceed `desired_replicas_at_least`, Agents **SHOULD** prefer draining or idling surplus replicas where policy dictates, but **MUST** maintain consistency gating for stateful writes. |
| **Manifest mismatch / invalid config**  | Agents **MUST** fail fast and log clear errors when Machineplane-provided configuration is invalid or incompatible; they **MUST NOT** silently relax security or consistency constraints.                   |

---

## **10. Observability**

Workplane logs **SHOULD** include:

* Workload Peer ID and Replica ID.
* Service ID and role.
* Correlation IDs for incoming/outgoing requests (where applicable).
* Manifest version (from Machineplane) and relevant consistency flags.

Logs **MUST** be emitted to `stdout` / `stderr` so that runtime log aggregation can collect them without implementation-specific sinks.

---

## **11. Security Considerations**

* Workplane Agents **MUST** treat any workload-to-workload connection without proper mutual authentication as **untrusted** and **MUST** reject or sandbox it.
* Workload credentials (keys, tokens) **MUST NOT** be shared with the Machineplane.
* Agents **SHOULD** rate-limit expensive WDHT operations and connection attempts per peer to reduce abuse.
* Sensitive metadata (for example, tenant IDs, internal topology hints) **SHOULD** be encrypted or omitted from public WDHT records where possible.

---

## **12. Interop & Compatibility**

* Wire formats for Workplane messages and WDHT records **MUST** be versioned.
* Unknown fields **MUST** be ignored by older nodes.
* Different Workplane versions **SHOULD** coexist within the same workload cluster as long as they share a minimal compatible schema.

---

## **13. Configuration**

Agent configuration is provided via environment variables prefixed with `BEE_`:

| Variable | Field | Default | Description |
|----------|-------|---------|-------------|
| `BEE_NAMESPACE` | `namespace` | `"default"` | Kubernetes-style namespace for workload isolation |
| `BEE_WORKLOAD_NAME` | `workload_name` | (required) | Name of the workload (e.g., "nginx", "redis") |
| `BEE_POD_NAME` | `pod_name` | (empty) | Pod name; used to extract ordinal for stateful workloads |
| `BEE_REPLICAS` | `replicas` | `1` | Desired replica count for self-healing |
| `BEE_WORKLOAD_KIND` | `workload_kind` | `"Deployment"` | Workload kind (see below) |
| `BEE_LIVENESS_URL` | `liveness_url` | (empty) | Full URL for liveness probe |
| `BEE_READINESS_URL` | `readiness_url` | (empty) | Full URL for readiness probe |
| `BEE_BOOTSTRAP_PEERS` | `bootstrap_peer_strings` | (empty) | Comma-separated bootstrap multiaddrs |
| `BEE_BEEMESH_API` | `beemesh_api` | `http://localhost:8080` | Machineplane API base URL |
| `BEE_REPLICA_CHECK_INTERVAL` | `replica_check_interval` | `30s` | Interval between replica count checks |
| `BEE_DHT_TTL` | `dht_ttl` | `15s` | TTL for WDHT service records |
| `BEE_HEARTBEAT_INTERVAL` | `heartbeat_interval` | `5s` | Interval for publishing WDHT heartbeats |
| `BEE_HEALTH_PROBE_INTERVAL` | `health_probe_interval` | `10s` | Interval for health probe checks |
| `BEE_HEALTH_PROBE_TIMEOUT` | `health_probe_timeout` | `5s` | HTTP timeout for health probes |
| `BEE_ALLOW_CROSS_NAMESPACE` | `allow_cross_namespace` | `false` | Allow discovering other namespaces |
| `BEE_ALLOWED_WORKLOADS` | `allowed_workloads` | (all) | Allowlist of workload IDs |
| `BEE_DENIED_WORKLOADS` | `denied_workloads` | (empty) | Denylist of workload IDs |
| `BEE_LISTEN_ADDRS` | `listen_addrs` | (empty) | libp2p listen addresses |

### **13.1 Workload Kind Mapping**

| `BEE_WORKLOAD_KIND` | Behavior | Task Kind |
|---------------------|----------|-----------|
| `Deployment` | Stateless (no Raft) | `StatelessWorkload` |
| `StatelessWorkload` | Stateless (no Raft) | `StatelessWorkload` |
| `StatefulSet` | Stateful (Raft leader election) | `StatefulWorkload` |
| `StatefulWorkload` | Stateful (Raft leader election) | `StatefulWorkload` |
| `DaemonSet` | Stateless, one per node | `DaemonSet` |
| Other | Custom | `CustomWorkload` |

### **13.2 Derived Values**

* **`workload_id()`:** `{namespace}/{workload_kind}/{workload_name}` (e.g., `default/StatefulSet/redis`)
* **`ordinal()`:** Parsed from pod name suffix (e.g., `redis-2` → `2`)
* **`is_stateful()`:** `true` for `StatefulSet` or `StatefulWorkload` kinds

---

## **14. Gherkin Scenarios (Executable-Style)**

### **14.1 Feature: Workload Identity & Secure Streams**

```gherkin
Scenario: Workload rejects unauthenticated peer
  Given a running Workplane Agent with a valid Workload Peer ID
  And a remote peer connects without presenting a valid Workload certificate
  When the Agent evaluates the libp2p connection
  Then it MUST reject the connection
  And it MUST NOT exchange application data on this stream
```

```gherkin
Scenario: Distinct machine and workload identities
  Given a node with a Machine Peer ID
  And a workload replica with a Workload Peer ID
  When the workload establishes a workload-to-workload stream over libp2p
  Then the stream MUST authenticate using the Workload Peer ID
  And it MUST NOT reuse the Machine Peer ID credentials
```

### **14.2 Feature: Service Discovery & Roles**

```gherkin
Scenario: Leader and follower registration in WDHT
  Given a stateful workload with one leader replica L and one follower replica F
  And the workload uses Raft for consensus
  When L starts and becomes leader
  Then it MUST publish a signed ServiceRecord with role=leader in the WDHT

  When F starts and joins the cluster
  Then it MUST publish a signed ServiceRecord with role=follower in the WDHT
```

```gherkin
Scenario: Client only writes to leader
  Given a client that resolves ServiceRecords for a stateful service
  And the WDHT contains one leader and one follower
  When the client sends a write request
  Then it MUST route the write only to the leader endpoint
  And if the follower receives a write request
  Then the follower MUST reject the request or redirect it to the leader
```

```gherkin
Scenario: Follower proxies leader-only request to leader
  Given a stateful workload with replicas L (leader) and F (follower)
  And both replicas are healthy and connected via libp2p
  When a client sends a request with leader_only=true to follower F
  Then F MUST forward the request to leader L via libp2p secure stream
  And F MUST return L's response to the client
  And F SHOULD NOT process the request locally
```

```gherkin
Scenario: Follower rejects leader-only request when leader unknown
  Given a stateful workload with replica F in follower state
  And F does not know the current leader (election in progress)
  When a client sends a request with leader_only=true to F
  Then F MUST reject the request with error "not leader" or "leader unknown"
  And F MUST NOT process the request locally
```

### **14.3 Feature: Consistency under Partition & Duplicates**

```gherkin
Scenario: Minority partition refuses writes
  Given a three-replica stateful workload using Raft majority quorum
  And a network partition isolates one replica R from the other two
  When R receives a write request
  Then R MUST refuse the write
  And R MUST NOT advertise itself as leader in new ServiceRecords
```

```gherkin
Scenario: Duplicate replicas after Machineplane race
  Given a workload with desired_replicas_at_least = 1
  And the Machineplane has started two replicas A and B due to scheduling races
  When both replicas register in the WDHT
  Then the Workplane MUST ensure that at most one replica is leader for writes
  And surplus replicas MAY remain as followers or standby according to policy
```

### **14.4 Feature: Self-Healing with "At-Least" Semantics**

```gherkin
Scenario: Stateless self-healing based on replica counts
  Given a stateless workload with desired_replicas_at_least = 3
  And only 2 healthy replicas are observed via WDHT and local health checks
  When one of the existing replicas detects the shortfall
  Then it MAY request an additional replica from the Machineplane
  And the Machineplane MUST use the manifest it already possesses for this workload
  And the workload SHOULD eventually converge to at least 3 healthy replicas
```

```gherkin
Scenario: Overprovisioned replicas do not break Raft consistency
  Given a stateful workload with desired_replicas_at_least = 1
  And 3 healthy replicas exist due to prior Machineplane scheduling events
  When clients perform writes
  Then only the Raft leader replica MUST accept writes
  And all replicas MUST continue to enforce leader-only and majority-only semantics
```

### **14.5 Feature: Stateless Traffic Routing**

```gherkin
Scenario: Client load-balances across stateless replicas
  Given a stateless workload with 3 healthy replicas A, B, and C
  And all replicas publish ServiceRecords with role=stateless and health=healthy
  When a client resolves the service_id from WDHT
  Then the client SHOULD receive all 3 replicas as candidates
  And the client MAY send requests to any of them
  And each replica MUST handle requests locally without proxying
```

```gherkin
Scenario: Stateless replica handles request locally
  Given a stateless workload replica R
  And R has published a ServiceRecord with role=stateless
  When R receives any request (regardless of leader_only flag)
  Then R MUST process the request locally
  And R MUST NOT proxy to another replica
  And R MUST return the response directly to the client
```

```gherkin
Scenario: Unhealthy stateless replica removed from routing
  Given a stateless workload with replicas A (healthy), B (healthy), and C (unhealthy)
  And A and B publish ServiceRecords with health=healthy
  And C publishes a ServiceRecord with health=unhealthy (or lets it expire)
  When a client resolves the service_id from WDHT
  Then the client SHOULD filter out C from the candidate list
  And the client SHOULD only route requests to A or B
```

```gherkin
Scenario: New stateless replica immediately serves traffic
  Given a stateless workload with 2 existing healthy replicas
  And the Machineplane starts a new replica R
  When R completes startup and passes readiness checks
  Then R MUST publish a ServiceRecord with role=stateless and health=healthy
  And clients resolving from WDHT SHOULD discover R immediately
  And R MUST handle requests without waiting for any election or coordination
```

If you want, next step I can extract from this spec a **minimal “implementor checklist”** just for the Workplane Agent (things you must code).
