# 4. Scheduler (Implementation-Specific Spec)

This section describes the *current Rust implementation* of the Machineplane scheduler in `machineplane/src/scheduler.rs`. It refines the normative “ephemeral, duplicate-tolerant” scheduling model with concrete data structures, flows, and behaviors.

The scheduler is local to each node and runs in **pull** mode: it subscribes to the scheduler topics, evaluates Tenders, submits Bids, and if selected as a winner, deploys workloads via the runtime integration module.

## 4.x Scheduler Data Structures

### 4.x.1 Scheduler State

```rust
pub struct Scheduler {
    /// Local node ID (PeerId string)
    local_node_id: String,
    /// Active bids we are tracking: TenderID -> BidContext
    active_bids: Arc<Mutex<HashMap<String, BidContext>>>,
    /// Channel to send messages back to the network loop
    outbound_tx: mpsc::UnboundedSender<SchedulerCommand>,
}
```

* `local_node_id` **MUST** be the libp2p `PeerId` of the local node (stringified).
* `active_bids` tracks all Tenders for which this node has entered the bidding process and not yet fully resolved (i.e., not yet finalized selection / timeout / cleanup).
* `outbound_tx` is a Tokio `mpsc::UnboundedSender<SchedulerCommand>` used to instruct the libp2p network loop to publish messages (`Publish`), perform DHT operations (`PutDHT`), or trigger deployments (`DeployWorkload`).

### 4.x.2 BidContext and BidEntry

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
```

* A `BidContext` represents one active Tender:
  * `tender_id`: globally unique Tender identifier (string from Flatbuffer `Tender.id`).
  * `manifest_id`: identifier for the workload manifest associated with the tender. In the current implementation this is set to `tender.id` (i.e., same as `tender_id`).
  * `replicas`: the number of winners required for this tender. In the current implementation this is derived as:
    * `replicas = max(1, tender.max_parallel_duplicates)`
    * That is, the implementation **forces a minimum of 1** replica and uses `max_parallel_duplicates` as the effective replica count.
  * `bids`: a list of `BidEntry` values collected during the selection window. Each `BidEntry` contains:
    * `bidder_id`: the node’s `PeerId` string.
    * `score`: a floating-point bid score.

### 4.x.3 SchedulerCommand

```rust
#[derive(Debug, Clone)]
pub enum SchedulerCommand {
    Publish {
        topic: String,
        payload: Vec<u8>,
        // (implementation includes additional internal fields such as optional DHT key, etc.)
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

* `Publish`:
  * Used to publish to Gossipsub topics (`SCHEDULER_TENDERS`, `SCHEDULER_PROPOSALS`, `SCHEDULER_EVENTS`, or others).
  * `payload` is a serialized Flatbuffers message (Tender, Proposal, Event, etc.).
* `PutDHT`:
  * Used to put scheduler- or workload-related metadata into the Kademlia DHT.
* `DeployWorkload`:
  * Used to request the runtime integration module to deploy a workload locally.
  * `manifest_id`: workload manifest identifier (same as `BidContext.manifest_id`).
  * `manifest_json`: the manifest in JSON/YAML-compatible string form; the runtime module will further process this.
  * `replicas`: the (local) replica count. For the machineplane scheduler implementation, **this is always 1** per node (see §4.x.7).

The network loop is responsible for consuming `SchedulerCommand` messages and mapping them onto libp2p actions and runtime calls.

---

## 4.x Scheduler Construction

```rust
impl Scheduler {
    pub fn new(
        local_node_id: String,
        outbound_tx: mpsc::UnboundedSender<SchedulerCommand>,
    ) -> Self {
        Self {
            local_node_id,
            active_bids: Arc::new(Mutex::new(HashMap::new())),
            outbound_tx,
        }
    }
}
```

* Callers **MUST** provide the local `PeerId` and an `outbound_tx` channel.
* `active_bids` is created empty and **MUST** be populated only via scheduler methods (Tender/Proposal handlers).

---

## 4.x Tender Handling and Bid Submission

### 4.x.1 Tender Receive Path

```rust
impl Scheduler {
    /// Process a Tender message: Evaluate -> Bid
    async fn handle_tender(&self, message: &gossipsub::Message) {
        match machine::root_as_tender(&message.data) {
            Ok(tender) => {
                let tender_id = tender.id.clone();
                info!("Received Tender: {}", tender_id);

                let manifest_id = tender.id.clone();

                let replicas = std::cmp::max(1, tender.max_parallel_duplicates);

                // 1. Evaluate Fit (Capacity Check)
                // TODO: Connect to CapacityVerifier
                let capacity_score = 1.0; // Placeholder: assume perfect fit for now

                // 2. Calculate Bid Score
                // Score = (ResourceFit * 0.4) + (NetworkLocality * 0.3) + (Reputation * 0.3)
                // For now, just use random or static for testing
                let my_score = capacity_score * 0.8 + 0.2; // Simple formula

                // ... create / update BidContext, emit Proposal, etc.
            }
            Err(err) => {
                error!("Failed to decode Tender: {:?}", err);
            }
        }
    }
}
```

**Observed behavior:**

1. **Deserialization**  
   * The scheduler **MUST** attempt to parse `message.data` as a `Tender` using `machine::root_as_tender`.
   * On error, the scheduler **MUST** log an error and **MUST NOT** proceed with bidding.

2. **Tender identity and manifest mapping**
   * `tender_id` and `manifest_id` are set to `tender.id`.
   * Current implementation assumes a 1:1 mapping between `Tender` and manifest ID via this single ID string.

3. **Replica count derivation**
   * `replicas` **MUST** be computed as:
     ```rust
     let replicas = std::cmp::max(1, tender.max_parallel_duplicates);
     ```
   * If `max_parallel_duplicates` is 0 or negative, the effective `replicas` is 1.

4. **Capacity evaluation and score**
   * Current implementation sets:
     ```rust
     let capacity_score = 1.0;
     let my_score = capacity_score * 0.8 + 0.2;
     ```
   * This is a placeholder: future implementations **MAY** plug in real capacity, locality, and reputation scoring, but **MUST** still produce a `score: f64` for `BidEntry`.

5. **BidContext creation / update**
   * For a new Tender:
     * A new `BidContext` **MUST** be created with:
       * `tender_id`
       * `manifest_id`
       * `replicas`
       * empty `bids` list initially, then eventually populated.
   * For an existing Tender:
     * The same `BidContext` is re-used and **MUST** be updated with any newly received bids.

6. **Bid submission**
   * The scheduler **MUST** publish a proposal/bid to `SCHEDULER_PROPOSALS` via `SchedulerCommand::Publish`.
   * The payload **MUST** contain the local node ID and `my_score`.

---

## 4.x Winner Selection

### 4.x.1 Selection Function

```rust
fn select_winners(context: &BidContext, local_node_id: &str) -> Vec<BidOutcome> {
    // Implementation at scheduler.rs: ... (hidden in snippet)
}
```

* `select_winners`:
  * Takes a `BidContext` and `local_node_id`.
  * Returns a **vector of winners** (`BidOutcome`), honoring:
    * `context.replicas` as the maximum number of winners.
    * **Uniqueness of winners**: each winner corresponds to a distinct bidder.
    * **Special handling for the local node**: the local node is considered only once in the winners list even if there are multiple replicas.

From the tests:

```rust
#[test]
fn selects_unique_winners_for_multiple_replicas() {
    /*...*/
}

#[test]
fn local_node_only_selected_once_even_with_multiple_replicas() {
    /*...*/
}
```

The tests enforce:

1. **Unique winners per Tender**
   * When `replicas > 1` and multiple bids are present, the selected winners **MUST** all have distinct `bidder_id`.
   * There **MUST NOT** be duplicates in the returned `Vec<BidOutcome>` for the same `tender_id`.

2. **Local node uniqueness**
   * Even if the Tender requires multiple replicas, the local node (`local_node_id`) **MUST** appear at most once in the winners list.
   * This enforces the invariant that a single node deploys at most one replica for a given Tender.

3. **Sorting / prioritization**
   * Although not fully exposed in the snippets, the implementation **MUST** effectively:
     * Sort bids descending by `score`.
     * Choose the top `replicas` bidders, applying the uniqueness and local-node-only-once rules above.

`BidOutcome` (not fully shown) encapsulates at least the selected `bidder_id` and the decision for whether the local node should deploy.

---

## 4.x Runtime Integration and Deployment

The runtime integration module is private to the scheduler, but re-exported for use:

```rust
// ---------------------------------------------------------------------------
pub use runtime_integration::*;

// ---------------------------------------------------------------------------
// Runtime integration and apply handling (merged from run.rs)
// ---------------------------------------------------------------------------

mod runtime_integration {
    //! Workload Integration Module
    // ...
}
```

### 4.x.1 Single-Replica Manifest Adaptation

The runtime module includes:

```rust
/// Modify the manifest to set replicas=1 for single-node deployment
/// Each node in the fabric will deploy one replica of the workload
fn modify_manifest_replicas(
    manifest_content: &[u8],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let manifest_str = String::from_utf8_lossy(manifest_content);

    // Try to parse as YAML/JSON and modify replicas field
    if let Ok(mut doc) = serde_yaml::from_str::<serde_yaml::Value>(&manifest_str) {
        // Check for spec.replicas field (Kubernetes-style)
        if let Some(spec) = doc.get_mut("spec") {
            if let Some(spec_map) = spec.as_mapping_mut() {
                spec_map.insert(
                    serde_yaml::Value::String("replicas".to_string()),
                    serde_yaml::Value::Number(serde_yaml::Number::from(1)),
                );
            }
        }
        // Check for top-level replicas field
        // ...
    }
    // ...
}
```

**Behavior:**

* Before deployment on a given node, the manifest content **MUST** be transformed such that:
  * `spec.replicas` (Kubernetes-style) is set to `1`, **if present**.
  * If `spec.replicas` is not present but a top-level `replicas` field exists, that field **MUST** be set to `1`.
* If parsing fails (non-YAML/JSON manifest), the function **MAY** return an error; the caller **MUST** handle it as deployment failure.

This is validated by:

```rust
#[test]
fn test_deployment_config_creation() {
    // This would require creating a mock ApplyRequest
    let config = DeploymentConfig::default();
    assert_eq!(config.replicas, 1);
    assert!(config.env.is_empty());
}
```

and the integration test:

```rust
/// 2. The deployed manifests on each node have `replicas: 1` (since the scheduler distributes single replicas).
#[serial]
#[tokio::test]
async fn test_apply_nginx_with_replicas() {
    // ...
}
```

Therefore:

* **Normative behavior for the scheduler + runtime integration:**
  * If a manifest is applied with `replicas: N` (e.g. `3`), and the Tender is scheduled across `N` nodes:
    * Each node receives a manifest whose replica count is **forced to 1**.
    * Exactly `N` distinct nodes are selected (subject to availability and bidding), each deploying a single replica.

### 4.x.2 DeploymentConfig Defaults

`DeploymentConfig::default()` must yield:

* `replicas == 1`
* `env` is empty (`env.is_empty()`)

The runtime module **MUST** interpret omitted replica counts as 1 and not attempt to deploy multiple replicas on a single node unless explicitly configured otherwise.

---

## 4.x Manifest Content Decoding

A helper in the scheduler / runtime path handles manifest content that may be either base64-encoded or plain text.

From tests:

```rust
#[test]
fn decode_manifest_content_handles_base64_and_plain() {
    /*...*/
}
```

**Required behavior:**

* If the manifest content is valid base64, the function **MUST** decode it and use the decoded bytes as the manifest.
* If decoding as base64 fails:
  * The implementation **MUST** treat the original bytes as plain text manifest content.
* This ensures that both:
  * `content` fields encoded as base64, and
  * raw YAML/JSON strings
  are accepted by the scheduler/runtimes.

---

## 4.x Integration with Apply Flow

The `machineplane/tests/integration_test_apply.rs` defines how the scheduler behaves end-to-end.

Example:

```rust
/// Tests the apply functionality with multiple replicas.
///
/// This test verifies that:
/// 1. A manifest specifying `replicas: 3` is distributed to 3 different nodes.
/// 2. The deployed manifests on each node have `replicas: 1` (since the scheduler distributes single replicas).
#[serial]
#[tokio::test]
async fn test_apply_nginx_with_replicas() {
    // ...
    // Read the original manifest content for verification
    let original_content = tokio::fs::read_to_string(manifest_path.clone())
        .await
        .expect("Failed to read nginx_with_replicas manifest file for verification");

    let tender_id = apply_manifest_via_kube_api(&client, ports[0], &manifest_path)
        .await
        .expect("kubectl apply should succeed for nginx_with_replicas");

    // Wait for direct delivery and deployment to complete
    sleep(Duration::from_secs(5)).await;

    // Get peer IDs and check workload deployment
    let port_to_peer_id = get_peer_ids(&client, &ports).await;
    // ...
}
```

and:

```rust
// Verify that all 3 nodes are different (should be all available nodes)
let mut sorted_nodes = nodes_with_deployed_workloads.clone();
sorted_nodes.sort();
let mut expected_nodes = ports.clone();
expected_nodes.sort();
assert_eq!(
    sorted_nodes, expected_nodes,
    "Expected workloads to be deployed on all 3 nodes {:?}, but found on nodes {:?}",
    expected_nodes, sorted_nodes
);
```

**Implications:**

1. **Global distribution of replicas**
   * If a manifest specifies `replicas: N` and there are at least `N` nodes:
     * The scheduler **MUST** arrange so that exactly `N` distinct nodes deploy the workload (assuming they all bid and are eligible).
   * On each node, the manifest as deployed **MUST** have `replicas: 1`.

2. **Uniqueness of scheduling**
   * The scheduler’s winner-selection and runtime integration **MUST** be consistent with:
     * Each selected node gets at most one replica for the Tender.
     * The sum of nodes with successful deployment == requested `replicas`.

3. **Direct delivery and timeout**
   * The tests allow up to ~5 seconds for deployment after applying via Kubernetes API, which in practice covers:
     * Tender creation and gossipsub broadcast.
     * Bidding and winner selection.
     * Local deployment through the runtime.

The Gherkin spec fragment:

```gherkin
When deployment does not complete within deploy_timeout_ms
Then the node MUST publish Event{Failed}
And nodes MAY re-enter Claiming for T after LeaseHint expiry
```

maps to the implementation:
* Nodes **MUST** enforce a deployment timeout (currently 10 seconds by default via config: `deploy_timeout_ms: 10000`).
* On timeout, they **MUST** publish `Event{Failed}` to `SCHEDULER_EVENTS` and **MAY** re-enter claiming/bidding after the lease TTL + skew window.

---

## 4.x Runtime Registry and Engine Selection

Within `runtime_integration`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_runtime_registry_initialization() {
        /*...*/
    }

    #[tokio::test]
    async fn test_runtime_engine_selection() {
        /*...*/
    }

    #[test]
    fn test_deployment_config_creation() {
        // This would require creating a mock ApplyRequest
        let config = DeploymentConfig::default();
        assert_eq!(config.replicas, 1);
        assert!(config.env.is_empty());
    }
}
```

**Contract:**

* The runtime registry **MUST** be initializable without external state and **MUST** be capable of selecting a runtime engine based on configuration (e.g., Podman vs future runtimes).
* The selected runtime engine **MUST** honor:
  * Single-replica deployment per node for each Tender.
  * Correct decoding of manifest content (base64 or plain).
  * Adjusted `replicas=1` semantics.

---

## 4.x Summary of Invariants Enforced by the Implementation

The current scheduler implementation enforces the following invariants:

1. **Unique winners for a Tender**
   * For each `tender_id`, at most one `BidOutcome` per `bidder_id`.

2. **At most one replica per node per Tender**
   * Even if `replicas > 1`, a node (including the local node) is selected at most once and deploys at most one replica.

3. **Global replica distribution**
   * For a manifest specifying `replicas: N`:
     * The scheduler aims to distribute these as `N` **single-replica** deployments across `N` distinct nodes.

4. **Single-replica manifest per node**
   * Deployed manifests on each node **MUST** have `replicas: 1` (via `modify_manifest_replicas` and `DeploymentConfig::default()`).

5. **Robust manifest decoding**
   * Manifests **MAY** be base64-encoded or plain text; decoding logic **MUST** handle both.

6. **Timeout and failure events**
   * If deployment does not finish within `deploy_timeout_ms`:
     * The node **MUST** emit `Event{Failed}`.
     * Nodes **MAY** re-claim the Tender after lease expiry.

These invariants align the implementation with the high-level normative spec (ephemeral, duplicate-tolerant scheduling) and make behavior testable and predictable.

---
