# **Beemesh Machineplane — Normative Specification (v0.2)**

> Scope: This document specifies the **Machineplane** of Beemesh: node discovery, ephemeral scheduling, deployment, security, failure handling, and observability. It uses RFC 2119 keywords and provides executable-style Gherkin scenarios.
> **Profile:** Rust implementation using `libp2p` (Kademlia DHT + Gossipsub + Noise/TLS).
> **Semantics:** Machineplane scheduling is **best-effort and duplicate-tolerant (A/P)**. **Consistency for stateful workloads is guaranteed by the Workplane (C/P).**

-----

## **1. Overview & Goals**

The Machineplane is a **stateless, decentralized infrastructure layer** that turns machines into fungible resources. It coordinates **node discovery, ephemeral scheduling, and workload deployment** using `libp2p` primitives (**DHT + Pub/Sub + secure streams**). It **MUST NOT** persist fabric-wide state and **MUST** operate under **Availability/Partition-Tolerance (A/P)** tradeoffs.

**Scheduling semantics:** Elections on the Machineplane are **hint-based**; **multiple concurrent lease hints MAY occur**. The Machineplane provides **at-least-once, duplicate-tolerant** scheduling. Exact leadership and write safety for stateful workloads are enforced by the **Workplane**.

### **1.1 Non-Goals**

  * Application service discovery and pod-to-pod overlay networking (**Workplane** responsibility).
  * Workload state/consensus (carried by workloads/Workplane).

### **1.2 Glossary (selected)**

  * **MDHT**: Machine DHT (libp2p Kademlia).
  * **Task**: Scheduling intent to run a workload.
  * **Bid**: A machine’s proposal to run a Task.
  * **LeaseHint**: Short-lived **non-exclusive** claim record to progress deployment.

-----

## **2. Architecture**

### **2.1 Components**

  * **Machine Daemon (Rust)**: single lightweight process per node (50–80 MiB RAM). It **MAY** be packaged and run as an OCI **container**; when containerized it **MUST**:

      * mount the host container runtime socket (e.g., Podman `/run/podman/podman.sock` or `$XDG_RUNTIME_DIR/...`),
      * provide networking suitable for libp2p inbound/outbound (host networking **RECOMMENDED**; otherwise expose ports),
      * have permissions to query/apply cgroup limits via the runtime,
      * **NOT** mount workload credentials; machine and workload identities stay disjoint.

  * **Machine DHT (MDHT)**: node discovery, transient metadata, **LeaseHint** storage (TTL).

  * **Pub/Sub Topics (Gossipsub)**:

      * `scheduler-tasks` — task publication (write-once, best-effort fanout)
      * `scheduler-proposals` — bid publication / request-response to the task publisher
      * `scheduler-events` — confirmations (`Deployed`, `Failed`, `Preempted`, `Cancelled`)

  * **Secure Streams**: bilateral encrypted channels for optional point-to-point negotiation.

  * **Runtime Adapter**: Podman executor (default) + pluggable interface for other runtimes.

### **2.2 Data Isolation**

  * MDHT **MUST** store **only minimal, transient** machine metadata and **LeaseHints** with TTL.
  * Workload DHT **MUST NOT** contain any machine-level data.

### **2.3 Diagrams**

```mermaid
sequenceDiagram
  autonumber
  participant P as Producer (kubectl/controller)
  participant N1 as Node A
  participant N2 as Node B
  participant MDHT as Machine DHT
  participant R as Runtime (Podman)
  participant PubSub as Pub/Sub

  P->>PubSub: Publish Task(task_id, reqs, qos, manifest_ref)
  Note over N1,N2: All nodes receive task via Pub/Sub

  N1->>N1: Local Evaluate(reqs, policies)
  N2->>N2: Local Evaluate(reqs, policies)

  N1->>PubSub: Bid(task_id, node_id, score, fit, latency)
  N2->>PubSub: Bid(task_id, node_id, score, fit, latency)

  par Selection Window (~250ms ± jitter)
    N1->>N1: best_bid = argmax(score)
  and
    N2->>N2: best_bid = argmax(score)
  end

  N1->>MDHT: Put LeaseHint(task_id, node_id, ttl=3s) [best-effort]
  N2->>MDHT: Put LeaseHint(task_id, node_id, ttl=3s) [best-effort]

  alt One or more LeaseHints exist
    N1->>R: Deploy(manifest_ref) (if self-hint or allowed by policy)
    R-->>N1: Deployment Result (OK / Failed)
    N1->>PubSub: Event(task_id, status=Deployed|Failed, node_id)
  else No hint observed
    N2->>N2: Backoff and observe events
  end
````

```mermaid
stateDiagram-v2
  [*] --> Idle
  Idle --> Bidding: TaskSeen
  Bidding --> Idle: NotEligible
  Bidding --> Claiming: BestBidder
  Claiming --> Deploying: HintObservedOrWritten
  Claiming --> Observing: HintLost
  Deploying --> Running: DeployOK
  Deploying --> Observing: DeployFail
  Running --> Observing: EventPublished
  Observing --> Idle: TTLExpire | TaskComplete
```

```mermaid
graph TD
  subgraph Machineplane
    A[Node Daemon] -- secure streams --> B[Node Daemon]
    A -- MDHT --> C[(Ephemeral Records)]
    B -- MDHT --> C
    A ==> D{{Pub/Sub}}
    B ==> D
    D -- tasks/proposals/events --> A
    D -- tasks/proposals/events --> B
  end

  subgraph "Workplane separate"
    E[Pod Workload]
  end

  A -.runtime adapter.-> E
```

---

## **3. Protocols & Data Models**

### **3.1 Identifiers**

* **Machine Peer ID**: `libp2p` peer ID (base58). **MUST** be unique per machine.
* **Task ID**: ULID string. **MUST** be globally unique for de-duplication.
* **Lease Key**: `lease/<task_id>` in MDHT (value is a **LeaseHint**, not an exclusive lock).

### **3.2 Message Schemas (FlatBuffer Protocol)**

All inter-node communication uses **FlatBuffers** wrapped in signed **Envelope** structures.

**Task (additions)**

* `workload_type`: `"stateless" | "stateful"`
* `duplicate_tolerant`: `bool` (default `true`)
* `max_parallel_duplicates`: `u32` (hint; default `1`)
* `placement_token`: ULID (monotonic per `(workload_id, attempt)`; hint for Workplane fencing)

**LeaseHint** (MDHT value)

* `task_id`: ULID
* `node_id`: Machine Peer ID
* `score`: f64
* `ttl_ms`: u32 (e.g., 3000)
* `renew_nonce`: u64 (monotonic per renewal)
* `ts`: i64 (ms epoch)
* `sig`: ed25519 signature (by `node_id`)

> **Interpretation:** `LeaseHint` is **non-exclusive**. It signals intent and enables backoff; multiple hints **MAY** coexist.

**Existing messages** (`ApplyRequest/Response`, `CapacityRequest/Reply`, `Envelope`) stay as in v0.1.

### **3.3 Cryptography**

* All messages **MUST** be signed by the sender’s **Machine Peer ID** key (Ed25519 recommended).
* All streams **MUST** use `libp2p` **Noise** or **TLS** with mutual authentication.
* Messages **MUST** include `ts` and **SHOULD** include `nonce` for replay mitigation.
* Receivers **MUST** reject messages with clock skew > **±30 s** or a repeated `(task_id, node_id, nonce)` tuple within **5 minutes**.

---

## **4. Scheduling Algorithm (Ephemeral, Duplicate-Tolerant)**

### **4.1 Flow (Normative)**

1. **Publication**: A producer **MUST** publish a Task once on `scheduler-tasks`.

2. **Evaluation**: Each node **MUST** locally evaluate eligibility against `reqs`, policies, and real-time resources, and **MUST NOT** submit a Bid if it is ineligible.

3. **Bid Window**: Nodes **SHOULD** submit ≤ 1 Bid per Task within **250 ms ± 100 ms** jitter from first receipt.

4. **Score**: Score **MUST** be deterministic given Task + local metrics. **SHOULD** use: Resource fit (50%), Network locality (30%), Historical reliability (10%), Price/QoS or energy (10%).

5. **Lease Hint (non-exclusive)**:

   * Nodes **MUST** compute `best_bid` (tie-break `(score, node_id)`).
   * The `best_bid` node **MAY** write a **LeaseHint** at `lease/<task_id>` with TTL **3 s** (best-effort MDHT put). **Multiple LeaseHints MAY coexist**.

6. **Deployment**:

   * A node **MAY** proceed to deploy if it wrote or observes a **self-addressed** LeaseHint **and** either `duplicate_tolerant=true` **or** local policy allows tentative start (the **Workplane** gates stateful writes/leadership).
   * The Machineplane provides **at-least-once** scheduling semantics; multiple concurrent deployments of the same Task are acceptable, and correctness is enforced by the Workplane/workload logic.

7. **Confirmation**: Deployer **MUST** publish `Event{Deployed|Failed}`.

8. **Lease Renewal**: Deployer **SHOULD** refresh its LeaseHint every **1 s** until `Deployed` or timeout **(10 s)**.

9. **Backoff**: Non-deployers **MUST** observe `scheduler-events` and **SHOULD** back off with jitter; if no `Deployed` by `(lease_ttl + 1 s)`, they **MAY** re-enter Claiming.

### **4.2 Resource Accounting**

* Nodes **MUST** pessimistically reserve requested resources upon deciding to deploy and **MUST** release on failure or after `Event{Deployed}` emission.
* Overcommit **MAY** be supported via policy; when enabled, nodes **MUST** expose overcommit ratios per resource.

### **4.3 Preemption**

* Tasks marked `qos.preemptible=true` **MAY** be evicted by higher priority tasks. Evicting node **MUST** publish `Event{Preempted}` and **MUST** best-effort re-schedule the evicted task by republishing the original Task payload.

> **Stateful note:** The **Workplane** enforces leader election and minority write refusal. Machineplane **MAY** start more than one replica transiently; Workplane **MUST** gate writes.

---

## **5. Security & Identity (Normative)**

* Machine and Workload identities **MUST** be disjoint keyspaces.
* Machine-to-machine topics and streams **MUST NOT** accept Workload credentials.
* Nodes **MUST** verify message signatures and **MUST** discard unauthenticated content.
* Nodes **SHOULD** apply admission policies (allow/deny lists, org cert chains) before bidding.

---

## **6. Failure Handling**

| Failure                             | Required Behavior                                                                                                                                                                                                                                                              |
| ----------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **Network Partition**               | Nodes continue local bidding and **LeaseHints**. On heal, **duplicate deployments MAY exist**. Nodes **SHOULD** prefer the deployment with freshest local health and event visibility; excess replicas **SHOULD** be drained or **MUST** respond to Workplane signals to exit. |
| **Winner crash before deploy**      | LeaseHint expiry triggers re-bidding/backoff.                                                                                                                                                                                                                                  |
| **Late bids**                       | Ignored for current window; **MAY** be used for retry if deploy fails.                                                                                                                                                                                                         |
| **Conflicting LeaseHints**          | Nodes **MUST** tolerate multiple hints; **MUST NOT** assume exclusivity. Prefer self-hint + highest score; otherwise observe `scheduler-events` and back off.                                                                                                                  |
| **Manifest fetch fail**             | Deployer **MUST** emit `Event{Failed}` and **MAY** retry per local policy.                                                                                                                                                                                                     |
| **Global bootstrap loss (paradox)** | If the Machineplane and all workloads are lost simultaneously, Beemesh alone **CANNOT** restore the fabric. An external bootstrap mechanism (for example, installers, image registries, GitOps controllers) is **REQUIRED** to repopulate the cluster.                         |

---

## **7. Runtime Adapter (Podman)**

### **7.1 Contract**

* **MUST** support: pull image, load manifest, create network namespace (as configured by Workplane), start container(s), stream logs, report status.
* **MUST** return a **DeploymentID** and current cgroup allocations.
* **MUST** enforce requested resource limits (e.g., `cpu.shares`, `memory.limit_in_bytes`).

### **7.2 Example Invocation**

```bash
podman play kube deployment.yaml --replace --log-driver=journald
```

---

## **8. Configuration Surface (Node)**

```yaml
machineplane:
  topics:
    tasks: scheduler-tasks
    proposals: scheduler-proposals
    events: scheduler-events
  selection_window_ms: 250
  selection_jitter_ms: 100
  lease_ttl_ms: 3000
  lease_renew_ms: 1000
  deploy_timeout_ms: 10000
  clock_skew_ms: 30000
  duplicate_tolerant_default: true
  max_parallel_duplicates: 2    # hint; not a hard guarantee
  policies:
    overcommit:
      cpu: 1.2
      mem: 1.0
    affinities:
      include: ["region:eu"]
      exclude: []
  runtime: podman
  security:
    require_signed_messages: true
    accepted_cas: ["spiffe://org/ca"]
  packaging:
    run_mode: daemon|container   # either native systemd service or OCI container
```

---

## **9. Observability**

### **9.1 Events (Pub/Sub)**

* **MUST** be emitted for: `Deployed`, `Failed`, `Preempted`, `Cancelled`.

### **9.2 Metrics (Prometheus-style)**

* `machineplane_tasks_seen_total{task_id}`
* `machineplane_bids_submitted_total{task_id}`
* `machineplane_leasehint_put_total{result="ok|error"}`
* `machineplane_schedule_latency_ms_bucket{}`          # publish → first deploy attempt
* `machineplane_deploy_failures_total{reason}`
* `machineplane_duplicate_deploys_total{task_id}`
* `machineplane_reconcile_kills_total{reason}`

### **9.3 Logs**

* **SHOULD** include Task ID, Node ID, **LeaseHint** fields (`renew_nonce`, `ttl_ms`), and causal span IDs.

---

## **10. CLI Integration (`kubectl`)**

* `kubectl create -f app.yaml` **MUST** publish a Task to `scheduler-tasks`.
* `kubectl get pods` **SHOULD** read from local node/runtime and/or observe `scheduler-events` for status aggregation (best-effort).
* `kubectl delete pod <name>` **MUST** result in the Machineplane publishing a cancellation Task or sending a secure stream command to the owning node.
* The Machineplane daemon **MUST** expose Kubernetes-compatible REST endpoints (`/version`, `/api`, `/apis/apps/v1/...`) so that the upstream `kubectl` binary can talk to it using its normal HTTP flow.

---

## **11. Interop & Compatibility**

* Wire formats **MUST** be FlatBuffers with signed Envelope wrappers; unknown fields in FlatBuffers are automatically ignored (forward-compat).
* Versioning **MUST** appear in FlatBuffer schema evolution; nodes **MUST** expose supported protocol versions in MDHT metadata.

---

## **12. Security Considerations**

* Replay protection via `(ts, nonce)`; **MUST** reject duplicates.
* Rate limits on `scheduler-proposals`; nodes **SHOULD** cap bids/sec per peer.
* **MUST** sandbox runtime with least privilege and drop ambient capabilities not required by the manifest.

---

## **13. Gherkin Scenarios (Executable-style)**

### **13.1 Feature: Node Discovery & Identity**

```gherkin
Scenario: New node joins the Machine DHT
  Given a fresh machine with no persisted state
  When the Rust daemon starts
  Then it MUST generate a unique Machine Peer ID
  And it MUST announce presence in the MDHT with a TTL record
  And other nodes SHOULD be able to discover it within 2 seconds
```

```gherkin
Scenario: Reject unsigned scheduler messages
  Given a running node subscribed to scheduler topics
  When it receives an unsigned Task
  Then it MUST discard the message
  And it MUST NOT emit a Bid
```

### **13.2 Feature: Ephemeral, Duplicate-Tolerant Scheduling**

```gherkin
Scenario: At-least-once scheduling with LeaseHints
  Given two eligible nodes A and B
  And a Task T published at time t0
  When A and B submit valid Bids within the selection window
  And A and/or B write LeaseHint(T) in the MDHT
  Then one or more nodes MAY proceed to deploy T
  And at least one node MUST publish Event{Deployed} if deployment succeeds
```

```gherkin
Scenario: Partition with conflicting LeaseHints
  Given a network split causing duplicate deployments for T
  When the partition heals
  Then nodes MUST tolerate the duplicates temporarily
  And nodes SHOULD drain excess replicas
  And nodes MUST honor Workplane leader/consistency signals when present
```

```gherkin
Scenario: Deployment timeout triggers retry
  Given a node is deploying T
  When deployment does not complete within deploy_timeout_ms
  Then the node MUST publish Event{Failed}
  And nodes MAY re-enter Claiming for T after LeaseHint expiry
```

### **13.3 Feature: Preemption**

```gherkin
Scenario: Higher priority Task preempts lower priority Task
  Given a node running Task L (low priority)
  And the node is fully utilized
  When a Task H (high priority) arrives with qos.preemptible=true
  Then the node MUST evict L to make space for H
  And the node MUST publish Event{Preempted} for L
  And the node MUST republish L to the scheduler-tasks topic
```
