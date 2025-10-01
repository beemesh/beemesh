# 🔍 Architecture Violations Analysis Report

Based on my analysis of the Beemesh machineplane codebase against the normative specification in `README.md`, I've identified several significant architecture violations and missing implementations.

## 🔴 Critical Violations (RFC 2119 MUST Requirements)

### 1. Missing Pub/Sub Topic Architecture
**Violation**: The specification requires three distinct pub/sub topics:
- `scheduler-tasks` (task publication) 
- `scheduler-proposals` (bid publication)
- `scheduler-events` (confirmations: deployed, failed, preempted)

**Current Implementation**: Only uses a single generic topic `BEEMESH_CLUSTER = "beemesh-cluster"` defined in `/root/git/beemesh/machineplane/protocol/src/libp2p_constants.rs`

**Impact**: ⚡ High - Core scheduling algorithm cannot function without proper topic separation

**Files Affected**: 
- `machine/src/libp2p_beemesh/mod.rs` (uses single topic)
- `protocol/src/libp2p_constants.rs` (missing topic definitions)

**Required Changes**:
```rust
// In protocol/src/libp2p_constants.rs
pub const SCHEDULER_TASKS_TOPIC: &str = "scheduler-tasks";
pub const SCHEDULER_PROPOSALS_TOPIC: &str = "scheduler-proposals";  
pub const SCHEDULER_EVENTS_TOPIC: &str = "scheduler-events";
```

### 2. Missing Task, Bid, Event, and Lease Message Schemas
**Violation**: The specification defines specific JSON message schemas but implementation only has FlatBuffer schemas for ApplyRequest/Response and capacity queries.

**Current Implementation**: Missing FlatBuffer schemas for:
- Task (with task_id, manifest_ref, reqs, qos, affinity, hints, nonce, ts, sig)
- Bid (with task_id, node_id, score, fit, reasons, capabilities, nonce, ts, sig) 
- Event (with task_id, node_id, status, details, ts, sig)
- Lease (with task_id, winner_id, expires_at, version, sig)

**Files Missing**: 
- `protocol/schema/machine/task.fbs`
- `protocol/schema/machine/bid.fbs` 
- `protocol/schema/machine/event.fbs`
- `protocol/schema/machine/lease.fbs`

### 3. Missing ULID Task ID Implementation
**Violation**: Specification requires ULID strings for globally unique task IDs but no ULID dependency or implementation exists.

**Current Implementation**: Uses UUID in capacity requests but no ULID anywhere.

### 4. Missing Message Signing and Verification 
**Violation**: **MUST** sign all messages with Machine Peer ID keys (Ed25519) and **MUST** verify signatures.

**Current Implementation**: 
- CLI has ML-DSA-65 signing but machines don't sign pub/sub messages
- No signature verification in gossipsub message handlers
- Crypto crate is empty (`crypto/src/lib.rs` is empty)

**Files Needing Updates**:
- `machine/src/libp2p_beemesh/behaviour/gossipsub_message.rs` (no signature verification)
- `crypto/src/lib.rs` (empty - should contain signing helpers)

### 5. Missing Ephemeral Scheduling Algorithm
**Violation**: Complex scheduling workflow with bidding, lease-based election, and CAS operations is completely missing.

**Current Implementation**: Simple capacity request/response pattern, no:
- Task evaluation against requirements
- Bidding with scoring algorithm  
- Lease-based winner election with CAS operations
- Selection windows (250ms ± 100ms jitter)
- Lease renewal every 1s
- Backup and retry logic

### 6. Missing DHT Lease Management
**Violation**: **MUST** use CAS operations on DHT for lease records with TTL=3s.

**Current Implementation**: DHT exists but no lease management, no CAS operations for winner election.

## 🟡 Moderate Violations

### 7. Missing Configuration Surface
**Violation**: Specification defines YAML configuration but no configuration loading exists.

**Current Implementation**: Hardcoded values in code instead of configurable YAML structure defined in spec.

### 8. Missing Observability and Metrics
**Violation**: **MUST** emit events and **SHOULD** expose Prometheus-style metrics.

**Current Implementation**: Basic println! logging, no structured events or metrics.

### 9. Missing CLI Integration Features  
**Violation**: `beectl` should support get/delete operations and observe scheduler-events.

**Current Implementation**: Only supports `apply` command.

### 10. Inconsistent Workflow Implementation
**Violation**: The prompt `workflow-workload-applying.prompt.md` describes a completely different workflow involving:
- Post-quantum encryption with 3 shares, 2 required
- DHT manifest storage
- Asymmetric key encryption per node
- Gossipsub workload topics
- Capability tokens

**Current Implementation**: Uses direct request-response instead of the described encrypted workflow.

## 🟢 Minor Violations  

### 11. Missing Preemption Support
**Current Implementation**: No preemption logic for higher priority tasks.

### 12. Missing Runtime Adapter Interface
**Current Implementation**: Hardcoded references to Podman without pluggable interface.

### 13. Missing Replay Protection
**Current Implementation**: No nonce/timestamp validation for replay attack prevention.

## 🎯 Compliance Checklist Status

- [ ] Node generates unique Machine Peer ID (❌ MISSING)
- [ ] All pub/sub and stream messages are signed (❌ MISSING) 
- [ ] Tasks published once; de‑duplicated by Task ID (❌ MISSING)
- [ ] Deterministic scoring and tie‑breaks (❌ MISSING)
- [ ] Lease CAS with TTL for winner election (❌ MISSING)
- [ ] Deployment confirmation events (❌ MISSING)
- [ ] Backoff and retry after lease expiry (❌ MISSING)
- [ ] MDHT stores only transient metadata (✅ PARTIAL - DHT exists)
- [ ] Workload DHT contains no machine data (❌ NEEDS VERIFICATION)

## 🚀 Recommended Implementation Priority

### Phase 1 (Critical - 2-4 weeks)
1. Implement three pub/sub topics
2. Create Task, Bid, Event, Lease FlatBuffer schemas
3. Add ULID dependency and task ID generation
4. Implement basic message signing/verification

### Phase 2 (Core Scheduling - 4-6 weeks)  
1. Implement ephemeral scheduling algorithm
2. Add lease management with CAS operations
3. Implement bidding and scoring logic
4. Add selection windows and timers

### Phase 3 (Observability - 2-3 weeks)
1. Add structured event emission
2. Implement Prometheus metrics
3. Add configuration file support
4. Enhance CLI with get/delete operations

### Phase 4 (Advanced Features - 3-4 weeks)
1. Add preemption support
2. Implement runtime adapter interface
3. Add replay protection
4. Reconcile workflow differences between spec and prompts

## 📋 Documentation Issues

1. **Inconsistent Workflows**: The `workflow-workload-applying.prompt.md` describes a fundamentally different approach than the main specification
2. **Empty Prompt Files**: Several prompt files are empty and need content
3. **Missing Implementation Guidance**: The specification lacks guidance on how to implement CAS operations with libp2p Kademlia DHT

The current implementation appears to be an early prototype that implements basic peer discovery and apply operations, but is missing the core ephemeral scheduling algorithm that defines the Beemesh architecture.
