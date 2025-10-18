//! Workload Scheduler
//!
//! This crate provides scheduling strategies for placing workloads on available nodes
//! in a decentralized environment. The scheduler focuses on optimal node selection
//! for workload placement rather than cryptographic distribution.

use log::info;

/// Configuration for scheduling strategy
#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    /// Maximum number of nodes to consider for scheduling
    pub max_candidates: Option<usize>,
    /// Scheduling strategy to use
    pub strategy: SchedulingStrategy,
    /// Whether to enable load balancing considerations
    pub enable_load_balancing: bool,
}

/// Scheduling strategy determines how nodes are selected
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SchedulingStrategy {
    /// Round-robin selection among available nodes
    RoundRobin,
    /// Random selection from available nodes
    Random,
    /// Load-based selection (future: consider node capacity/load)
    LoadBased,
    /// First available node
    FirstAvailable,
}

/// Result of scheduling calculation
#[derive(Debug, Clone)]
pub struct SchedulingPlan {
    /// Selected node candidates for workload placement
    pub selected_candidates: Vec<usize>,
    /// Total number of available candidates
    pub total_candidates: usize,
    /// Strategy used for selection
    pub strategy_used: SchedulingStrategy,
}

/// Node candidate information
#[derive(Debug, Clone)]
pub struct NodeCandidate {
    /// Unique identifier for the node
    pub node_id: String,
    /// Current load factor (0.0 = no load, 1.0 = full capacity)
    pub load_factor: f32,
    /// Whether the node is currently available
    pub available: bool,
    /// Node capabilities/resources
    pub capabilities: NodeCapabilities,
}

/// Node capabilities and resources
#[derive(Debug, Clone)]
pub struct NodeCapabilities {
    /// Available CPU cores
    pub cpu_cores: u32,
    /// Available memory in MB
    pub memory_mb: u64,
    /// Available storage in MB
    pub storage_mb: u64,
    /// Supported container runtimes
    pub container_runtimes: Vec<String>,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            max_candidates: None,
            strategy: SchedulingStrategy::RoundRobin,
            enable_load_balancing: true,
        }
    }
}

impl Default for NodeCapabilities {
    fn default() -> Self {
        Self {
            cpu_cores: 1,
            memory_mb: 1024,
            storage_mb: 10240,
            container_runtimes: vec!["docker".to_string()],
        }
    }
}

/// Main scheduler implementation
pub struct Scheduler {
    config: SchedulerConfig,
    round_robin_counter: std::sync::atomic::AtomicUsize,
}

impl Scheduler {
    /// Create a new scheduler with the given configuration
    pub fn new(config: SchedulerConfig) -> Self {
        Self {
            config,
            round_robin_counter: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Schedule a workload on available candidates
    pub fn schedule_workload(
        &self,
        candidates: &[NodeCandidate],
        required_count: usize,
    ) -> Result<SchedulingPlan, SchedulingError> {
        if candidates.is_empty() {
            return Err(SchedulingError::NoCandidatesAvailable);
        }

        // Filter available candidates
        let available_candidates: Vec<(usize, &NodeCandidate)> = candidates
            .iter()
            .enumerate()
            .filter(|(_, candidate)| candidate.available)
            .collect();

        if available_candidates.is_empty() {
            return Err(SchedulingError::NoAvailableCandidates);
        }

        let available_count = available_candidates.len();
        let actual_required_count = if let Some(max) = self.config.max_candidates {
            std::cmp::min(required_count, max)
        } else {
            required_count
        };

        if available_count < actual_required_count {
            return Err(SchedulingError::InsufficientCandidates {
                available: available_count,
                required: actual_required_count,
            });
        }

        let selected_indices =
            self.select_candidates(&available_candidates, actual_required_count)?;

        let plan = SchedulingPlan {
            selected_candidates: selected_indices,
            total_candidates: candidates.len(),
            strategy_used: self.config.strategy,
        };

        info!(
            "Scheduled workload: selected {} candidates from {} available using {:?} strategy",
            plan.selected_candidates.len(),
            available_count,
            plan.strategy_used
        );

        Ok(plan)
    }

    /// Select candidates based on the configured strategy
    fn select_candidates(
        &self,
        available_candidates: &[(usize, &NodeCandidate)],
        count: usize,
    ) -> Result<Vec<usize>, SchedulingError> {
        match self.config.strategy {
            SchedulingStrategy::FirstAvailable => Ok(available_candidates
                .iter()
                .take(count)
                .map(|(idx, _)| *idx)
                .collect()),
            SchedulingStrategy::RoundRobin => self.select_round_robin(available_candidates, count),
            SchedulingStrategy::Random => self.select_random(available_candidates, count),
            SchedulingStrategy::LoadBased => self.select_load_based(available_candidates, count),
        }
    }

    /// Round-robin selection
    fn select_round_robin(
        &self,
        available_candidates: &[(usize, &NodeCandidate)],
        count: usize,
    ) -> Result<Vec<usize>, SchedulingError> {
        let start_index = self
            .round_robin_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut selected = Vec::new();

        for i in 0..count {
            let idx = (start_index + i) % available_candidates.len();
            selected.push(available_candidates[idx].0);
        }

        Ok(selected)
    }

    /// Random selection
    fn select_random(
        &self,
        available_candidates: &[(usize, &NodeCandidate)],
        count: usize,
    ) -> Result<Vec<usize>, SchedulingError> {
        use std::collections::HashSet;

        let mut selected = HashSet::new();
        let mut attempts = 0;
        const MAX_ATTEMPTS: usize = 1000;

        // Simple pseudo-random selection (in production, use a proper RNG)
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as usize;

        while selected.len() < count && attempts < MAX_ATTEMPTS {
            let idx = (seed.wrapping_add(attempts)) % available_candidates.len();
            selected.insert(available_candidates[idx].0);
            attempts += 1;
        }

        if selected.len() < count {
            return Err(SchedulingError::SelectionFailed);
        }

        Ok(selected.into_iter().collect())
    }

    /// Load-based selection (selects nodes with lowest load first)
    fn select_load_based(
        &self,
        available_candidates: &[(usize, &NodeCandidate)],
        count: usize,
    ) -> Result<Vec<usize>, SchedulingError> {
        let mut candidates_with_load: Vec<_> = available_candidates
            .iter()
            .map(|(idx, candidate)| (*idx, candidate.load_factor))
            .collect();

        // Sort by load factor (lowest first)
        candidates_with_load
            .sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        Ok(candidates_with_load
            .into_iter()
            .take(count)
            .map(|(idx, _)| idx)
            .collect())
    }

    /// Get scheduler statistics
    pub fn get_stats(&self) -> SchedulerStats {
        SchedulerStats {
            strategy: self.config.strategy,
            round_robin_counter: self
                .round_robin_counter
                .load(std::sync::atomic::Ordering::Relaxed),
            load_balancing_enabled: self.config.enable_load_balancing,
        }
    }
}

/// Scheduler statistics
#[derive(Debug)]
pub struct SchedulerStats {
    pub strategy: SchedulingStrategy,
    pub round_robin_counter: usize,
    pub load_balancing_enabled: bool,
}

/// Errors that can occur during scheduling
#[derive(Debug, Clone)]
pub enum SchedulingError {
    /// No candidates provided
    NoCandidatesAvailable,
    /// No candidates are currently available
    NoAvailableCandidates,
    /// Not enough candidates to meet requirements
    InsufficientCandidates { available: usize, required: usize },
    /// Selection algorithm failed
    SelectionFailed,
}

impl std::fmt::Display for SchedulingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SchedulingError::NoCandidatesAvailable => {
                write!(f, "No candidates available for scheduling")
            }
            SchedulingError::NoAvailableCandidates => {
                write!(f, "No candidates are currently available")
            }
            SchedulingError::InsufficientCandidates {
                available,
                required,
            } => {
                write!(
                    f,
                    "Insufficient candidates: have {}, need {}",
                    available, required
                )
            }
            SchedulingError::SelectionFailed => {
                write!(f, "Failed to select candidates")
            }
        }
    }
}

impl std::error::Error for SchedulingError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_candidates(count: usize) -> Vec<NodeCandidate> {
        (0..count)
            .map(|i| NodeCandidate {
                node_id: format!("node-{}", i),
                load_factor: (i as f32) * 0.1,
                available: true,
                capabilities: NodeCapabilities::default(),
            })
            .collect()
    }

    #[test]
    fn test_first_available_scheduling() {
        let config = SchedulerConfig {
            strategy: SchedulingStrategy::FirstAvailable,
            ..Default::default()
        };
        let scheduler = Scheduler::new(config);
        let candidates = create_test_candidates(5);

        let plan = scheduler.schedule_workload(&candidates, 2).unwrap();
        assert_eq!(plan.selected_candidates, vec![0, 1]);
    }

    #[test]
    fn test_insufficient_candidates() {
        let config = SchedulerConfig::default();
        let scheduler = Scheduler::new(config);
        let candidates = create_test_candidates(2);

        let result = scheduler.schedule_workload(&candidates, 5);
        assert!(matches!(
            result,
            Err(SchedulingError::InsufficientCandidates { .. })
        ));
    }

    #[test]
    fn test_load_based_scheduling() {
        let config = SchedulerConfig {
            strategy: SchedulingStrategy::LoadBased,
            ..Default::default()
        };
        let scheduler = Scheduler::new(config);
        let candidates = create_test_candidates(5);

        let plan = scheduler.schedule_workload(&candidates, 2).unwrap();
        // Should select nodes with lowest load (0 and 1)
        assert_eq!(plan.selected_candidates, vec![0, 1]);
    }
}
