//! Re-exported constants from the protocol crate to maintain backwards compatibility.
//!
//! This module eliminates duplication by importing all constants from the protocol crate.

pub use protocol::libp2p_constants::*;

// Re-export commonly used constants with their original names for backwards compatibility
pub use protocol::libp2p_constants::{
    DEFAULT_LEASE_TTL_MS, DEFAULT_SELECTION_WINDOW_MS, LEASE_PREFIX,
    SCHEDULER_EVENTS_TOPIC as TOPIC_EVENTS, SCHEDULER_PROPOSALS_TOPIC as TOPIC_PROPOSALS,
    SCHEDULER_TASKS_TOPIC as TOPIC_TASKS,
};
