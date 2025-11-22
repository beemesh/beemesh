# Scheduler Implementation Spec (machineplane/src/scheduler.rs)

This document describes the *actual* scheduler behavior in the `machineplane` crate, with particular focus on `machineplane/src/scheduler.rs` and the end‑to‑end integration tests under `machineplane/tests`.

It refines the high‑level Machineplane semantics (ephemeral, duplicate‑tolerant scheduling) with concrete data structures, algorithms, and invariants enforced by the current Rust implementation.

---

## 1. Scope

This spec covers:

- The `Scheduler` struct and its bid‑tracking model.
- Handling of Tenders, Bids, and Scheduler Events on Gossipsub topics.
- Winner selection and replica distribution behavior.
- Runtime integration details relevant to scheduling:
  - Manifest decoding and modification (replicas → 1).
  - Runtime engine selection.
  - `DeploymentConfig` construction.

Out of scope:

- Full libp2p behaviour wiring, HTTP API, or CLI details.
- Workplane consistency behavior (delegated to workloads / other layers).
- CapacityVerifier internals (only use as implied in scheduler+runtime code).

---

## 2. Core Types and Responsibilities

### 2.1 `Scheduler`

Location: `machineplane/src/scheduler.rs`

```rust
/// Scheduler manages the bidding lifecycle
pub struct Scheduler {
    /// Local node ID (PeerId string)
    local_node_id: String,
    /// Active bids we are tracking: TenderID -> BidContext
    active_bids: Arc<Mutex<HashMap<String, BidContext>>>,
    /// Channel to send messages back to the network loop
    outbound_tx: mpsc::UnboundedSender<SchedulerCommand>,
}
```

**Responsibilities:**

- Subscribe to scheduler topics:
  - `SCHEDULER_TENDERS`
  - `SCHEDULER_PROPOSALS`
  - `SCHEDULER_EVENTS`
- For each incoming Tender:
  - Evaluate local eligibility (currently stubbed with a fixed capacity score).
  - Submit a Bid on the proposals topic.
  - Track all Bids (local + remote) in `active_bids`.
  - After a fixed selection window, select winners via `select_winners`.
  - If local node is a winner:
    - Write a LeaseHint to MDHT.
    - Instruct runtime integration to deploy the workload locally.
- For incoming Bids:
  - Populate/extend the relevant `BidContext`.
- For incoming Scheduler Events:
  - Log and react to termination events relating to local deployments.

The scheduler is instantiated via:

```rust
impl Scheduler {
    pub fn new(
        local_node_id: String,
        outbound_tx: mpsc::UnboundedSender<SchedulerCommand>,
    ) -> Self { /*...*/ }
}
```

Callers **MUST** provide:

- `local_node_id`: stringified libp2p `PeerId` of the node.
- `outbound_tx`: unbounded channel that the main network loop consumes and maps to:
  - Gossipsub publish calls.
  - Kademlia `put_record` calls.
  - Runtime deployment calls.

### 2.2 Bid Tracking Structures

```rust
struct BidContext {
    tender_id: String,
    manifest_id: String,
    replicas: u32,
    bids: Vec<BidEntry>,
}

#[derive(Clone)]
struct BidEntry {
    bidder_id: String,
    score: f64,
}

#[derive(Clone)]
struct BidOutcome {
    bidder_id: String,
    score: f64,
}
```

**Semantics:**

- `BidContext` is **per Tender** (`tender_id`).
- `manifest_id` is currently derived as `tender.id` (1:1 mapping).
- `replicas` is the *target number of distinct winners* for this Tender.
  - Computed as: `replicas = max(1, tender.max_parallel_duplicates)`.
- `bids` collects the best known bid per node (after normalization, see `select_winners`).

The scheduler stores these contexts in:

```rust
active_bids: Arc<Mutex<HashMap<String, BidContext>>>
```

where the key is `tender_id`.

### 2.3 `SchedulerCommand` Channel Contract

```rust
#[derive(Debug, Clone)]
pub enum SchedulerCommand {
    Publish {
        topic: String,
        payload: Vec<u8>,
    },
    PutDHT {
        key: Vec<u8>,
        value: Vec<u8>,
    },
    DeployWorkload {
        manifest_id: String,
        manifest_json: String,
        replicas: u32,
    },
}
```

Implementations of the event loop consuming this channel **MUST**:

- Map `Publish` to Gossipsub publish with the provided `topic` and serialized `payload`.
- Map `PutDHT` to a Kademlia `put_record` (or equivalent) with arbitrary `key` / `value`.
- Map `DeployWorkload` to a local deployment request wired into the runtime integration (`runtime_integration::process_manifest_deployment`).

**Current usage from `Scheduler`:**

- `Publish` is used to send Bids (`scheduler-proposals`).
- `PutDHT` is used to publish a LeaseHint for a winning node.
- `DeployWorkload` is used by a winning node to trigger local deployment.

---

## 3. Message Handling

### 3.1 Topic Dispatch

```rust
pub async fn handle_message(
    &self,
    topic_hash: &gossipsub::TopicHash,
    message: &gossipsub::Message,
) {
    let tenders = gossipsub::IdentTopic::new(SCHEDULER_TENDERS).hash();
    let proposals = gossipsub::IdentTopic::new(SCHEDULER_PROPOSALS).hash();
    let events = gossipsub::IdentTopic::new(SCHEDULER_EVENTS).hash();

    if *topic_hash == tenders {
        self.handle_tender(message).await;
    } else if *topic_hash == proposals {
        self.handle_bid(message).await;
    } else if *topic_hash == events {
        self.handle_event(message).await;
    }
}
```

The node subscribes to all three topics and dispatches to:

- `handle_tender` for `SCHEDULER_TENDERS`
- `handle_bid` for `SCHEDULER_PROPOSALS`
- `handle_event` for `SCHEDULER_EVENTS`

### 3.2 Tender Handling → Local Bid

**Path:** `Scheduler::handle_tender`

Key steps (as implemented):

1. **Deserialize Tender**

   ```rust
   let tender = machine::decode_tender(&message.data)?;
   let tender_id = tender.id.clone();
   let manifest_id = tender.id.clone();
   ```

   - On parse failure, log an error and stop.

2. **Derive replica target**

   ```rust
   let replicas = std::cmp::max(1, tender.max_parallel_duplicates);
   ```

   - This enforces a **minimum of 1**.
   - `max_parallel_duplicates` is interpreted as **requested global replicas**.

3. **Evaluate capacity & compute score (stub)**

   ```rust
   let capacity_score = 1.0; // placeholder, assume perfect fit
   let my_score = capacity_score * 0.8 + 0.2;
   ```

   - Capacity evaluation is currently a stub and **MUST** be replaced later with:
     - real `CapacityVerifier` queries,
     - network locality,
     - reliability, etc.
   - The score is a deterministic function of `capacity_score` for now.

4. **Create `BidContext`**

   The scheduler writes a fresh context for this Tender:

   ```rust
   bids.insert(
       tender_id.to_string(),
       BidContext {
           tender_id: tender_id.to_string(),
           manifest_id: manifest_id.clone(),
           replicas,
           bids: vec![BidEntry {
               bidder_id: self.local_node_id.clone(),
               score: my_score,
           }],
       },
   );
   ```

   - The initial `bids` vector **always** contains a single local bid entry.

5. **Publish Bid**

   ```rust
   let now = SystemTime::now()
       .duration_since(UNIX_EPOCH)
       .unwrap()
       .as_millis() as u64;

   let bid_bytes = machine::build_bid(
       &tender_id,
       &self.local_node_id,
       my_score,
       capacity_score,
       0.5, // network locality placeholder
       now,
       &[], // signature placeholder
   );

   self.outbound_tx.send(SchedulerCommand::Publish {
       topic: SCHEDULER_PROPOSALS.to_string(),
       payload: bid_bytes,
   })?;
   ```

   - The Bid includes:
     - `tender_id`
     - `node_id` (local peer)
     - `score`
     - `capacity_score`
     - placeholder `network_locality`
     - timestamp
     - empty signature (`[]`) at present.

6. **Spawn selection window task**

   ```rust
   tokio::spawn(async move {
       sleep(Duration::from_millis(DEFAULT_SELECTION_WINDOW_MS)).await;
       // ... compute winners & deploy ...
   });
   ```

   - `DEFAULT_SELECTION_WINDOW_MS` is defined in `messages::constants` and effectively implements the bid window.
   - After sleeping, the task:
     - takes the `BidContext` out of `active_bids`,
     - calls `select_winners(&ctx, &local_id)`,
     - logs the outcome,
     - if the local node is among winners, proceeds with LeaseHint and deployment steps.

### 3.3 Bid Handling

**Path:** `Scheduler::handle_bid`

```rust
async fn handle_bid(&self, message: &gossipsub::Message) {
    match machine::decode_bid(&message.data) {
        Ok(bid) => {
            let tender_id = bid.tender_id.clone();
            let bidder_id = bid.node_id.clone();
            let score = bid.score;

            // Ignore our own bids (handled locally)
            if bidder_id == self.local_node_id {
                return;
            }

            let mut bids = self.active_bids.lock().unwrap();
            if let Some(ctx) = bids.get_mut(&tender_id) {
                info!(
                    "Recorded bid for tender {}: {:.2} from {}",
                    tender_id, score, bidder_id
                );
                ctx.bids.push(BidEntry {
                    bidder_id: bidder_id.to_string(),
                    score,
                });
            }
        }
        Err(e) => error!("Failed to parse Bid message: {}", e),
    }
}
```

**Semantics:**

- Remote Bids (`node_id != local_node_id`) for a known Tender are appended to the corresponding `BidContext.bids`.
- Local Bids published by this node’s scheduler are **ignored** here and only present via the initial entry added in `handle_tender`.
- Bids received **after** the context has been removed (i.e. after selection window elapses and `remove(tender_id)` is called) are effectively ignored.

### 3.4 Event Handling

**Path:** `Scheduler::handle_event`

```rust
async fn handle_event(&self, message: &gossipsub::Message) {
    match machine::decode_scheduler_event(&message.data) {
        Ok(event) => {
            let tender_id = event.tender_id.clone();
            info!(
                "Received Scheduler Event for tender {}: {:?}",
                tender_id, event.event_type
            );

            if event.node_id == self.local_node_id {
                use crate::messages::types::EventType;

                if matches!(
                    event.event_type,
                    EventType::Cancelled | EventType::Preempted | EventType::Failed
                ) {
                    info!(
                        "Received termination event for locally deployed tender {}",
                        tender_id
                    );
                }
            }
        }
        Err(e) => error!("Failed to parse SchedulerEvent message: {}", e),
    }
}
```

**Semantics:**

- The scheduler currently only **logs** events.
- For events where `event.node_id == local_node_id` and type is `Cancelled | Preempted | Failed`:
  - The scheduler logs that a local deployment has been terminated.
- There is currently **no** automatic restart or re‑bidding triggered here; higher‑level logic or future scheduler extensions are expected to handle retries.

---

## 4. Winner Selection Algorithm

**Function:** `select_winners(context: &BidContext, local_node_id: &str) -> Vec<BidOutcome>`

**Key steps (from source + tests):**

1. **Deduplicate per node, keeping best score**

   ```rust
   let mut best_by_node: HashMap<String, BidEntry> = HashMap::new();

   for bid in &context.bids {
       let entry = best_by_node
           .entry(bid.bidder_id.clone())
           .or_insert_with(|| bid.clone());

       if bid.score > entry.score {
           *entry = bid.clone();
       }
   }
   ```

   - If a node submits multiple Bids, only the highest score is kept.

2. **Sort descending by score**

   ```rust
   let mut bids: Vec<BidEntry> = best_by_node.into_values().collect();
   bids.sort_by(|a, b| {
       b.score
           .partial_cmp(&a.score)
           .unwrap_or(std::cmp::Ordering::Equal)
   });
   ```

3. **Select up to `replicas` distinct winners, with local‑node special rule**

   ```rust
   let mut selected_nodes = HashSet::new();
   let mut outcomes = Vec::new();

   for bid in bids.into_iter() {
       if outcomes.len() as u32 >= context.replicas {
           break;
       }

       // Enforce that the local node can never be selected for more than one replica
       if bid.bidder_id == local_node_id && selected_nodes.contains(local_node_id) {
           continue;
       }

       if selected_nodes.insert(bid.bidder_id.clone()) {
           outcomes.push(BidOutcome {
               bidder_id: bid.bidder_id,
               score: bid.score,
           });
       }
   }
   ```

**Invariants (enforced by tests):**

- For `replicas = N` and multiple unique bidders:
  - The winners vector **MUST** have length `<= N`.
  - Winners **MUST** be unique by `bidder_id`.
- The local node (`local_node_id`) **MUST NOT** appear more than once in `outcomes`, even if `replicas > 1` and multiple high‑scoring bids came from the local node.

From tests:

```rust
#[test]
fn selects_unique_winners_for_multiple_replicas() { /* ... */ }

#[test]
fn local_node_only_selected_once_even_with_multiple_replicas() { /* ... */ }
```

This implements the spec’s “distribute replicas across nodes” semantics: **a node deploys at most one replica of a given Tender**.

---

## 5. LeaseHint & Deployment Trigger (Winner Path)

Inside the selection‑window task in `handle_tender`, once winners are computed:

1. The `BidContext` is removed from `active_bids`.
2. If `winners` contains this node’s `local_id`:
   - Log: “WON BID for tender …”
   - **Step 1**: publish a LeaseHint to DHT.
   - **Step 2**: trigger `DeployWorkload` via `SchedulerCommand`.

### 5.1 LeaseHint publication

```rust
let lease_hint = machine::build_lease_hint(
    &tender_id_clone,
    &local_id,
    1.0,   // score (currently constant here)
    30000, // ttl_ms (note: currently 30s in code; spec suggests 3s)
    0,     // nonce
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64,
    &[], // signature
);

outbound_tx.send(SchedulerCommand::PutDHT {
    key: format!("lease:{}", tender_id_clone).into_bytes(),
    value: lease_hint,
})?;
```

**Implementation notes / gaps vs spec:**

- TTL is currently hardcoded to `30000` ms (30s); the normative spec suggests `3000` ms for LeaseHints.
- Signature is currently empty; future implementation **MUST** sign LeaseHints.

### 5.2 Deployment trigger

```rust
let manifest_id = manifest_id_opt.unwrap_or_else(|| tender_id_clone.clone());

outbound_tx.send(SchedulerCommand::DeployWorkload {
    manifest_id,
    manifest_json: manifest_json.clone(),
    replicas: 1,
})?;
```

- The scheduler **always** requests `replicas: 1` for the local deployment request, regardless of the global replicas requested by the Tender.
- Combined with the winner selection invariant, this ensures:
  - At most one replica per node.
  - Up to `replicas` distinct nodes (where `replicas = max(1, max_parallel_duplicates)`).

---

## 6. Runtime Integration Behavior (Scheduling‑Relevant Pieces)

The scheduler re‑exports `runtime_integration::*`. Key behaviors that matter for the scheduler’s semantics are:

### 6.1 Manifest Content Decoding

**Function:** `decode_manifest_content(manifest_json: &str) -> Vec<u8>`

Behavior (validated by tests):

- Attempt to base64‑decode `manifest_json` using `STANDARD` base64.
- If decoding succeeds **and** the decoded bytes are valid UTF‑8:
  - Return decoded bytes.
- If decoding fails or the decoded bytes are not valid UTF‑8:
  - Fall back to `manifest_json.as_bytes()`.

This lets upstream callers provide either:

- raw YAML/JSON strings, or
- base64‑encoded YAML/JSON content.

### 6.2 Manifest Replica Rewriting

**Function:** `modify_manifest_replicas(manifest_content: &[u8]) -> Result<Vec<u8>, Box<dyn Error>>`

Behavior:

- Interpret `manifest_content` as UTF‑8 and parse as `serde_yaml::Value`.
- If parsing as YAML succeeds:
  - If `.spec.replicas` exists:
    - Set it to `1`.
  - Else, if a top‑level `replicas` key exists:
    - Set that to `1`.
  - Serialize the updated document back to YAML bytes.
  - Log that the manifest was modified.
- If parsing fails:
  - Log a warning.
  - Return the original bytes unchanged.

This ensures that **each node** that wins for a Tender deploys **one replica**, regardless of the original manifest’s `replicas` field.

### 6.3 DeploymentConfig Defaults

`DeploymentConfig::default()` (as tested) has:

- `replicas == 1`
- `env.is_empty()`

`create_deployment_config(apply_req: &ApplyRequest)` uses:

- `config.replicas = apply_req.replicas`
- Adds `BEEMESH_OPERATION_ID` to `env` if `operation_id` is non‑empty.

For workloads scheduled via the scheduler (as opposed to direct apply), the driving code ensures that:

- Manifest replicas are forced to `1` via `modify_manifest_replicas`.
- The scheduler’s `DeployWorkload` uses `replicas: 1`.

### 6.4 Engine Selection

**Function:** `select_runtime_engine(manifest_content: &[u8]) -> Result<String, Box<dyn Error>>`

Key behavior:

- Parse manifest as YAML if possible.
- If `.metadata.annotations["beemesh.io/runtime-engine"]` exists:
  - Use that engine name.
- Otherwise:
  - Detect Kubernetes kinds (`Pod`, `Deployment`, …) or Compose (`services` + `version`), but these are hints only.
- Query `RUNTIME_REGISTRY` for available engines via `check_available_engines()`.
- Prefer engines in order:
  1. `"podman"` if available,
  2. `"docker"` if available,
  3. `"mock"` if available.
- If no engines are available:
  - Return error `"No suitable runtime engine available"`.

The scheduler itself is agnostic to the engine name; it only cares that deployment completes or fails, so it can log and participate in events.

---

## 7. End‑to‑End Replica Distribution Guarantees

Integration test: `machineplane/tests/integration_test_apply.rs::test_apply_nginx_with_replicas`

This test encodes the following effective guarantees:

1. A manifest with `replicas: 3` (e.g. `nginx_with_replicas.yml`) is applied.
2. After the scheduler and runtime run:
   - Exactly 3 nodes (`ports`) have a deployed workload for the Tender ID.
   - These 3 nodes are **all distinct**.
   - On each node, the deployed manifest content is consistent with the original manifest except for the expected replica handling.

The expectations explicitly state:

> 2. The deployed manifests on each node have `replicas: 1` (since the scheduler distributes single replicas).

and assert:

```rust
assert_eq!(
    nodes_with_deployed_workloads.len(),
    3,
    "Expected exactly 3 nodes to have workload deployed (replicas=3)..."
);

assert_eq!(
    sorted_nodes, expected_nodes,
    "Expected workloads to be deployed on all 3 nodes..."
);
```

Thus, the **implementation‑level scheduling spec** is:

- For a Tender whose associated manifest requests `replicas = N` and is applied into a fabric with at least `N` eligible nodes:
  - The scheduler **MUST** converge to `N` distinct winner nodes.
  - Each winner node **MUST** deploy exactly 1 replica.
  - Combined, this results in `N` replicas in the fabric, aligned with the high‑level Machineplane semantics: “distribute replicas across machines, not within a single machine.”

---

## 8. Notable Deviations / TODOs vs High‑Level Spec

Based on the implementation, the following are **known gaps** relative to the normative spec in `machineplane-spec.md`:

- **LeaseHint TTL**
  - Implementation uses `ttl_ms = 30000` (30s).
  - Spec suggests `lease_ttl_ms: 3000` (3s) in config.
  - TODO: align implementation with configurable TTL.

- **Message Signing**
  - Bids and LeaseHints currently use `&[]` for signatures.
  - Spec requires:
    - Ed25519 signatures,
    - replay protection with nonce and timestamp checks.
  - TODO: integrate real signing and verification into scheduler paths.

- **Capacity‑aware scoring**
  - `capacity_score` is currently a constant `1.0`.
  - Spec requires deterministic scoring including:
    - resource fit,
    - network locality,
    - historical reliability,
    - (optional) price/energy.
  - TODO: plug in `CapacityVerifier` and metrics.

- **Backoff and retries based on events**
  - The current `handle_event` only logs.
  - Spec allows nodes to re‑enter Claiming after LeaseHint expiry and `Event{Failed}`.
  - TODO: implement reactive behavior to Failed/Cancelled/Preempted events in scheduler.

Despite these gaps, the **core replica distribution and per‑node single‑replica invariants are enforced** by the current code (and tested).

---
