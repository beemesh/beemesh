# Machineplane Test Specification

## Scope
This document describes the tests under `machineplane/tests` that validate the Machineplane daemon and apply workflow. It complements the normative behavior described in `machineplane-spec.md` by summarizing what each test covers, its dependencies, and how to run it.

## How to run the suites
- Fast unit-style tests (CLI env parsing, manifest utilities):
  - `cargo test -p machineplane cli_env_test`
  - `cargo test -p machineplane error_handling_test`
- Full host flow integration test (5 nodes, REST + libp2p formation):
  - `cargo test -p machineplane --test integration_test -- --ignored`
  - Marked `#[ignore]` because it requires multiple local ports (3000/3100/3200/3300/3400 REST; 4001–4005 QUIC) and a working libp2p environment.
- Apply workflow tests (exercise REST, libp2p mesh, and optional Podman runtime):
  - `cargo test -p machineplane integration_test_apply`
  - These tests auto-skip when Podman is unavailable; set `PODMAN_CMD`/`PODMAN_HOST` to point at a reachable Podman daemon.

## Test suites as Gherkin scenarios

### CLI environment parsing (`tests/cli_env_test.rs`)
Scenario: CLI MUST map `PODMAN_HOST` to `podman_socket` (tests/cli_env_test.rs)
  - Given a temporary environment where `PODMAN_HOST` is set
  - When the CLI arguments are constructed
  - Then the parsed configuration MUST include `podman_socket` populated from `PODMAN_HOST`
  - And the temporary environment MUST be cleaned up after the test

### Manifest parsing and IDs (`tests/error_handling_test.rs`)
Scenario: Manifest name extraction MUST handle incomplete JSON (tests/error_handling_test.rs)
  - Given a manifest missing required fields
  - When `extract_manifest_name` parses the document
  - Then it MUST return `None`

Scenario: Manifest name SHOULD be extracted from valid JSON (tests/error_handling_test.rs)
  - Given a manifest containing a valid Kubernetes `metadata.name`
  - When `extract_manifest_name` parses the document
  - Then it SHOULD return the manifest name

Scenario: Manifest ID MUST be stable for identical content (tests/error_handling_test.rs)
  - Given two identical manifest contents
  - When `compute_manifest_id_from_content` hashes the bytes
  - Then the resulting IDs MUST match

Scenario: Manifest ID MUST differ for distinct content (tests/error_handling_test.rs)
  - Given two different manifest contents
  - When `compute_manifest_id_from_content` hashes each
  - Then the resulting IDs MUST NOT match

Scenario: Manifest ID version suffix MUST distinguish updates (tests/error_handling_test.rs)
  - Given a manifest content and an incremented version
  - When `compute_manifest_id` is called
  - Then the resulting ID MUST include the version suffix to differentiate revisions

### Host application flow (`tests/integration_test.rs`)
Background: Five nodes SHOULD be started in-process using a bootstrap node to form the mesh.

Scenario: Mesh formation MUST discover peers (tests/integration_test.rs)
  - Given the nodes are running
  - When `/debug/peers` is polled until timeout
  - Then at least four peers MUST be discovered

Scenario: REST endpoints MUST expose non-empty keys (tests/integration_test.rs)
  - Given the nodes are running
  - When the REST health, `kem_pubkey`, and `signing_pubkey` endpoints are queried
  - Then the health check MUST succeed
  - And both key endpoints MUST return non-empty values

Teardown: Nodes SHOULD be shut down after validation. The suite is `#[ignore]` by default because it requires REST ports 3000/3100/3200/3300/3400 and QUIC ports 4001–4005 plus a working libp2p environment.

### Apply workflow (`tests/integration_test_apply.rs` + helpers)
Background: A three-node fabric SHOULD be started with `start_fabric_nodes` from `tests/apply_common.rs` / `tests/runtime_helpers.rs` using REST ports 3000/3100/3200 and QUIC ports 4001/4002/4003.

Scenario: Single replica apply MUST land on exactly one node (tests/integration_test_apply.rs)
  - Given the fabric is healthy and using `tests/sample_manifests/nginx.yml`
  - When `test_apply_functionality` applies the manifest via REST
  - Then the mesh formation MUST complete before verification
  - And exactly one node MUST report the workload
  - And the deployed manifest MUST match the submitted content

Scenario: Podman-backed apply MUST provision and clean up containers (tests/integration_test_apply.rs)
  - Given Podman is available through `PODMAN_CMD` or `PODMAN_HOST`
  - And the fabric is healthy and mesh-ready
  - When `test_apply_with_real_podman` applies the manifest
  - Then Podman MUST create containers named `beemesh-{tender_id}-pod` (or equivalent)
  - And deleting the manifest MUST tear down those Podman resources
  - And any leftover Podman artifacts MUST be cleaned up

Scenario: Replica apply SHOULD distribute across nodes (tests/integration_test_apply.rs)
  - Given the fabric is healthy and using `tests/sample_manifests/nginx_with_replicas.yml`
  - When `test_apply_nginx_with_replicas` applies the manifest
  - Then the mesh formation MUST complete before verification
  - And the replicas SHOULD be distributed across nodes (allowing partial spread in constrained environments)

Environment controls
- `PODMAN_CMD` / `PODMAN_HOST` MAY be set to select a Podman binary/socket.
- Timeouts MAY be overridden: `BEEMESH_APPLY_MESH_TIMEOUT_SECS`, `BEEMESH_APPLY_DELIVERY_TIMEOUT_SECS`, `BEEMESH_APPLY_REPLICA_TIMEOUT_SECS`, `BEEMESH_PODMAN_HEALTH_TIMEOUT_SECS`, `BEEMESH_PODMAN_MESH_TIMEOUT_SECS`, `BEEMESH_PODMAN_DELIVERY_TIMEOUT_SECS`, `BEEMESH_PODMAN_VERIFY_TIMEOUT_SECS`, `BEEMESH_PODMAN_TEARDOWN_TIMEOUT_SECS`.

## Helper modules and fixtures
- `tests/runtime_helpers.rs` builds ephemeral `DaemonConfig` instances, spawns nodes, and polls for local peer IDs/multiaddrs.
- `tests/apply_common.rs` and `tests/kube_helpers.rs` provide shared apply utilities (mesh formation checks, Kubernetes-style apply/delete calls, workload verification logic).
- `tests/sample_manifests/` contains nginx manifests used by the apply tests.
