//! # Metrics – Simple Counter and Gauge Recording
//!
//! This module provides a minimal metrics recording system for workplane telemetry.
//! It exports two macros:
//!
//! - [`increment_counter!`] – Increment a named counter
//! - [`gauge!`] – Set a named gauge value
//!
//! ## Usage
//!
//! ```ignore
//! use workplane::{increment_counter, gauge};
//!
//! // Increment counters
//! increment_counter!("workplane.requests.total");
//! increment_counter!("workplane.errors.total", 5);
//!
//! // Set gauges
//! gauge!("workplane.replicas.healthy", 3.0);
//! gauge!("workplane.reconciliation.failures", 0.0);
//! ```
//!
//! ## Metrics Keys
//!
//! | Key                                    | Type    | Description                          |
//! |----------------------------------------|---------|--------------------------------------|
//! | `workplane.replicas.healthy`           | Gauge   | Current healthy replica count        |
//! | `workplane.replicas.unhealthy`         | Gauge   | Current unhealthy replica count      |
//! | `workplane.reconciliation.failures`    | Counter | Reconciliation failure count         |
//! | `workplane.reconciliation.scale_up`    | Counter | Scale-up events                      |
//! | `workplane.reconciliation.scale_down`  | Counter | Scale-down events                    |
//! | `workplane.raft.follower_rejections`   | Counter | Rejected leader_only requests        |
//! | `workplane.network.leader_advertisements` | Counter | Leader endpoint publications      |
//! | `workplane.network.leader_withdrawals` | Counter | Leader endpoint withdrawals          |
//!
//! ## Thread Safety
//!
//! The registry is protected by a `Mutex` and can be safely accessed from multiple
//! tokio tasks. Values are stored in a simple `HashMap<&'static str, f64>`.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Get or initialize the global metrics registry.
fn registry() -> &'static Mutex<HashMap<&'static str, f64>> {
    static REGISTRY: OnceLock<Mutex<HashMap<&'static str, f64>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Mode for storing metric values.
#[derive(Clone, Copy)]
enum StoreMode {
    /// Add to existing value (counter behavior).
    Counter,
    /// Replace existing value (gauge behavior).
    Gauge,
}

/// Store a metric value in the registry.
fn store_value(key: &'static str, value: f64, mode: StoreMode) {
    let mut guard = registry().lock().expect("metrics registry mutex poisoned");
    match mode {
        StoreMode::Counter => {
            let entry = guard.entry(key).or_insert(0.0);
            *entry += value;
        }
        StoreMode::Gauge => {
            guard.insert(key, value);
        }
    }
}

/// Internal function for incrementing counters.
///
/// Called by the [`increment_counter!`] macro.
#[doc(hidden)]
pub fn increment_counter_internal(name: &'static str, value: f64) {
    store_value(name, value, StoreMode::Counter);
}

/// Internal function for setting gauges.
///
/// Called by the [`gauge!`] macro.
#[doc(hidden)]
pub fn gauge_internal(name: &'static str, value: f64) {
    store_value(name, value, StoreMode::Gauge);
}

/// Increment a counter metric.
///
/// Counters are cumulative values that only increase. Use for counting events.
///
/// # Examples
///
/// ```ignore
/// increment_counter!("workplane.requests.total");        // Increment by 1
/// increment_counter!("workplane.errors.total", 5);       // Increment by 5
/// ```
#[macro_export]
macro_rules! increment_counter {
    ($name:expr) => {
        $crate::metrics::increment_counter_internal($name, 1.0);
    };
    ($name:expr, $value:expr) => {
        $crate::metrics::increment_counter_internal($name, $value as f64);
    };
}

/// Set a gauge metric value.
///
/// Gauges are point-in-time values that can go up or down. Use for current state.
///
/// # Examples
///
/// ```ignore
/// gauge!("workplane.replicas.healthy", 3.0);
/// gauge!("workplane.memory.bytes", 1024 * 1024);
/// ```
#[macro_export]
macro_rules! gauge {
    ($name:expr, $value:expr) => {
        $crate::metrics::gauge_internal($name, $value as f64);
    };
}
