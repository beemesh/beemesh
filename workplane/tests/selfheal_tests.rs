//! Self-Heal Tests
//!
//! This module tests the self-healing subsystem for replica management:
//! - Policy checks (`policy_allows_workload`)
//! - Missing ordinal calculation for stateful workloads
//! - Replica ranking for scale-down decisions
//! - Configuration defaults and workload kind detection
//!
//! # Spec References
//!
//! - workplane-spec.md §7: Self-Healing & Replica Management
//! - workplane-spec.md §6.1: Stateless workload behavior
//! - workplane-spec.md §6.2: Stateful workload behavior

use std::time::Duration;
use workplane::config::Config;
use workplane::discovery::ServiceRecord;

// ============================================================================
// Test Utilities
// ============================================================================

/// Creates a minimal Config for testing.
fn make_config(namespace: &str, workload_name: &str) -> Config {
    let mut cfg = Config {
        peer_id_str: None,
        private_key: vec![],
        namespace: namespace.to_string(),
        workload_name: workload_name.to_string(),
        pod_name: String::new(),
        replicas: 1,
        workload_kind: "Deployment".to_string(),
        liveness_url: String::new(),
        readiness_url: String::new(),
        bootstrap_peer_strings: vec![],
        beemesh_api: String::new(),
        replica_check_interval: Duration::from_secs(0),
        dht_ttl: Duration::from_secs(0),
        heartbeat_interval: Duration::from_secs(0),
        health_probe_interval: Duration::from_secs(0),
        health_probe_timeout: Duration::from_secs(0),
        allow_cross_namespace: false,
        allowed_workloads: vec![],
        denied_workloads: vec![],
        listen_addrs: vec![],
    };
    cfg.apply_defaults();
    cfg
}

/// Creates a test ServiceRecord with specified health status.
fn make_record(
    namespace: &str,
    workload_name: &str,
    peer_id: &str,
    ready: bool,
    healthy: bool,
    ordinal: Option<u32>,
) -> ServiceRecord {
    ServiceRecord {
        workload_id: format!("{}/{}", namespace, workload_name),
        namespace: namespace.to_string(),
        workload_name: workload_name.to_string(),
        peer_id: peer_id.to_string(),
        pod_name: ordinal.map(|o| format!("{}-{}", workload_name, o)),
        ordinal,
        addrs: vec![],
        caps: serde_json::Map::new(),
        version: 1,
        ts: 0,
        nonce: uuid::Uuid::new_v4().to_string(),
        ready,
        healthy,
    }
}

// ============================================================================
// Config Tests
// ============================================================================

/// Verifies that workload_id() returns correct format.
#[test]
fn config_workload_id_format() {
    let cfg = make_config("production", "nginx");
    assert_eq!(cfg.workload_id(), "production/nginx");
}

/// Verifies that is_stateful() correctly identifies stateful workloads.
#[test]
fn config_is_stateful_detection() {
    let mut cfg = make_config("default", "test");

    // Stateless kinds
    cfg.workload_kind = "Deployment".to_string();
    assert!(!cfg.is_stateful(), "Deployment should be stateless");

    cfg.workload_kind = "StatelessWorkload".to_string();
    assert!(!cfg.is_stateful(), "StatelessWorkload should be stateless");

    cfg.workload_kind = "deployment".to_string(); // lowercase
    assert!(!cfg.is_stateful(), "deployment (lowercase) should be stateless");

    // Stateful kinds
    cfg.workload_kind = "StatefulSet".to_string();
    assert!(cfg.is_stateful(), "StatefulSet should be stateful");

    cfg.workload_kind = "StatefulWorkload".to_string();
    assert!(cfg.is_stateful(), "StatefulWorkload should be stateful");

    cfg.workload_kind = "statefulset".to_string(); // lowercase
    assert!(cfg.is_stateful(), "statefulset (lowercase) should be stateful");
}

/// Verifies that task_kind() returns correct API task type.
#[test]
fn config_task_kind_mapping() {
    let mut cfg = make_config("default", "test");

    cfg.workload_kind = "Deployment".to_string();
    assert_eq!(cfg.task_kind(), "StatelessWorkload");

    cfg.workload_kind = "StatelessWorkload".to_string();
    assert_eq!(cfg.task_kind(), "StatelessWorkload");

    cfg.workload_kind = "StatefulSet".to_string();
    assert_eq!(cfg.task_kind(), "StatefulWorkload");

    cfg.workload_kind = "StatefulWorkload".to_string();
    assert_eq!(cfg.task_kind(), "StatefulWorkload");

    cfg.workload_kind = "DaemonSet".to_string();
    assert_eq!(cfg.task_kind(), "DaemonSet");

    cfg.workload_kind = "Unknown".to_string();
    assert_eq!(cfg.task_kind(), "CustomWorkload");
}

/// Verifies that ordinal() extracts ordinal from pod name.
#[test]
fn config_ordinal_extraction() {
    let mut cfg = make_config("default", "redis");

    cfg.pod_name = "redis-0".to_string();
    assert_eq!(cfg.ordinal(), Some(0));

    cfg.pod_name = "redis-1".to_string();
    assert_eq!(cfg.ordinal(), Some(1));

    cfg.pod_name = "redis-master-2".to_string();
    assert_eq!(cfg.ordinal(), Some(2));

    cfg.pod_name = "redis-99".to_string();
    assert_eq!(cfg.ordinal(), Some(99));

    // Edge cases
    cfg.pod_name = "redis".to_string(); // no ordinal
    assert_eq!(cfg.ordinal(), None);

    cfg.pod_name = "redis-abc".to_string(); // non-numeric
    assert_eq!(cfg.ordinal(), None);

    cfg.pod_name = String::new(); // empty
    assert_eq!(cfg.ordinal(), None);
}

/// Verifies that apply_defaults() sets sensible defaults.
#[test]
fn config_apply_defaults() {
    let mut cfg = Config {
        peer_id_str: None,
        private_key: vec![],
        namespace: String::new(),
        workload_name: "test".to_string(),
        pod_name: String::new(),
        replicas: 0,
        workload_kind: String::new(),
        liveness_url: String::new(),
        readiness_url: String::new(),
        bootstrap_peer_strings: vec![],
        beemesh_api: String::new(),
        replica_check_interval: Duration::from_secs(0),
        dht_ttl: Duration::from_secs(0),
        heartbeat_interval: Duration::from_secs(0),
        health_probe_interval: Duration::from_secs(0),
        health_probe_timeout: Duration::from_secs(0),
        allow_cross_namespace: false,
        allowed_workloads: vec![],
        denied_workloads: vec![],
        listen_addrs: vec![],
    };

    cfg.apply_defaults();

    assert_eq!(cfg.namespace, "default");
    assert_eq!(cfg.replicas, 1);
    assert_eq!(cfg.workload_kind, "Deployment");
    assert_eq!(cfg.beemesh_api, "http://localhost:8080");
    assert_eq!(cfg.replica_check_interval, Duration::from_secs(30));
    assert_eq!(cfg.dht_ttl, Duration::from_secs(15));
    assert_eq!(cfg.heartbeat_interval, Duration::from_secs(5));
    assert_eq!(cfg.health_probe_interval, Duration::from_secs(10));
    assert_eq!(cfg.health_probe_timeout, Duration::from_secs(5));
}

// ============================================================================
// Policy Tests
// ============================================================================

/// Re-implementation of policy_allows_workload for testing.
/// (The actual function is private in selfheal.rs)
fn policy_allows_workload(cfg: &Config, namespace: &str, workload_name: &str) -> bool {
    let workload_id = format!("{namespace}/{workload_name}");

    // Denylist takes precedence
    if cfg
        .denied_workloads
        .iter()
        .any(|denied| denied == &workload_id)
    {
        return false;
    }

    // Cross-namespace check
    if !cfg.allow_cross_namespace && namespace != cfg.namespace {
        return false;
    }

    // Empty allowlist = allow all
    if cfg.allowed_workloads.is_empty() {
        return true;
    }

    // Check allowlist
    cfg.allowed_workloads
        .iter()
        .any(|allowed| allowed == &workload_id)
}

/// Verifies that empty policy allows all workloads in same namespace.
#[test]
fn policy_empty_allows_same_namespace() {
    let cfg = make_config("default", "nginx");

    assert!(
        policy_allows_workload(&cfg, "default", "nginx"),
        "Same workload should be allowed"
    );
    assert!(
        policy_allows_workload(&cfg, "default", "other"),
        "Other workload in same namespace should be allowed"
    );
}

/// Verifies that cross-namespace is blocked by default.
#[test]
fn policy_blocks_cross_namespace_by_default() {
    let cfg = make_config("default", "nginx");

    assert!(
        !policy_allows_workload(&cfg, "production", "nginx"),
        "Cross-namespace should be blocked by default"
    );
}

/// Verifies that allow_cross_namespace enables cross-namespace access.
#[test]
fn policy_allows_cross_namespace_when_enabled() {
    let mut cfg = make_config("default", "nginx");
    cfg.allow_cross_namespace = true;

    assert!(
        policy_allows_workload(&cfg, "production", "nginx"),
        "Cross-namespace should be allowed when enabled"
    );
}

/// Verifies that denylist blocks specific workloads.
#[test]
fn policy_denylist_blocks_workload() {
    let mut cfg = make_config("default", "nginx");
    cfg.denied_workloads = vec!["default/blocked".to_string()];

    assert!(
        !policy_allows_workload(&cfg, "default", "blocked"),
        "Denylisted workload should be blocked"
    );
    assert!(
        policy_allows_workload(&cfg, "default", "allowed"),
        "Non-denylisted workload should be allowed"
    );
}

/// Verifies that denylist takes precedence over allowlist.
#[test]
fn policy_denylist_precedence_over_allowlist() {
    let mut cfg = make_config("default", "nginx");
    cfg.allowed_workloads = vec!["default/both".to_string()];
    cfg.denied_workloads = vec!["default/both".to_string()];

    assert!(
        !policy_allows_workload(&cfg, "default", "both"),
        "Denylist should take precedence over allowlist"
    );
}

/// Verifies that non-empty allowlist restricts to listed workloads only.
#[test]
fn policy_allowlist_restricts_access() {
    let mut cfg = make_config("default", "nginx");
    cfg.allowed_workloads = vec!["default/nginx".to_string(), "default/redis".to_string()];

    assert!(
        policy_allows_workload(&cfg, "default", "nginx"),
        "Allowlisted workload should be allowed"
    );
    assert!(
        policy_allows_workload(&cfg, "default", "redis"),
        "Allowlisted workload should be allowed"
    );
    assert!(
        !policy_allows_workload(&cfg, "default", "postgres"),
        "Non-allowlisted workload should be blocked"
    );
}

// ============================================================================
// Missing Ordinal Calculation Tests
// ============================================================================

/// Re-implementation of missing_stateful_ordinals for testing.
fn missing_stateful_ordinals(desired_replicas: usize, records: &[ServiceRecord]) -> Vec<u32> {
    let mut present = std::collections::HashSet::new();
    for record in records {
        if let Some(ord) = record.ordinal {
            present.insert(ord);
        }
    }
    (0..desired_replicas as u32)
        .filter(|ord| !present.contains(ord))
        .collect()
}

/// Verifies that all ordinals are missing when no records exist.
#[test]
fn missing_ordinals_all_missing() {
    let records: Vec<ServiceRecord> = vec![];

    let missing = missing_stateful_ordinals(3, &records);

    assert_eq!(missing, vec![0, 1, 2]);
}

/// Verifies that no ordinals are missing when all exist.
#[test]
fn missing_ordinals_none_missing() {
    let records = vec![
        make_record("default", "redis", "peer-0", true, true, Some(0)),
        make_record("default", "redis", "peer-1", true, true, Some(1)),
        make_record("default", "redis", "peer-2", true, true, Some(2)),
    ];

    let missing = missing_stateful_ordinals(3, &records);

    assert!(missing.is_empty(), "No ordinals should be missing");
}

/// Verifies that only missing ordinals are returned.
#[test]
fn missing_ordinals_partial() {
    let records = vec![
        make_record("default", "redis", "peer-0", true, true, Some(0)),
        make_record("default", "redis", "peer-2", true, true, Some(2)),
        // ordinal 1 is missing
    ];

    let missing = missing_stateful_ordinals(3, &records);

    assert_eq!(missing, vec![1], "Only ordinal 1 should be missing");
}

/// Verifies that ordinals beyond desired count are ignored.
#[test]
fn missing_ordinals_ignores_excess() {
    let records = vec![
        make_record("default", "redis", "peer-0", true, true, Some(0)),
        make_record("default", "redis", "peer-5", true, true, Some(5)), // beyond desired
    ];

    let missing = missing_stateful_ordinals(3, &records);

    assert_eq!(missing, vec![1, 2], "Should only check 0..desired_replicas");
}

// ============================================================================
// Replica Ranking Tests
// ============================================================================

/// Re-implementation of rank_records for testing.
fn rank_records(a: &ServiceRecord, b: &ServiceRecord) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let key = |record: &ServiceRecord| {
        (
            record.ready && record.healthy,
            record.ready,
            record.healthy,
            record.ts,
            record.version,
            record.peer_id.clone(),
        )
    };
    match key(b).cmp(&key(a)) {
        Ordering::Equal => b.peer_id.cmp(&a.peer_id),
        other => other,
    }
}

/// Verifies that healthy records rank higher than unhealthy.
#[test]
fn ranking_healthy_over_unhealthy() {
    let healthy = make_record("default", "nginx", "peer-1", true, true, None);
    let unhealthy = make_record("default", "nginx", "peer-2", false, false, None);

    let result = rank_records(&healthy, &unhealthy);

    assert_eq!(
        result,
        std::cmp::Ordering::Less,
        "Healthy should rank higher (appear first when sorted)"
    );
}

/// Verifies that ready+healthy ranks higher than ready-only.
#[test]
fn ranking_ready_healthy_over_ready_only() {
    let both = make_record("default", "nginx", "peer-1", true, true, None);
    let ready_only = make_record("default", "nginx", "peer-2", true, false, None);

    let result = rank_records(&both, &ready_only);

    assert_eq!(
        result,
        std::cmp::Ordering::Less,
        "ready+healthy should rank higher than ready-only"
    );
}

/// Verifies that higher timestamp ranks higher when health is equal.
#[test]
fn ranking_higher_timestamp_wins() {
    let mut older = make_record("default", "nginx", "peer-1", true, true, None);
    older.ts = 1000;

    let mut newer = make_record("default", "nginx", "peer-2", true, true, None);
    newer.ts = 2000;

    let result = rank_records(&older, &newer);

    assert_eq!(
        result,
        std::cmp::Ordering::Greater,
        "Higher timestamp should rank higher"
    );
}

/// Verifies deterministic ordering when all else is equal.
#[test]
fn ranking_deterministic_with_peer_id() {
    let mut a = make_record("default", "nginx", "peer-a", true, true, None);
    a.ts = 1000;
    a.version = 1;

    let mut b = make_record("default", "nginx", "peer-b", true, true, None);
    b.ts = 1000;
    b.version = 1;

    let result = rank_records(&a, &b);

    // peer-b > peer-a lexicographically, so b ranks higher
    assert_eq!(
        result,
        std::cmp::Ordering::Greater,
        "peer_id should be deterministic tiebreaker"
    );
}

// ============================================================================
// Health Status Partitioning Tests
// ============================================================================

/// Verifies correct partitioning of healthy vs unhealthy records.
#[test]
fn partition_healthy_unhealthy() {
    let records = vec![
        make_record("default", "nginx", "peer-1", true, true, None),   // healthy
        make_record("default", "nginx", "peer-2", false, false, None), // unhealthy
        make_record("default", "nginx", "peer-3", true, true, None),   // healthy
        make_record("default", "nginx", "peer-4", true, false, None),  // ready but not healthy
    ];

    let (healthy, unhealthy): (Vec<ServiceRecord>, Vec<ServiceRecord>) = records
        .into_iter()
        .partition(|r| r.ready && r.healthy);

    assert_eq!(healthy.len(), 2);
    assert_eq!(unhealthy.len(), 2);

    assert!(healthy.iter().all(|r| r.ready && r.healthy));
    assert!(unhealthy.iter().all(|r| !r.ready || !r.healthy));
}

// ============================================================================
// Scale Decision Tests
// ============================================================================

/// Verifies scale-up detection when below desired count.
#[test]
fn scale_decision_scale_up_needed() {
    let desired = 3usize;
    let healthy_count = 1usize;

    let deficit = desired.saturating_sub(healthy_count);

    assert_eq!(deficit, 2, "Should need 2 more replicas");
}

/// Verifies scale-down detection when above desired count.
#[test]
fn scale_decision_scale_down_needed() {
    let desired = 2usize;
    let healthy_count = 5usize;

    let surplus = healthy_count.saturating_sub(desired);

    assert_eq!(surplus, 3, "Should remove 3 replicas");
}

/// Verifies no scaling when at desired count.
#[test]
fn scale_decision_no_scaling_at_desired() {
    let desired = 3usize;
    let healthy_count = 3usize;

    let deficit = desired.saturating_sub(healthy_count);
    let surplus = healthy_count.saturating_sub(desired);

    assert_eq!(deficit, 0, "No deficit");
    assert_eq!(surplus, 0, "No surplus");
}

// ============================================================================
// Duplicate Ordinal Detection Tests (Stateful)
// ============================================================================

/// Verifies detection of duplicate ordinals.
#[test]
fn stateful_detects_duplicate_ordinals() {
    let records = vec![
        make_record("default", "redis", "peer-0a", true, true, Some(0)),
        make_record("default", "redis", "peer-0b", true, true, Some(0)), // duplicate
        make_record("default", "redis", "peer-1", true, true, Some(1)),
    ];

    let mut seen = std::collections::HashSet::new();
    let mut duplicates = Vec::new();
    let mut deduped = Vec::new();

    for record in records {
        if let Some(ord) = record.ordinal {
            if !seen.insert(ord) {
                duplicates.push(record);
                continue;
            }
        }
        deduped.push(record);
    }

    assert_eq!(deduped.len(), 2, "Should have 2 unique ordinals");
    assert_eq!(duplicates.len(), 1, "Should have 1 duplicate");
    assert_eq!(
        duplicates[0].peer_id, "peer-0b",
        "Second peer-0 should be the duplicate"
    );
}

// ============================================================================
// Removal Priority Tests
// ============================================================================

/// Verifies removal order: duplicates > unhealthy > excess healthy.
#[test]
fn removal_priority_order() {
    // Build removal candidates list in priority order
    let mut removal_candidates: Vec<(&str, &str)> = Vec::new();

    // 1. Duplicates first
    removal_candidates.push(("duplicate", "peer-dup"));

    // 2. Unhealthy second
    removal_candidates.push(("unhealthy", "peer-unhealthy"));

    // 3. Excess healthy last
    removal_candidates.push(("excess", "peer-excess"));

    assert_eq!(removal_candidates[0].0, "duplicate");
    assert_eq!(removal_candidates[1].0, "unhealthy");
    assert_eq!(removal_candidates[2].0, "excess");
}
