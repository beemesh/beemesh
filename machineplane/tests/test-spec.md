# Machineplane Test Specification

## Scope
This document describes the tests under `machineplane/tests` that validate the Machineplane daemon, apply workflow, and decentralized scheduler. It complements the normative behavior described in `machineplane-spec.md` by summarizing what each test covers, its dependencies, and how to run it.

```gherkin
Feature: CLI configuration from environment

  # This covers the contract that global configuration is taken from CONTAINER_HOST
  # when no explicit CLI override is provided.

  Scenario: CLI MUST derive container runtime endpoint from CONTAINER_HOST
    Given the process environment has CONTAINER_HOST set to a non-empty URI value
      And the CLI is invoked without any explicit option that selects a container runtime endpoint
    When the CLI parses its configuration from arguments and environment
    Then the resulting configuration MUST have its container runtime endpoint set to exactly the value of CONTAINER_HOST
      And configuration parsing MUST succeed without reporting any error
      And the CLI MUST NOT ignore or modify the CONTAINER_HOST value when populating the container runtime endpoint
```

---

```gherkin
Feature: Manifest name extraction

  # The "manifest name extraction" operation is treated as a black box that inspects
  # a Kubernetes-style manifest document and returns an optional name.

  Scenario: Name extraction MUST indicate absence for manifests without metadata.name
    Given a manifest document encoded as structured data
      And the document either lacks a "metadata" field
        Or the "metadata" field does not contain a "name" field
        Or the "metadata.name" field is present but empty or not a string
    When the manifest name is requested from this document
    Then the operation MUST indicate that the manifest has no name
      And the operation MUST NOT throw an error
      And the operation MUST NOT synthesize a fallback name from any other field

  Scenario: Name extraction SHOULD return metadata.name for valid manifests
    Given a manifest document encoded as structured data
      And the document contains a "metadata.name" field
      And the "metadata.name" field is a non-empty string
    When the manifest name is requested from this document
    Then the operation SHOULD return exactly the value of "metadata.name"
      And the operation MUST NOT modify, trim, or normalize the returned name
      And the operation MUST NOT fall back to any other field when "metadata.name" is present and valid
```

---

```gherkin
Feature: Manifest ID computation

  # "Manifest ID computation" is a pure function over manifest content
  # and, optionally, a version value. IDs are opaque strings from the caller’s
  # point of view; tests compare only equality/inequality.

  Scenario: Manifest ID MUST be stable for identical content
    Given two manifest contents that are byte-for-byte identical
    When a manifest ID is computed separately for each content value
    Then the two computed IDs MUST be exactly equal
      And repeating the computation for the same content any number of times MUST always yield the same ID
      And the computation MUST NOT depend on external state such as time, random values, or environment

  Scenario: Manifest ID MUST differ for distinct content
    Given two manifest contents that are not byte-for-byte identical
    When a manifest ID is computed for each content value
    Then the two computed IDs MUST NOT be equal
      And the computation MUST treat any difference in content as sufficient to produce a different ID

  Scenario: Manifest ID MUST distinguish revisions by version
    Given a manifest content that is otherwise identical across revisions
      And a first version identifier V1
      And a second version identifier V2 such that V2 is different from V1
    When a manifest ID is computed for the pair (content, V1)
      And a manifest ID is computed for the pair (content, V2)
    Then the ID for (content, V1) and the ID for (content, V2) MUST NOT be equal
      And each ID MUST uniquely identify the combination of content and version
      And changing only the version value while keeping content unchanged MUST be sufficient to change the ID
      And changing only the content while keeping the version unchanged MUST also be sufficient to change the ID
```

---

```gherkin
Feature: Host mesh formation and REST API

  # A "node" here is a running daemon instance that exposes an HTTP API
  # and participates in a peer-to-peer mesh.

  Background:
    Given a cluster of five nodes started from a clean state
      And each node exposes an HTTP health endpoint
      And each node exposes an HTTP endpoint that reports its currently known peers
      And all nodes are configured to form a single connected peer-to-peer mesh

  Scenario: Mesh formation MUST result in peer discovery across nodes
    Given the five nodes have been started as described in the Background
    When the health endpoint of each node is polled until all nodes report a healthy state or a global timeout is reached
      And, once all nodes report healthy, the peer-reporting endpoint of each node is polled within a bounded time window
    Then the set of peers reported across all nodes MUST show that each node knows about at least one other node
      And the total number of distinct peer relationships observed across the cluster MUST be consistent with a connected mesh of five nodes
      And no node’s peer-reporting endpoint MUST return an error while the node’s health endpoint reports a healthy state

  Scenario: REST endpoints MUST expose non-empty key material on a healthy node
    Given at least one node in the cluster reports a healthy state via its health endpoint
    When the health endpoint is queried on that node
      And the endpoint that returns its key encapsulation mechanism (KEM) public key is queried on that node
      And the endpoint that returns its signing public key is queried on that node
    Then the health endpoint response MUST indicate an overall "ok" or equivalent healthy status
      And the KEM public key endpoint MUST return a non-empty representation of a public key
      And the signing public key endpoint MUST return a non-empty representation of a public key
      And neither key endpoint MUST return an error while the health endpoint reports a healthy state
```

---

```gherkin
Feature: Apply workflow for single-replica workloads

  # The "apply API" is a REST-style interface that accepts manifest documents
  # describing workloads and reconciles cluster state accordingly.

  Background:
    Given a fabric of three nodes forming a connected mesh
      And each node exposes an apply API for submitting and deleting workload manifests
      And the fabric has reached a healthy, mesh-formed state as indicated by its health and mesh-observation APIs

  Scenario: Single-replica apply MUST place the workload on exactly one node
    Given a workload manifest that describes a single replica of a service
      And the fabric mesh is healthy as described in the Background
    When the manifest is submitted to the apply API
      And the system is allowed to reconcile until either a predefined timeout or stabilization is reached
      And the workload listing or equivalent introspection API is queried on each node
    Then exactly one node MUST report that the workload from this manifest is present and running
      And the other nodes MUST NOT report that this workload is present
      And the stored representation of the manifest on the node that hosts the workload MUST be byte-for-byte identical to the submitted manifest
      And no node MUST report any error related to this manifest once reconciliation has completed
```

---

```gherkin
Feature: Apply workflow with container runtime integration

  # Here we assume that at least one node is connected to a container runtime
  # (for example, a Podman-compatible daemon) and that the apply workflow
  # can materialize workloads as containers.

  Scenario: Container-backed apply MUST provision and tear down containers
    Given a fabric of nodes that has reached a healthy, mesh-formed state
      And at least one node has access to a container runtime capable of creating and removing containers
      And a workload manifest that describes a service to be run as one or more containers
    When the manifest is submitted to the apply API
      And the system is allowed to reconcile until either a predefined timeout or stabilization is reached
    Then the container runtime on one or more nodes MUST create container instances corresponding to the workload
      And each created container MUST be identifiable as belonging to this workload (for example by a deterministic naming or labeling scheme tied to the workload identity)
      And the workload listing or equivalent introspection API on the hosting nodes MUST report the workload as present and running

    When the same manifest is subsequently deleted via the apply API
      And the system is allowed to reconcile until either a predefined timeout or stabilization is reached
    Then all containers and associated runtime resources that were created solely for this workload MUST be removed by the container runtime
      And the workload listing or equivalent introspection API on all nodes MUST indicate that the workload is no longer present
      And the system MUST NOT leave behind any stale containers or workload records associated with the deleted manifest
```

---

```gherkin
Feature: Apply workflow for replicated workloads

  # This covers scheduling behavior for manifests that request more than one
  # replica of the same workload.

  Background:
    Given a fabric of three nodes forming a connected mesh
      And each node exposes an apply API and a workload introspection API
      And the fabric has reached a healthy, mesh-formed state as indicated by its health and mesh-observation APIs

  Scenario: Replica apply SHOULD distribute replicas across nodes
    Given a workload manifest that requests multiple identical replicas of the same service
      And the fabric is in the healthy state described in the Background
    When the manifest is submitted to the apply API
      And the system is allowed to reconcile until either a predefined timeout or stabilization is reached
      And the workload introspection API is queried on each node
    Then the total number of running replicas reported across all nodes MUST equal the replica count requested in the manifest
      And, under normal resource conditions, replicas SHOULD be distributed across more than one node
      And no node SHOULD host all replicas unless resource constraints or explicit scheduling rules require co-location
      And no node MUST report an error related to this manifest once reconciliation has completed
```

---

```gherkin
Feature: Scheduler – deterministic bidding and winner selection

  # These tests validate that the decentralized bidding process and award
  # selection behave according to the normative scheduling flow:
  # - at-least-once scheduling
  # - deterministic score computation
  # - deterministic winner selection given the same inputs
  # - respect of the configured selection window and jitter.

  Background:
    Given a stable fabric topic configured as "beemesh-fabric"
      And a selection window and jitter configured in node configuration
      And a set of N ≥ 3 nodes subscribed to the "beemesh-fabric" topic
      And each node runs the Machineplane scheduler with identical protocol version

  Scenario: Nodes MUST ignore Tenders on the wrong topic
    Given a running node subscribed to the "beemesh-fabric" topic
      And a Tender message published on a different topic
    When the scheduler receives that message
    Then the node MUST NOT treat it as a valid Tender
      And the node MUST NOT emit any Bid in response to that message

  Scenario: Ineligible nodes MUST NOT bid
    Given a Tender T with explicit resource and policy requirements
      And a node whose current resources or policies do not satisfy T
    When the node receives T on the "beemesh-fabric" topic
    Then the node MUST locally evaluate its eligibility
      And if it is ineligible it MUST NOT submit any Bid for T
      And it MUST NOT partially bid or bid with downgraded requirements

  Scenario: Eligible nodes MUST compute deterministic scores
    Given a Tender T with fixed resource requirements and QoS hints
      And a node whose local metrics (resource availability, locality, history) are fixed for the duration of this test
    When the node receives T and decides it is eligible
      And the scheduler computes a score for T
    Then the score computed for T MUST be the same when recomputed with the same Tender and the same local metrics
      And the score computation MUST NOT depend on wall clock time beyond the T timestamp and configured skew limits
      And the score computation MUST NOT depend on random values

  Scenario: Nodes SHOULD submit at most one Bid per Tender within the selection window
    Given a node that has received Tender T at time ts_recv
      And the global selection window and jitter are configured in the node
    When the node decides it is eligible for T
    Then the node SHOULD emit at most one Bid for T
      And the Bid SHOULD be emitted within selection_window_ms ± selection_jitter_ms of ts_recv
      And the node MUST NOT emit Bids for T after the selection window has closed

  Scenario: Tender owner MUST deterministically select winners
    Given a set of K ≥ 3 Bids received for Tender T within the selection window
      And all Bids have valid signatures and non-duplicate node IDs
      And a well-defined deterministic ordering of Bids (for example by score, then latency, then node_id)
      And a requested replica count R such that 1 ≤ R ≤ K
    When the tender owner closes the selection window for T
      And it runs its winner selection procedure over the K Bids
    Then the same ordered list of winners MUST be produced every time the procedure is run with the same Bid set
      And exactly R winners MUST be selected
      And if two different tender owners are given the same ordered Bid set and configuration
        Then they MUST both produce exactly the same winner list for T

  Scenario: Manifest MUST remain with the publisher until winners are chosen
    Given a producer that owns the manifest M for Tender T
      And a set of eligible nodes that have received T
    When the producer publishes T on the "beemesh-fabric" topic
    Then the producer MUST include only the manifest digest and NOT the full manifest payload
      And no node other than the producer MUST receive the full manifest M before the winners are selected
      And the full manifest M MUST be transferred only to awarded nodes after the Award is decided

  Scenario: Award MUST be visible on the fabric topic
    Given a producer that has selected winners for Tender T
      And a fabric topic configured as "beemesh-fabric"
    When the producer publishes the Award for T
    Then the Award message MUST be sent on the "beemesh-fabric" topic
      And all subscribed nodes MUST be able to observe the Award
      And nodes that were not selected as winners MUST NOT proceed to deploy T
```

---

```gherkin
Feature: Scheduler – deployment, events, and backoff

  # These tests validate behavior after an Award:
  # - manifest transfer to winners only
  # - deployment success and failure events
  # - backoff and retry behavior on missing or failed deployments.

  Background:
    Given a Tender T with a single winner node W
      And W has received the Award for T
      And the producer holds the manifest M referenced by T
      And all nodes share the same deploy_timeout_ms and backoff policy

  Scenario: Manifest MUST be transferred only to awarded nodes
    Given the Award for T lists node W as the only winner
      And another node U is not in the winner set
    When the producer transfers the manifest for T
    Then node W MUST receive the manifest M via a mutually authenticated stream
      And node U MUST NOT receive M
      And no other node MUST receive M unless added explicitly to the Award winner set

  Scenario: Successful deployment MUST emit a Deployed event
    Given node W has received the manifest M for T
      And W is able to start the workload described by M
    When W completes deployment of the workload for T
    Then W MUST publish an Event{Deployed} for T on the fabric
      And the event MUST include correlation data sufficient to match it to T
      And other nodes observing Event{Deployed} MUST treat T as successfully deployed

  Scenario: Failed deployment MUST emit a Failed event
    Given node W has received the manifest M for T
      And W encounters a non-recoverable error while deploying the workload for T
    When W determines that deployment cannot succeed
    Then W MUST publish an Event{Failed} for T on the fabric
      And the event MUST include correlation data sufficient to match it to T
      And W MUST release any reserved resources for T

  Scenario: Deployment timeout MUST trigger failure and allow re-evaluation
    Given node W is attempting to deploy T
      And deploy_timeout_ms is configured
    When deployment does not complete within deploy_timeout_ms from the time W started deploying T
    Then W MUST publish Event{Failed} for T
      And all nodes observing the fabric MAY treat T as eligible for re-evaluation
      And subsequent Tenders for the same manifest or workload identity MAY be evaluated again according to local policy

  Scenario: Nodes MUST back off before re-evaluating after failure
    Given nodes observe that T has failed to deploy (for example via Event{Failed} or absence of Event{Deployed} within deploy_timeout_ms)
      And a backoff policy with jitter is configured locally for each node
    When nodes consider re-evaluating T
    Then each node MUST wait at least the configured minimum backoff interval before bidding on a new Tender for the same workload
      And nodes SHOULD add jitter to avoid synchronized retries
      And nodes MUST NOT enter a tight retry loop for T
```

---

```gherkin
Feature: Scheduler – partitions and conflicting awards

  # These tests validate behavior when the scheduling fabric experiences
  # partitions or conflicting Award messages for the same Tender.

  Background:
    Given a Tender T that is published on the "beemesh-fabric" topic
      And a network partition that splits the fabric into two disjoint groups of nodes
      And each partition can independently observe T and issue an Award for T

  Scenario: Partition MAY cause conflicting Awards
    Given partition A and partition B both observe T
      And a tender owner in partition A selects winners and publishes Award_A for T
      And a tender owner in partition B selects a different winner set and publishes Award_B for T
    When the network remains partitioned
    Then nodes in partition A MAY act according to Award_A
      And nodes in partition B MAY act according to Award_B
      And more than one deployment of the same workload MAY temporarily exist in the overall fabric

  Scenario: After healing, nodes MUST converge on the freshest valid Award
    Given the network partition heals
      And nodes can now observe both Award_A and Award_B for T
      And each Award carries a version or timestamp field that can be totally ordered
    When nodes reconcile the Award history for T
    Then all nodes MUST converge on the Award that is considered freshest and valid according to the ordering rule
      And nodes whose current deployment does not match the freshest Award MUST treat their deployment as stale

  Scenario: Nodes SHOULD drain deployments that do not match the freshest Award
    Given nodes have converged on a single freshest Award_F for T
      And some nodes are running deployments that correspond to a losing Award
    When those nodes detect that their local deployment does not match Award_F
    Then they SHOULD gracefully drain and stop those deployments
      And they SHOULD release resources associated with the losing deployment
      And they MUST honor any Workplane leader or consistency signals when shutting down stateful workloads
```

---

```gherkin
Feature: Scheduler – at-least-once semantics and interaction with Workplane

  # These tests validate that the scheduler provides at-least-once semantics
  # and delegates strict leadership and write-safety guarantees to the Workplane.

  Background:
    Given a workload that is stateful from the Workplane’s point of view
      And a deployment policy that allows the Machineplane to temporarily start more than one replica
      And a Workplane deployment that enforces leader election and minority write refusal

  Scenario: Scheduler MAY start more than one replica transiently
    Given a Tender T for a stateful workload W
      And due to timing, retries, or partitions multiple nodes are awarded T or its equivalent
    When the Machineplane scheduler starts deployments on more than one node
    Then it MAY run more than one replica of W at the same time
      And it MUST continue to emit events that allow the Workplane to reason about which deployments are active

  Scenario: Workplane MUST gate writes despite at-least-once scheduling
    Given multiple replicas of stateful workload W are running due to Machineplane at-least-once behavior
      And the Workplane has elected a single leader for W
    When client write traffic is routed to instances of W
    Then the Workplane MUST only allow writes to the elected leader
      And non-leader replicas MUST reject or buffer writes according to Workplane rules
      And Machineplane scheduler behavior MUST NOT bypass these write-safety guarantees
```
