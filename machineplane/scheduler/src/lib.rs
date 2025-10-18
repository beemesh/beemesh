//! Zero-Trust Distribution Scheduler
//!
//! This crate provides optimal distribution strategies for keyshares, manifests, and capability tokens
//! in a zero-trust decentralized environment to maximize security and fault tolerance.

use log::info;

/// Configuration for distribution strategy
#[derive(Debug, Clone)]
pub struct DistributionConfig {
    /// Minimum threshold for secret reconstruction (k in k-of-n)
    pub min_threshold: usize,
    /// Maximum acceptable fault tolerance (number of nodes that can fail)
    pub max_fault_tolerance: usize,
    /// Minimum security level (affects how many extra nodes we use)
    pub security_level: SecurityLevel,
}

/// Security level determines how aggressively we distribute
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SecurityLevel {
    /// Minimal distribution - just meets requirements
    Basic,
    /// Balanced distribution - good security/efficiency trade-off
    Standard,
    /// Maximum distribution - prioritizes security over efficiency
    High,
}

/// Result of distribution calculation
#[derive(Debug, Clone)]
pub struct DistributionPlan {
    /// Number of keyshare replicas to create
    pub keyshare_count: usize,
    /// Threshold required for reconstruction (k in k-of-n)
    pub reconstruction_threshold: usize,
    /// Number of manifest replicas to distribute
    pub manifest_count: usize,
    /// Total capability tokens needed (keyshare + manifest)
    pub total_capability_tokens: usize,
    /// Minimum responders needed for zero-trust operation
    pub min_required_responders: usize,
    /// Byzantine fault tolerance (max failures system can handle)
    pub byzantine_fault_tolerance: usize,
}

impl Default for DistributionConfig {
    fn default() -> Self {
        Self {
            min_threshold: 2,
            max_fault_tolerance: 3,
            security_level: SecurityLevel::Standard,
        }
    }
}

impl DistributionPlan {
    /// Calculate optimal distribution based on available candidates and security requirements
    pub fn calculate_optimal_distribution(
        available_candidates: usize,
        config: &DistributionConfig,
    ) -> Result<DistributionPlan, DistributionError> {
        if available_candidates == 0 {
            return Err(DistributionError::InsufficientCandidates {
                available: 0,
                minimum_required: config.min_threshold + 1,
            });
        }

        // Ensure we have enough candidates for basic operation
        let min_required = config.min_threshold + 1;
        if available_candidates < min_required {
            return Err(DistributionError::InsufficientCandidates {
                available: available_candidates,
                minimum_required: min_required,
            });
        }

        // Calculate Byzantine fault tolerance based on available nodes
        // For f failures, need at least 3f+1 nodes (Byzantine agreement)
        let max_byzantine_failures = (available_candidates.saturating_sub(1)) / 3;
        let byzantine_fault_tolerance =
            std::cmp::min(max_byzantine_failures, config.max_fault_tolerance);

        // Calculate keyshare distribution
        let (keyshare_count, reconstruction_threshold) = Self::calculate_keyshare_distribution(
            available_candidates,
            config,
            byzantine_fault_tolerance,
        )?;

        // Calculate manifest distribution
        let manifest_count = Self::calculate_manifest_distribution(
            available_candidates,
            keyshare_count,
            config,
            byzantine_fault_tolerance,
        )?;

        // Total capability tokens
        let total_capability_tokens = keyshare_count + manifest_count;

        // Calculate minimum responders needed
        let min_required_responders = Self::calculate_min_responders(
            keyshare_count,
            manifest_count,
            reconstruction_threshold,
            byzantine_fault_tolerance,
        );

        let plan = DistributionPlan {
            keyshare_count,
            reconstruction_threshold,
            manifest_count,
            total_capability_tokens,
            min_required_responders,
            byzantine_fault_tolerance,
        };

        info!(
            "Calculated distribution plan: keyshares={}, threshold={}, manifests={}, total_tokens={}, min_responders={}, byzantine_ft={}",
            plan.keyshare_count,
            plan.reconstruction_threshold,
            plan.manifest_count,
            plan.total_capability_tokens,
            plan.min_required_responders,
            plan.byzantine_fault_tolerance
        );

        Ok(plan)
    }

    /// Calculate optimal keyshare distribution
    fn calculate_keyshare_distribution(
        available_candidates: usize,
        config: &DistributionConfig,
        byzantine_fault_tolerance: usize,
    ) -> Result<(usize, usize), DistributionError> {
        // Keyshare count based on security level and available candidates
        let keyshare_count = match config.security_level {
            SecurityLevel::Basic => {
                // Minimum viable: k+1 shares (just above threshold)
                config.min_threshold + 1
            }
            SecurityLevel::Standard => {
                // Balanced: use up to 60% of available candidates, but at least k+2
                let balanced = (available_candidates * 3) / 5; // 60%
                std::cmp::max(balanced, config.min_threshold + 2)
            }
            SecurityLevel::High => {
                // Maximum security: use up to 80% of available candidates
                let high_security = (available_candidates * 4) / 5; // 80%
                std::cmp::max(
                    high_security,
                    config.min_threshold + byzantine_fault_tolerance,
                )
            }
        };

        // Ensure we don't exceed available candidates
        let keyshare_count = std::cmp::min(keyshare_count, available_candidates);

        // Reconstruction threshold should be based on the number of shares
        // For k-of-n threshold schemes, k should be > n/2 for security
        // but we also respect the configured minimum
        let reconstruction_threshold = std::cmp::max(
            config.min_threshold,
            (keyshare_count / 2) + 1, // Majority + 1
        );

        // Sanity check
        if reconstruction_threshold >= keyshare_count {
            return Err(DistributionError::InvalidThreshold {
                threshold: reconstruction_threshold,
                total_shares: keyshare_count,
            });
        }

        Ok((keyshare_count, reconstruction_threshold))
    }

    /// Calculate optimal manifest distribution
    fn calculate_manifest_distribution(
        available_candidates: usize,
        keyshare_count: usize,
        config: &DistributionConfig,
        byzantine_fault_tolerance: usize,
    ) -> Result<usize, DistributionError> {
        let manifest_count = match config.security_level {
            SecurityLevel::Basic => {
                // Basic: same nodes that get keyshares also get manifests
                keyshare_count
            }
            SecurityLevel::Standard => {
                // Standard: manifests can go to more nodes for redundancy
                // Use keyshare count + some additional nodes for redundancy
                let additional_redundancy = std::cmp::min(byzantine_fault_tolerance, 2);
                std::cmp::min(keyshare_count + additional_redundancy, available_candidates)
            }
            SecurityLevel::High => {
                // High security: distribute manifests to most available nodes
                // But ensure at least the keyshare holders get them
                let high_distribution = (available_candidates * 3) / 4; // 75%
                std::cmp::max(high_distribution, keyshare_count)
            }
        };

        Ok(std::cmp::min(manifest_count, available_candidates))
    }

    /// Calculate minimum responders needed for zero-trust operation
    fn calculate_min_responders(
        keyshare_count: usize,
        manifest_count: usize,
        reconstruction_threshold: usize,
        byzantine_fault_tolerance: usize,
    ) -> usize {
        // Need enough responders for:
        // 1. Secret reconstruction (at least threshold keyshare holders)
        // 2. Manifest access (at least 1 manifest holder)
        // 3. Fault tolerance buffer

        let min_for_reconstruction = reconstruction_threshold;
        let min_for_manifest_access = 1;
        let fault_tolerance_buffer = byzantine_fault_tolerance;

        let calculated_min =
            min_for_reconstruction + min_for_manifest_access + fault_tolerance_buffer;

        // But don't require more than total available tokens - 1 (leave room for 1 failure)
        let total_tokens = keyshare_count + manifest_count;
        let max_reasonable = total_tokens.saturating_sub(1);

        std::cmp::min(calculated_min, max_reasonable)
    }

    /// Check if a distribution plan can handle the specified number of failures
    pub fn can_handle_failures(&self, failure_count: usize) -> bool {
        // Check if we can still reconstruct secrets
        let remaining_keyshares = self.keyshare_count.saturating_sub(failure_count);
        let can_reconstruct = remaining_keyshares >= self.reconstruction_threshold;

        // Check if we still have manifest access
        let remaining_manifests = self.manifest_count.saturating_sub(failure_count);
        let has_manifest_access = remaining_manifests > 0;

        can_reconstruct && has_manifest_access
    }

    /// Get recommended candidate selection strategy
    pub fn get_selection_strategy(&self) -> CandidateSelectionStrategy {
        CandidateSelectionStrategy {
            keyshare_selection: SelectionMethod::FirstN(self.keyshare_count),
            manifest_selection: if self.manifest_count == self.keyshare_count {
                SelectionMethod::SameAsKeyshares
            } else {
                SelectionMethod::FirstN(self.manifest_count)
            },
        }
    }
}

/// Strategy for selecting which candidates get what
#[derive(Debug, Clone)]
pub struct CandidateSelectionStrategy {
    pub keyshare_selection: SelectionMethod,
    pub manifest_selection: SelectionMethod,
}

#[derive(Debug, Clone)]
pub enum SelectionMethod {
    /// Select first N candidates
    FirstN(usize),
    /// Use same candidates as keyshares
    SameAsKeyshares,
    /// Distributed selection (future: could implement ring hashing, etc.)
    Distributed(usize),
}

/// Errors that can occur during distribution calculation
#[derive(Debug, Clone)]
pub enum DistributionError {
    InsufficientCandidates {
        available: usize,
        minimum_required: usize,
    },
    InvalidThreshold {
        threshold: usize,
        total_shares: usize,
    },
    SecurityLevelNotSupported,
}

impl std::fmt::Display for DistributionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DistributionError::InsufficientCandidates {
                available,
                minimum_required,
            } => {
                write!(
                    f,
                    "Insufficient candidates: have {}, need at least {}",
                    available, minimum_required
                )
            }
            DistributionError::InvalidThreshold {
                threshold,
                total_shares,
            } => {
                write!(
                    f,
                    "Invalid threshold: {} must be less than total shares {}",
                    threshold, total_shares
                )
            }
            DistributionError::SecurityLevelNotSupported => {
                write!(f, "Security level not supported")
            }
        }
    }
}

impl std::error::Error for DistributionError {}
