# Machineplane Test Specification

## Scope
This document describes the tests under `machineplane/tests` that validate the Machineplane daemon and apply workflow. It complements the normative behavior described in `machineplane-spec.md` by summarizing what each test covers, its dependencies, and how to run it.

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
