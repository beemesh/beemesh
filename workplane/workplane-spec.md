# **Beemesh Workplane — Normative Specification (v0.2)**

> **Scope:** This document specifies the **Workplane** of Beemesh: per-workload service discovery, workload identity, secure workload-to-workload connectivity, and self-healing for both stateful and stateless workloads. It uses RFC 2119 terminology and provides executable-style Gherkin scenarios.
>
> **Semantics:** The Workplane is **Consistency/Partition-Tolerant (C/P)**. It scopes consistency to each workload’s own trust domain (for example, a Raft quorum). The Machineplane is A/P and schedules via a **tender → bid → award** process; the Workplane **MUST** enforce correctness for replicas that are awarded.

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
* **Workload DHT (WDHT)**: libp2p-based distributed key-value store for **workload-level** service discovery and metadata (separate from the Machine DHT).
* **Role**: Logical role of a replica, for example `leader | follower | read-only | standby | stateless`.
* **Desired Replica Count**: Target number of replicas, interpreted as an **“at least”** value at runtime (`desired_replicas_at_least`).

---

## **3. Architecture**

### **3.1 Components**

* **Workplane Agent** (sidecar or in-process library):

  * Generates or loads a **unique Workload Peer ID per replica** and manages associated keys.
  * Uses **libp2p** to participate in the **WDHT** and maintain connections.
  * Registers and refreshes **ServiceRecord** entries in WDHT.
  * Observes local health and Raft state (for stateful workloads).
  * Enforces **leader-only / majority-only write semantics** based on workload consensus.
  * Implements **self-healing** via:

    * Observed **replica counts** for stateless workloads.
    * **Raft-based status** for stateful workloads.
  * Triggers **Machineplane interactions** via local APIs when more replicas are needed; the Machineplane uses the **existing manifests** it already owns.

* **Workload DHT (WDHT)**:

  * Implemented using **libp2p DHT** or equivalent.
  * Stores **ServiceRecord** entries (service endpoints, roles, health, version, etc.).
  * Stores **transient** health and role information with TTL.
  * **MUST NOT** store machine-level data (that belongs to the Machine DHT).

* **Secure Workload Streams**:

  * **libp2p streams** between Workload Peer IDs.
  * **Mutually authenticated**, encrypted channels.
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

* **Workload Peer ID**: Unique per replica (format implementation-defined, for example libp2p `PeerId`).
* **Service ID**: String or structured identifier for a logical service (for example, `payments.v1`).
* **Replica ID**: Monotonic or ULID-style identifier for a particular replica instance.

### **5.2 Workload DHT Records**

The WDHT **MUST** support at least the following logical record type.

#### **5.2.1 ServiceRecord**

* `service_id`: Service ID.
* `workload_peer_id`: Workload Peer ID.
* `replica_id`: Replica ID.
* `addresses`: List of dialable addresses (IP:port, DNS name, or overlay/libp2p address).
* `role`: `leader | follower | read-only | standby | stateless`.
* `health`: `healthy | degraded | unhealthy | unknown`.
* `version`: Opaque version string (for example, image tag, commit hash).
* `ts`: i64 (ms epoch).
* `ttl_ms`: u32.
* `sig`: Signature by the Workload Peer ID key.

**Interpretation:**

* ServiceRecords are **ephemeral, signed, TTL’d hints**.
* Consumers **MUST**:

  * Validate `sig` against the `workload_peer_id`.
  * Treat records as **hints**, not ground truth.
  * Ignore expired records (`now - ts > ttl_ms`).
  * Ignore unknown fields for forward compatibility.

### **5.3 Cryptography**

* All WDHT records **MUST** be signed by the corresponding Workload Peer ID.
* Receivers **MUST** validate signatures before using any data.
* Workplane Agents **SHOULD** attach `ts` and a `nonce` to messages and records for replay mitigation.

---

## **6. Consistency & Role Semantics**

The Workplane relies on **workload-owned Raft consensus** for stateful workloads and enforces policies derived from it.

### **6.1 Stateless Workloads**

For workloads marked `stateful = false` (or `role = stateless`):

* Any **healthy** replica **MAY** serve both reads and writes.
* Clients **SHOULD** use WDHT to discover all healthy replicas and **MAY** load-balance across them.
* Self-healing for stateless workloads is based **only on replica counts** relative to `desired_replicas_at_least`.

### **6.2 Stateful Workloads**

For workloads with `stateful = true` in the manifest:

* The workload implementation **MUST** implement **Raft** (or equivalent consensus) within its trust domain.

* The Workplane **MUST NOT** implement fabric-wide consensus, but **MUST**:

  * Enforce **leader-only** semantics for writes.
  * Enforce **majority-only** semantics when indicated by the Raft configuration (for example, majority quorum).

* A replica that is **not** the leader (according to Raft) **MUST**:

  * Reject write attempts (for example, error or redirect to leader).
  * Avoid advertising itself as a write endpoint in WDHT (or mark `role != leader`).

* A replica that cannot contact a majority of peers (minority partition) **MUST NOT** accept writes, even if it previously believed itself to be leader. It **MUST**:

  * Demote its effective role for write handling.
  * Stop publishing `role = leader` ServiceRecords.

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

The Workplane Agent is responsible for **self-healing within the workload’s trust domain**, while delegating replica creation to the Machineplane.

* Agents **MUST** monitor local workload health using:

  * Liveness checks (process/container health).
  * Readiness checks (accepting traffic, dependencies available).

* For **stateless workloads**:

  * Agents **MUST** count **healthy replicas** (local + WDHT).
  * When counts drop below `desired_replicas_at_least`, an Agent **MAY** request more replicas via **local Machineplane APIs**.

* For **stateful workloads**:

  * Agents **MUST** use Raft state (leader/follower, quorum) to derive allowed operations.
  * Agents **MUST** enforce leader-only/majority-only semantics and minority-partition refusal as in **6.2**.
  * Agents **MAY** request more replicas via Machineplane when healthy replicas drop below `desired_replicas_at_least`, but **MUST NOT** relax consistency rules to compensate.

* On repeated failures:

  * Agents **SHOULD** apply backoff with jitter for Machineplane requests or restarts.
  * Agents **SHOULD** emit structured events with reason codes.

* Workplane Agents **MUST NOT** directly manipulate the Machineplane’s scheduling logic, but **MAY** emit **tenders or hints** describing desired replica counts and placement preferences.

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

## **13. Gherkin Scenarios (Executable-Style)**

### **13.1 Feature: Workload Identity & Secure Streams**

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

### **13.2 Feature: Service Discovery & Roles**

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

### **13.3 Feature: Consistency under Partition & Duplicates**

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

### **13.4 Feature: Self-Healing with "At-Least" Semantics**

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

### **13.5 Feature: Stateless Traffic Routing**

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
