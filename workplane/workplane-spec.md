# **Beemesh Workplane — Normative Specification (v0.1)**

> Scope: This document specifies the **Workplane** of Beemesh: per-workload service discovery, workload identity, secure workload-to-workload connectivity, self-healing, manifest-based recovery, and consistency gating for stateful workloads. It uses RFC 2119 keywords and provides executable-style Gherkin scenarios.
>
> **Profile:** Implementation-agnostic. A reference implementation MAY use `libp2p`-style DHTs and secure streams.
>
> **Semantics:** The Workplane is **Consistency/Partition-Tolerant (C/P)**: it scopes consistency to each workload’s own trust domain (for example, a database quorum). The Machineplane is A/P and **duplicate-tolerant**; the Workplane **MUST** enforce correctness in the presence of duplicate replicas.

-----

## **1. Overview & Goals**

The Workplane is a **per-workload control and data plane** that runs inside (or alongside) each Pod or workload instance. It provides:

  * **Service discovery** via the **Workload DHT (WDHT)**.
  * **Secure workload-to-workload networking** using mutually authenticated, encrypted streams.
  * **Self-healing** and **replica management** using local manifest data.
  * **Consistency gating** for **stateful** workloads (leader election, minority-write refusal).
  * **Manifest-based recovery** to restart workloads even when isolated.

The Workplane **MUST** assume the Machineplane can:

  * Start **more than one replica** of a workload (duplicate-tolerant scheduling).
  * Lose or reassign machines without warning.

The Workplane **MUST** therefore keep workloads **self-contained** and **safe under duplication**.

### **1.1 Non-Goals**

  * Infrastructure-level node discovery, bidding, and scheduling (**Machineplane** responsibility).
  * Container runtime interaction (pulling images, starting containers, cgroups).
  * Global fabric state, multi-workload transactions, or cross-workload consensus.

-----

## **2. Glossary (selected)**

  * **Workload**: A logical application unit (for example, `payments-api`, `orders-db`) consisting of one or more replicas.
  * **Replica**: A single running instance (pod/container set) of a workload.
  * **Workload Peer ID**: Unique cryptographic identity for a replica within the Workplane.
  * **Service ID**: Stable identifier for a logical service exposed by a workload.
  * **Workload DHT (WDHT)**: Distributed key-value store for service discovery and workload metadata (separate from the Machine DHT).
  * **Role**: Logical role of a replica (for example, `leader`, `follower`, `read-only`, `standby`).
  * **Desired Replica Count**: Declared target number of replicas; interpreted as an **“at least”** value at runtime.

-----

## **3. Architecture**

### **3.1 Components**

  * **Workplane Agent**: A sidecar process or in-process library that:

      * Manages the **Workload Peer ID** and secure keys.
      * Registers and refreshes **service records** in the WDHT.
      * Observes local health and replica state.
      * Manages **self-healing** based on local manifest data.
      * Enforces **consistency rules** for stateful workloads.

  * **Workload DHT (WDHT)**:

      * Stores **service discovery** records (Service → endpoints, roles, metadata).
      * Stores **transient health and role information** with TTL.
      * **MUST NOT** store machine-level data (that belongs to the MDHT).

  * **Secure Streams (Workload-to-Workload)**:

      * Mutually authenticated, encrypted channels between Workload Peer IDs.
      * Isolated from machine-to-machine streams and credentials.

  * **Local Manifest Store**:

      * Holds the workload manifest (desired replica counts, resource requests, roles, etc.).
      * **MAY** be backed by a file, CRD, GitOps source, or injected configuration.

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

  WDHT[(Workload DHT)]
  NET{{Secure Workload Streams}}

  MD --> RT
  RT -.hosts.-> Pod
  WA -->|register/lookup| WDHT
  APP <-->|mTLS / encrypted| NET
  NET <-->|mTLS / encrypted| APP
```

---

## **4. Identities & Trust**

* Workload identities **MUST** be disjoint from machine identities:

  * Workload Peer IDs **MUST NOT** reuse Machine Peer ID keys.
  * Machineplane topics and streams **MUST NOT** accept Workplane credentials, and vice versa.

* Each replica **MUST** have a unique **Workload Peer ID**, backed by a public/private key pair.

* All workload-to-workload streams **MUST** be mutually authenticated and encrypted:

  * Noise or TLS handshakes **MUST** verify both peers’ Workload Peer IDs.
  * Unauthenticated streams **MUST** be rejected.

* Workplane Agents **SHOULD** support:

  * Allow/deny lists and/or org trust chains.
  * Policy-based admission of peer workloads (for example, same tenant, same org).

---

## **5. Protocols & Data Models**

### **5.1 Identifiers**

* **Workload Peer ID**: Unique per replica. Format is implementation-defined (for example, libp2p PeerId).
* **Service ID**: String or structured identifier for a logical service (for example, `payments.v1`).
* **Replica ID**: Monotonic or ULID-style identifier for a particular replica instance.

### **5.2 Workload DHT Records**

The WDHT **MUST** support at least the following logical record types.

#### **5.2.1 ServiceRecord**

* `service_id`: Service ID.
* `workload_peer_id`: Workload Peer ID.
* `replica_id`: Replica ID.
* `addresses`: List of dialable addresses (IP:port, DNS name, or overlay address).
* `role`: `leader | follower | read-only | standby | stateless`.
* `health`: `healthy | degraded | unhealthy | unknown`.
* `version`: Opaque version string (for example, image tag, commit hash).
* `ts`: i64 (ms epoch).
* `ttl_ms`: u32.
* `sig`: Signature by the Workload Peer ID key.

**Interpretation:**

* Records are **ephemeral**; readers **MUST** treat them as hints.
* Consumers **MUST** ignore expired records (`now - ts > ttl_ms`).
* Unknown fields **MUST** be ignored to allow forward compatibility.

#### **5.2.2 ManifestRecord (optional)**

Workplane implementations **MAY** also publish manifest hints to WDHT:

* `service_id`
* `desired_replicas_at_least`: u32
* `stateful`: bool
* `consensus_scheme`: string (for example, `raft`, `paxos`, `native`)

This record is **advisory**; the **authoritative manifest** lives in the Local Manifest Store for each replica.

### **5.3 Cryptography**

* WDHT records **MUST** be signed by the corresponding Workload Peer ID.
* Receivers **MUST** validate signatures before using data.
* Workplane Agents **SHOULD** attach `ts` and a `nonce` to messages for replay mitigation.

---

## **6. Consistency & Role Semantics**

The Workplane **MUST** respect the following semantics:

### **6.1 Stateless Workloads**

* For `role=stateless`:

  * Any **healthy** replica **MAY** serve both reads and writes.
  * Clients **SHOULD** use WDHT to discover available replicas and may load-balance across them.

### **6.2 Stateful Workloads**

For stateful workloads (`stateful=true` in manifest):

* The workload implementation **MUST** carry its own consensus (for example, Raft inside the database).

* The Workplane **MUST NOT** implement fabric-wide consensus, but **MUST**:

  * Enforce **leader-only** semantics for writes.
  * Enforce **majority-only** semantics when so indicated by the workload/manifest.

* A replica that is **not** the leader **MUST**:

  * Reject write attempts (for example, return an error or redirect).
  * Avoid advertising itself as a write endpoint in WDHT (or mark `role != leader`).

* A replica that cannot contact a majority of peers (minority partition) **MUST NOT** accept writes, even if it previously believed itself to be leader.

### **6.3 Duplicate Replicas and “At-Least” Semantics**

Because the Machineplane is duplicate-tolerant:

* Workplane Agents **MUST** assume that **more than one replica** of the same workload may be running.

* Desired replica counts **MUST** be interpreted as **“at least N”**:

  * The Workplane **MUST** attempt to keep **at least** `desired_replicas_at_least` healthy replicas.
  * Having more than the desired count **MAY** happen transiently and **MUST NOT** violate correctness for stateful workloads (leader/write gating still applies).

* Workplane Agents **SHOULD** implement policies to:

  * Prefer draining or idling surplus replicas when above target.
  * Prefer keeping replicas that are healthiest and most up-to-date.

---

## **7. Self-Healing & Replica Management**

The Workplane Agent is responsible for **self-healing within a workload’s trust domain**.

* Agents **MUST** monitor local workload health using:

  * Liveness checks (process running, container health).
  * Readiness checks (accepting traffic, dependencies available).

* When the number of **healthy** replicas observed (locally + via WDHT) falls below `desired_replicas_at_least`, an Agent **MAY**:

  * Request additional replicas via a local mechanism (for example, write to a manifest controller or trigger a Machineplane tender).
  * Or, in single-node environments, restart the local replica from manifest.

* On repeated failures:

  * Agents **SHOULD** back off with jitter.
  * Agents **SHOULD** emit structured events indicating reason codes.

* Workplane Agents **MUST NOT** directly manipulate the Machineplane scheduling algorithm, but **MAY** produce tenders or hints to be consumed by the Machineplane.

---

## **8. Service Discovery & Routing**

* Agents **MUST** register `ServiceRecord` entries for all services a workload exposes.

* Agents **MUST** periodically refresh records before `ttl_ms` expires.

* Clients (Workplane-aware callers) **MUST**:

  * Resolve `service_id` → set of `ServiceRecord`s from WDHT.
  * Filter out expired or unhealthy records.
  * For stateful services, filter or prefer records with `role=leader` for writes.

* Implementations **SHOULD** support:

  * Client-side load-balancing across stateless replicas.
  * Simple routing policies (for example, locality hints, version-based routing).

---

## **9. Failure Handling**

| Failure Scenario                        | Required Workplane Behavior                                                                                                                                                                                        |
| --------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **Replica crash**                       | Local Agent **MUST** mark the replica unhealthy and/or remove its `ServiceRecord` from WDHT. It **MAY** attempt restart based on manifest policies.                                                                |
| **Network partition (within workload)** | Agents on both sides **MUST** assume duplicates may exist. Statefulness rules apply: minority-side replicas **MUST NOT** accept writes.                                                                            |
| **Stale leader record in WDHT**         | Clients **MUST** tolerate write failures and **SHOULD** retry, resolving WDHT again. Agents **MUST** refresh leader/follower roles regularly; expired records **MUST** be ignored.                                 |
| **Surplus replicas**                    | When the number of healthy replicas exceeds `desired_replicas_at_least`, Agents **SHOULD** prefer draining or idling surplus replicas where policies dictate, but **MUST** maintain consistency gating for writes. |
| **Manifest mismatch / invalid config**  | Agents **MUST** fail fast and log clear errors when manifest data is invalid or incompatible; they **MUST NOT** silently relax security or consistency constraints.                                                |

---

## **10. Observability**

### **10.1 Events**

Workplane implementations **SHOULD** emit events (for example, over Pub/Sub or logs) for:

* `ReplicaStarted`
* `ReplicaStopped`
* `RoleChanged` (for example, follower → leader)
* `HealthChanged`
* `ConsistencyViolation` (for example, attempted write on non-leader node)
* `ReplicaOverprovisioned` / `ReplicaDrained`

### **10.2 Metrics (Prometheus-style)**

Suggested metrics:

* `workplane_service_records_total{service_id, role}`
* `workplane_leader_changes_total{service_id}`
* `workplane_consistency_violations_total{service_id, reason}`
* `workplane_replica_health_total{service_id, status="healthy|unhealthy|degraded"}`
* `workplane_selfheal_actions_total{service_id, action="restart|scale_up|scale_down"}`

### **10.3 Logs**

Workplane logs **SHOULD** include:

* Workload Peer ID and Replica ID.
* Service ID and role.
* Correlation IDs for incoming/outgoing requests where applicable.
* Manifest version and relevant consistency flags.

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
* Different Workplane versions **SHOULD** be able to coexist within the same workload cluster as long as they share a minimal compatible schema.

---

## **13. Gherkin Scenarios (Executable-style)**

### **13.1 Feature: Workload Identity & Secure Streams**

```gherkin
Scenario: Workload rejects unauthenticated peer
  Given a running Workplane Agent with a valid Workload Peer ID
  And a remote peer connects without presenting a valid Workload certificate
  When the Agent evaluates the connection
  Then it MUST reject the connection
  And it MUST NOT exchange application data on this stream
```

```gherkin
Scenario: Distinct machine and workload identities
  Given a node with a Machine Peer ID
  And a workload replica with a Workload Peer ID
  When the workload establishes a workload-to-workload stream
  Then the stream MUST authenticate using the Workload Peer ID
  And it MUST NOT reuse the Machine Peer ID credentials
```

### **13.2 Feature: Service Discovery & Roles**

```gherkin
Scenario: Leader and follower registration in WDHT
  Given a stateful workload with one leader replica L and one follower replica F
  When L starts and becomes leader
  Then it MUST publish a ServiceRecord with role=leader in the WDHT

  When F starts and joins the cluster
  Then it MUST publish a ServiceRecord with role=follower in the WDHT
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

### **13.3 Feature: Consistency under Partition & Duplicates**

```gherkin
Scenario: Minority partition refuses writes
  Given a three-replica stateful workload using majority quorum
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

### **13.4 Feature: Self-Healing with “At-Least” Semantics**

```gherkin
Scenario: Self-healing spawns replacement replica
  Given a workload with desired_replicas_at_least = 3
  And only 2 healthy replicas are observed via WDHT and local health checks
  When one of the existing replicas detects the shortfall
  Then it MAY request an additional replica via the configured mechanism
  And the workload SHOULD eventually converge to at least 3 healthy replicas
```

```gherkin
Scenario: Overprovisioned replicas do not break consistency
  Given a workload with desired_replicas_at_least = 1
  And 3 healthy replicas exist due to prior scheduling events
  When clients perform writes
  Then only the leader replica MUST accept writes
  And all replicas MUST continue to enforce the same consistency rules
```
