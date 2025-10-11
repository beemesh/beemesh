# Machine Plane Capability Rebalancing

## Overview

This document describes the capability rebalancing approach for the Beemesh machine plane, addressing the fundamental tension between maintaining ephemeral/stateless principles while providing essential self-healing capabilities for zero-trust encrypted workload scheduling.

## Problem Statement

### The Fundamental Challenge

In a zero-trust decentralized mesh with encrypted manifests, the machine plane faces a critical challenge:

- **Manifests are encrypted** to protect sensitive workload information from compromised nodes
- **Keyshares are distributed** using Shamir's secret sharing across multiple nodes
- **Manifest capabilities** control access to encrypted manifest data
- **Node failures** can reduce capability distribution below safe thresholds
- **Scheduling becomes impossible** when no node has both keyshare and manifest capabilities

### Why Workload Plane Cannot Handle This

The workload plane **cannot** be responsible for capability rebalancing because:

1. **No cryptographic knowledge**: Workload plane doesn't understand keyshares, thresholds, or reconstruction
2. **No capability visibility**: Cannot assess capability distribution or security constraints
3. **Trust boundary violation**: Would need to fully trust machine plane for all capability operations
4. **Wrong abstraction layer**: Capability management is infrastructure, not workload concern

### Why External Controllers Are Insufficient

Pure external controller approaches break decentralization:

1. **Single points of failure**: External controllers become centralized authorities
2. **Trust requirements**: Must trust controllers to make security-correct capability decisions
3. **Coordination complexity**: Controllers must coordinate with each other to avoid conflicts
4. **Availability dependency**: System availability depends on controller availability

## Architecture Principles

### Core Tensions and Trade-offs

The machine plane must balance competing requirements:

| Principle | Ideal | Compromise Required |
|-----------|-------|-------------------|
| **Stateless** | No persistent cluster state | Minimal capability health tracking for active manifests |
| **Ephemeral** | Fire-and-forget operations | Brief coordination during capability emergencies |
| **A/P Design** | No consistency requirements | Temporary coordination that fails gracefully during partitions |
| **Lock-free** | No coordination mechanisms | Short-lived coordination leases for capability recovery |
| **Zero-trust** | No privileged nodes | Cryptographic validation of all capability operations |

### Acceptable Compromises

We accept **minimal, bounded compromises** to enable essential functionality:

- ✅ **Reactive monitoring** triggered by actual failures (not proactive)
- ✅ **Temporary coordination** for capability recovery (30 seconds max, auto-cleanup)
- ✅ **Minimal state** for active manifest capability health (expires automatically)
- ✅ **Emergency authority** during capability recovery (cryptographically validated)
- ❌ **Continuous monitoring** or persistent coordination infrastructure
- ❌ **Proactive optimization** or performance-driven rebalancing

## Capability Rebalancing Design

### Detection: Reactive Triggers Only

Rebalancing is triggered **only** by concrete operational failures:

```rust
enum RebalancingTrigger {
    // Node stops responding to lease renewal, task communication, etc.
    NodeTimeout(PeerId),

    // Encrypted task published but no capable nodes bid
    SchedulingFailure { task_id: String, manifest_id: String },

    // DHT provider announcements show capability holder loss
    CapabilityProviderLoss { manifest_id: String, provider_type: CapabilityType },
}
```

**No proactive monitoring** - detection happens through existing machine plane timeout mechanisms.

### Assessment: Stateless Capability Discovery

When rebalancing is triggered, discover current state on-demand:

```rust
struct CapabilityDistributionState {
    keyshare_holders: Vec<PeerId>,           // Current responsive keyshare holders
    manifest_holders: Vec<PeerId>,           // Current responsive manifest holders
    reconstruction_threshold: usize,         // Minimum keyshares needed for decryption
    scheduling_viable: bool,                 // Can tasks be scheduled?
    security_compliant: bool,                // Distribution meets security constraints?
}
```

Discovery uses:
- DHT provider queries for current capability announcements
- Direct peer communication to verify responsiveness
- No persistent tracking - fresh discovery each time

### Planning: Minimal Intervention

Calculate the **smallest possible intervention** to restore scheduling capability:

```rust
enum MinimalRebalancingAction {
    // Regenerate keyshares while reconstruction is still possible
    RegenerateKeyshares {
        manifest_id: String,
        additional_shares_needed: usize,
        target_nodes: Vec<PeerId>,
    },

    // Duplicate manifest capability to more nodes
    DuplicateManifestCapability {
        manifest_id: String,
        source_holder: PeerId,
        target_nodes: Vec<PeerId>,
    },

    // Transfer existing capability between nodes
    TransferCapability {
        manifest_id: String,
        capability_type: CapabilityType,
        from_node: PeerId,
        to_node: PeerId,
    },

    // Insufficient surviving capabilities - cannot recover
    Unrecoverable,
}
```

**Principles**:
- Restore **minimum viability** for scheduling, not optimal distribution
- **Preserve security constraints** - validate all actions cryptographically
- **Fail fast** if recovery impossible rather than attempt unsafe operations

### Validation: Zero-Trust Security Enforcement

Every rebalancing action must pass security validation:

```rust
struct ZeroTrustConstraints {
    min_reconstruction_threshold: usize,      // Never compromise threshold requirements
    max_capabilities_per_node: usize,        // Prevent dangerous capability concentration
    geographic_distribution_rules: Vec<Rule>, // Maintain data residency requirements
    organizational_separation: Vec<Policy>,   // Prevent single-org capability control
}
```

**Security checks**:
- Reconstruction threshold maintained
- No dangerous capability concentration created
- Geographic and regulatory constraints preserved
- Organizational distribution policies enforced

### Execution: Temporary Coordination

Execute rebalancing with **short-lived, bounded coordination**:

```rust
struct EmergencyCapabilityRecovery {
    manifest_id: String,
    coordinator_lease: String,               // DHT lease for coordination authority
    coordination_deadline: Instant,          // Fixed 30-second maximum duration
    participants: Vec<PeerId>,               // Nodes involved in recovery
    security_constraints: ZeroTrustConstraints, // Validation rules
}
```

**Coordination process**:
1. **Acquire coordination lease** (30 seconds max, auto-expires)
2. **Verify participant availability** and collect commitments
3. **Execute recovery action** with multi-party cooperation
4. **Validate result** against security constraints
5. **Cleanup coordination state** regardless of success/failure

### Recovery Operations

#### Keyshare Regeneration
When keyshare holders fail but reconstruction is still possible:

1. **Collect existing keyshares** from surviving holders (need threshold count)
2. **Cooperatively reconstruct secret** using multi-party computation
3. **Generate additional keyshares** to restore desired distribution
4. **Distribute new keyshares** to selected target nodes
5. **Verify distribution** and cleanup coordination

#### Manifest Capability Duplication
When manifest holders become unavailable:

1. **Request encrypted manifest** from surviving holder
2. **Generate manifest capability tokens** for target nodes
3. **Distribute manifest + capabilities** to target nodes
4. **Verify capability installation** and cleanup coordination

#### Capability Transfer
When nodes need to transfer capabilities (e.g., node shutdown, rebalancing):

1. **Validate transfer safety** against security constraints
2. **Generate new capability token** for target node
3. **Securely transfer capability** with cryptographic proof
4. **Revoke old capability** from source node
5. **Verify transfer completion** and cleanup coordination

## Implementation Strategy

### Integration with Current Scheduler

Rebalancing extends existing machine plane mechanisms:

- **Leverage existing timeout detection** for node failure identification
- **Extend task scheduling failure handling** to include capability analysis
- **Use existing DHT lease patterns** for coordination authority
- **Build on current peer communication** for capability operations

### Minimal State Requirements

The machine plane maintains **minimal, self-cleaning state**:

```rust
struct EssentialCapabilityState {
    // Only track capabilities for recently active manifests
    active_manifests: HashMap<ManifestId, CapabilityHealth>,
    manifest_ttl: Duration::from_hours(24),           // Auto-expire entries

    // Track active recovery operations to prevent conflicts
    active_recoveries: HashMap<ManifestId, RecoveryLease>,
    max_recovery_duration: Duration::from_secs(30),  // Force cleanup
}
```

**State properties**:
- Entries **expire automatically** - no persistent cluster state
- **Scoped to individual manifests** - no global coordination
- **Self-cleaning** - bounded memory usage and coordination duration

### Error Handling and Degradation

**Graceful degradation** when rebalancing fails:

- **Coordination timeouts** → Capability recovery fails, scheduling remains unavailable
- **Network partitions** → Partitioned groups operate independently, may have reduced capability
- **Insufficient nodes** → Honest failure reporting, no unsafe recovery attempts
- **Security violations** → Reject rebalancing, maintain security over availability

## Security Considerations

### Node Compromise Protection

Capability rebalancing **enhances security** against compromised nodes:

- **Proactive redistribution** away from suspicious nodes
- **Emergency capability revocation** for confirmed compromised nodes
- **Compromise-aware recovery** that excludes suspicious participants
- **Capability rotation** to limit blast radius of compromises

### Trust Minimization

Rebalancing operations minimize trust requirements:

- **Multi-party execution** - no single node has unilateral authority
- **Cryptographic validation** - all operations verified against security constraints
- **Automatic cleanup** - coordination authority expires automatically
- **Participant consensus** - recovery requires agreement from multiple nodes

### Attack Resistance

The system resists various attack vectors:

- **Capability hoarding** - Concentration limits prevent accumulation
- **False failure reports** - Direct verification of node responsiveness
- **Malicious coordination** - Security validation prevents unsafe operations
- **Social engineering** - Cryptographic proofs required for all operations

## Monitoring and Observability

### Rebalancing Events

Key events for monitoring system health:

- **Rebalancing triggers** - What caused capability recovery to activate
- **Coordination formation** - Which nodes participated in recovery
- **Security validations** - What constraints were checked and results
- **Recovery outcomes** - Success/failure of capability restoration
- **Cleanup completion** - Verification that coordination state was cleaned up

### Health Metrics

Essential metrics for capability distribution health:

- **Manifest scheduling viability** - Can encrypted tasks be scheduled
- **Capability distribution ratio** - Current vs. optimal distribution
- **Recovery frequency** - How often rebalancing is triggered
- **Recovery latency** - Time from trigger to restored scheduling capability
- **Security violations** - Attempted operations that violated constraints

## Future Considerations

### Scalability

As the system scales, consider:

- **Capability discovery efficiency** - DHT queries at scale
- **Coordination scalability** - Multi-party operations with many participants
- **Network partition resilience** - Larger networks, more partition scenarios
- **Cross-tenant isolation** - Capability management across tenant boundaries

### Security Evolution

Potential security enhancements:

- **Advanced compromise detection** - ML-based anomaly detection for capability behavior
- **Automated capability rotation** - Periodic rotation to limit compromise windows
- **Cross-cluster replication** - Capability distribution across multiple clusters
- **Quantum-resistant cryptography** - Future-proofing against quantum attacks

## Conclusion

The machine plane capability rebalancing approach represents a **carefully balanced compromise** between pure ephemeral principles and essential self-healing functionality. By accepting **minimal, bounded violations** of stateless/ephemeral principles, we enable **critical zero-trust capability management** while preserving the core architecture philosophy.

The key insight is that **some coordination is inevitable** for maintaining security properties in a zero-trust system - the goal is to make that coordination as **minimal, reactive, and bounded** as possible while still preventing catastrophic capability loss that would render the system unusable.

This approach provides **essential self-healing capability** without turning the machine plane into a full orchestration system, maintaining its focus on **ephemeral scheduling** while adding just enough **capability management** to ensure scheduling remains possible in the face of node failures and compromises.

# Future Directions
## 1. Suggested Enhancements (Actionable)

Prioritized list (higher value earlier):

1. Introduce a formal “RecoveryProposal” message schema (signed) and embed it into machineplane README (Protocols section).
2. Replace (or offer option to replace) secret reconstruction step with a resharing protocol (document normative security recommendation: reconstruction SHOULD NOT expose plaintext secret to any single node).
3. Specify lease data structure and acquisition algorithm (tie-breaker + backoff).
4. Define “capability distribution entropy” metric (e.g., normalized Shannon entropy across failure domains) and optional soft floor for opportunistic correction.
5. Add rate limiting / hysteresis around triggers (e.g., minimum interval per manifest between recoveries).
6. Extend compliance checklist (machineplane README) with: “Capability recovery MUST enforce: threshold invariants; concentration limits; signed recovery artifact; ≤30s lease duration.”
7. Document a revocation flow for capability supersession (how old keyshare or manifest capability is invalidated & how that is cryptographically proven to others).
8. Add partition-safe merge rule post-reconciliation (deterministic pruning order).
9. Provide observability event names and sample payloads: CapabilityRecoveryStarted, CapabilityRecoveryResult {success|failure|unrecoverable}, CapabilityDistributionStateSnapshot, CapabilitySecurityViolation.
10. Clarify security constraints source of truth (static config? signed policy object? per-tenant?).

---

## 2. Where the Current Design Is Strong

- Explicitly scoped minimal deviation from pure statelessness.
- Reactive-only philosophy preserves simplicity and operational frugality.
- Clear separation from the Workplane; no trust boundary erosion.
- Security-first failure mode (fail closed rather than degrade confidentiality).
- Provides an extensible mental model (Trigger → Assess → Plan → Validate → Execute → Cleanup).

---

## 3. Potential Oversights (Worth Calling Out Early)

- Lack of mention of how new nodes prove suitability (e.g., hardware attestation, jurisdiction metadata) before receiving capabilities.
- No anti-entropy reconciliation loop described to correct silent drift (e.g., a holder that still advertises but actually lost local state).
- Missing explicit cryptographic binding between manifest version and capability distribution snapshot (for replay protection).
- Recovery operation outcome verification relies on “validate result” but not enumerated invariants; formalizing invariants helps with testing and compliance automation.
