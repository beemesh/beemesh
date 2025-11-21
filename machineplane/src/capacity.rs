//! Capacity verification module for checking node resources
//!
//! This module provides functionality to verify whether a node has sufficient
//! resources to host requested workloads. It collects system metrics and
//! compares them against workload requests while respecting safety margins.

use crate::protocol::libp2p_constants::{
    DEFAULT_CPU_REQUEST_MILLI, DEFAULT_MEMORY_REQUEST_BYTES, DEFAULT_STORAGE_REQUEST_BYTES,
    MAX_CPU_ALLOCATION_PERCENT, MAX_MEMORY_ALLOCATION_PERCENT, MAX_STORAGE_ALLOCATION_PERCENT,
    MAX_WORKLOADS_PER_NODE, MIN_FREE_MEMORY_BYTES, MIN_FREE_STORAGE_BYTES,
};
use anyhow::anyhow;
use log::{debug, info, warn};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// System resource information collected from the host
#[derive(Debug, Clone)]
pub struct SystemResources {
    /// Total CPU cores available on the system
    pub total_cpu_cores: u32,
    /// Total CPU in millicores (cores * 1000)
    pub total_cpu_milli: u32,
    /// Total memory in bytes
    pub total_memory_bytes: u64,
    /// Total storage in bytes (root partition)
    pub total_storage_bytes: u64,
    /// Currently allocated CPU in millicores
    pub allocated_cpu_milli: u32,
    /// Currently allocated memory in bytes
    pub allocated_memory_bytes: u64,
    /// Currently allocated storage in bytes
    pub allocated_storage_bytes: u64,
    /// Number of currently running workloads
    pub running_workloads: u32,
}

impl SystemResources {
    /// Calculate available CPU in millicores respecting allocation limits
    pub fn available_cpu_milli(&self) -> u32 {
        let max_allocatable =
            (self.total_cpu_milli as u64 * MAX_CPU_ALLOCATION_PERCENT as u64 / 100) as u32;
        max_allocatable.saturating_sub(self.allocated_cpu_milli)
    }

    /// Calculate available memory in bytes respecting allocation limits
    pub fn available_memory_bytes(&self) -> u64 {
        let max_allocatable = self.total_memory_bytes * MAX_MEMORY_ALLOCATION_PERCENT as u64 / 100;
        let max_allocatable = max_allocatable.saturating_sub(MIN_FREE_MEMORY_BYTES);
        max_allocatable.saturating_sub(self.allocated_memory_bytes)
    }

    /// Calculate available storage in bytes respecting allocation limits
    pub fn available_storage_bytes(&self) -> u64 {
        let max_allocatable =
            self.total_storage_bytes * MAX_STORAGE_ALLOCATION_PERCENT as u64 / 100;
        let max_allocatable = max_allocatable.saturating_sub(MIN_FREE_STORAGE_BYTES);
        max_allocatable.saturating_sub(self.allocated_storage_bytes)
    }

    /// Check if the node can accept more workloads based on workload count limit
    pub fn can_accept_more_workloads(&self) -> bool {
        if MAX_WORKLOADS_PER_NODE == 0 {
            return true; // No limit
        }
        self.running_workloads < MAX_WORKLOADS_PER_NODE
    }
}

/// A workload resource request
#[derive(Debug, Clone)]
pub struct ResourceRequest {
    /// Requested CPU in millicores per replica
    pub cpu_milli: u32,
    /// Requested memory in bytes per replica
    pub memory_bytes: u64,
    /// Requested storage in bytes per replica
    pub storage_bytes: u64,
    /// Number of replicas to deploy
    pub replicas: u32,
}

impl ResourceRequest {
    /// Create a new resource request with defaults for unspecified values
    pub fn new(
        cpu_milli: Option<u32>,
        memory_bytes: Option<u64>,
        storage_bytes: Option<u64>,
        replicas: u32,
    ) -> Self {
        Self {
            cpu_milli: cpu_milli.unwrap_or(DEFAULT_CPU_REQUEST_MILLI),
            memory_bytes: memory_bytes.unwrap_or(DEFAULT_MEMORY_REQUEST_BYTES),
            storage_bytes: storage_bytes.unwrap_or(DEFAULT_STORAGE_REQUEST_BYTES),
            replicas: replicas.max(1), // At least 1 replica
        }
    }

    /// Calculate total CPU required for all replicas
    pub fn total_cpu_milli(&self) -> u32 {
        self.cpu_milli.saturating_mul(self.replicas)
    }

    /// Calculate total memory required for all replicas
    pub fn total_memory_bytes(&self) -> u64 {
        self.memory_bytes.saturating_mul(self.replicas as u64)
    }

    /// Calculate total storage required for all replicas
    pub fn total_storage_bytes(&self) -> u64 {
        self.storage_bytes.saturating_mul(self.replicas as u64)
    }
}

/// Result of a capacity verification check
#[derive(Debug, Clone)]
pub struct CapacityCheckResult {
    /// Whether the node has sufficient capacity
    pub has_capacity: bool,
    /// Reason for rejection if has_capacity is false
    pub rejection_reason: Option<String>,
    /// Available resources at time of check
    pub available_cpu_milli: u32,
    pub available_memory_bytes: u64,
    pub available_storage_bytes: u64,
}

/// Capacity verifier for checking node resources
pub struct CapacityVerifier {
    /// Cached system resources
    system_resources: Arc<RwLock<SystemResources>>,
    /// Active short-lived reservations created after bidding
    reservations: Arc<RwLock<HashMap<String, CapacityReservation>>>,
}

#[derive(Debug, Clone)]
struct CapacityReservation {
    cpu_milli: u32,
    memory_bytes: u64,
    storage_bytes: u64,
    manifest_id: Option<String>,
    expires_at: Instant,
}

const RESERVATION_HOLD_SECS: u64 = 3;

impl CapacityVerifier {
    /// Create a new capacity verifier
    pub fn new() -> Self {
        Self {
            system_resources: Arc::new(RwLock::new(SystemResources {
                total_cpu_cores: 0,
                total_cpu_milli: 0,
                total_memory_bytes: 0,
                total_storage_bytes: 0,
                allocated_cpu_milli: 0,
                allocated_memory_bytes: 0,
                allocated_storage_bytes: 0,
                running_workloads: 0,
            })),
            reservations: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    fn reservation_hold_duration() -> Duration {
        Duration::from_secs(RESERVATION_HOLD_SECS)
    }

    async fn prune_expired_reservations(&self) {
        let mut reservations = self.reservations.write().await;
        let now = Instant::now();
        reservations.retain(|_, reservation| reservation.expires_at > now);
    }

    async fn total_reserved_resources(&self) -> (u32, u64, u64) {
        let reservations = self.reservations.read().await;
        reservations
            .values()
            .fold((0u32, 0u64, 0u64), |acc, reservation| {
                (
                    acc.0.saturating_add(reservation.cpu_milli),
                    acc.1.saturating_add(reservation.memory_bytes),
                    acc.2.saturating_add(reservation.storage_bytes),
                )
            })
    }

    /// Reserve capacity for a request for a limited time window. Returns Ok when the reservation was
    /// accepted or refreshed, and Err when the resources are no longer available.
    pub async fn reserve_capacity(
        &self,
        request_id: &str,
        manifest_id: Option<&str>,
        request: &ResourceRequest,
    ) -> anyhow::Result<()> {
        self.prune_expired_reservations().await;

        {
            let mut reservations = self.reservations.write().await;
            if let Some(existing) = reservations.get_mut(request_id) {
                existing.expires_at = Instant::now() + Self::reservation_hold_duration();
                if let Some(manifest) = manifest_id {
                    existing.manifest_id = Some(manifest.to_string());
                }
                return Ok(());
            }
        }

        let (reserved_cpu, reserved_memory, reserved_storage) =
            self.total_reserved_resources().await;

        let request_cpu = request.total_cpu_milli();
        let request_memory = request.total_memory_bytes();
        let request_storage = request.total_storage_bytes();

        {
            let resources = self.system_resources.read().await;
            let available_cpu = resources.available_cpu_milli().saturating_sub(reserved_cpu);
            let available_memory = resources
                .available_memory_bytes()
                .saturating_sub(reserved_memory);
            let available_storage = resources
                .available_storage_bytes()
                .saturating_sub(reserved_storage);

            if request_cpu > available_cpu {
                return Err(anyhow!(
                    "Insufficient CPU reservation capacity: need {}m, reservable {}m",
                    request_cpu,
                    available_cpu
                ));
            }
            if request_memory > available_memory {
                return Err(anyhow!(
                    "Insufficient memory reservation capacity: need {} MB, reservable {} MB",
                    request_memory / (1024 * 1024),
                    available_memory / (1024 * 1024)
                ));
            }
            if request_storage > available_storage {
                return Err(anyhow!(
                    "Insufficient storage reservation capacity: need {} GB, reservable {} GB",
                    request_storage / (1024 * 1024 * 1024),
                    available_storage / (1024 * 1024 * 1024)
                ));
            }
        }

        let mut reservations = self.reservations.write().await;
        if let Some(existing) = reservations.get_mut(request_id) {
            existing.expires_at = Instant::now() + Self::reservation_hold_duration();
            if let Some(manifest) = manifest_id {
                existing.manifest_id = Some(manifest.to_string());
            }
            return Ok(());
        }

        reservations.insert(
            request_id.to_string(),
            CapacityReservation {
                cpu_milli: request_cpu,
                memory_bytes: request_memory,
                storage_bytes: request_storage,
                manifest_id: manifest_id.map(|m| m.to_string()),
                expires_at: Instant::now() + Self::reservation_hold_duration(),
            },
        );

        Ok(())
    }

    /// Check whether a manifest has an active reservation created by a prior capacity reply.
    pub async fn has_active_reservation_for_manifest(&self, manifest_id: &str) -> bool {
        self.prune_expired_reservations().await;
        let reservations = self.reservations.read().await;
        reservations
            .values()
            .any(|reservation| reservation.manifest_id.as_deref() == Some(manifest_id))
    }

    /// Update system resources by collecting metrics from the host
    pub async fn update_system_resources(&self) -> Result<(), String> {
        let mut resources = self.system_resources.write().await;

        // Collect CPU information
        match Self::get_cpu_info() {
            Ok((cores, milli)) => {
                resources.total_cpu_cores = cores;
                resources.total_cpu_milli = milli;
            }
            Err(e) => {
                warn!("Failed to get CPU info: {}", e);
                // Use defaults if unable to detect
                resources.total_cpu_cores = 4;
                resources.total_cpu_milli = 4000;
            }
        }

        // Collect memory information
        match Self::get_memory_info() {
            Ok(total_mem) => {
                resources.total_memory_bytes = total_mem;
            }
            Err(e) => {
                warn!("Failed to get memory info: {}", e);
                // Use default 8GB if unable to detect
                resources.total_memory_bytes = 8 * 1024 * 1024 * 1024;
            }
        }

        // Collect storage information
        match Self::get_storage_info() {
            Ok(total_storage) => {
                resources.total_storage_bytes = total_storage;
            }
            Err(e) => {
                warn!("Failed to get storage info: {}", e);
                // Use default 100GB if unable to detect
                resources.total_storage_bytes = 100 * 1024 * 1024 * 1024;
            }
        }

        // Update allocated resources from workload manager
        self.update_allocated_resources(&mut resources).await;

        info!(
            "System resources updated: CPU={} cores ({}m), Memory={} MB, Storage={} GB, Workloads={}",
            resources.total_cpu_cores,
            resources.total_cpu_milli,
            resources.total_memory_bytes / (1024 * 1024),
            resources.total_storage_bytes / (1024 * 1024 * 1024),
            resources.running_workloads
        );

        Ok(())
    }

    /// Update allocated resources by querying the runtime registry
    async fn update_allocated_resources(&self, resources: &mut SystemResources) {
        // Query the global runtime registry to get current workload allocations
        if let Some(registry_guard) =
            crate::run::get_global_runtime_registry().await
        {
            if let Some(registry) = registry_guard.as_ref() {
                let mut total_cpu = 0u32;
                let mut total_memory = 0u64;
                let mut total_storage = 0u64;
                let mut workload_count = 0u32;

                if let Some(engine) = registry.get_default_engine() {
                    if let Ok(workloads) = engine.list_workloads().await {
                        for workload in workloads {
                            workload_count += 1;

                            // Extract resource allocations from workload metadata
                            if let Some(cpu_str) = workload.metadata.get("cpu_milli") {
                                if let Ok(cpu) = cpu_str.parse::<u32>() {
                                    total_cpu = total_cpu.saturating_add(cpu);
                                }
                            }
                            if let Some(mem_str) = workload.metadata.get("memory_bytes") {
                                if let Ok(mem) = mem_str.parse::<u64>() {
                                    total_memory = total_memory.saturating_add(mem);
                                }
                            }
                            if let Some(storage_str) = workload.metadata.get("storage_bytes") {
                                if let Ok(storage) = storage_str.parse::<u64>() {
                                    total_storage = total_storage.saturating_add(storage);
                                }
                            }
                        }
                    }
                }

                resources.allocated_cpu_milli = total_cpu;
                resources.allocated_memory_bytes = total_memory;
                resources.allocated_storage_bytes = total_storage;
                resources.running_workloads = workload_count;
            } else {
                debug!("Runtime registry not initialized for resource tracking");
            }
        } else {
            debug!("Runtime registry not available for resource tracking");
        }
    }

    /// Get CPU information from the system (Linux only)
    fn get_cpu_info() -> Result<(u32, u32), String> {
        use std::fs;
        let cpuinfo = fs::read_to_string("/proc/cpuinfo")
            .map_err(|e| format!("Failed to read /proc/cpuinfo: {}", e))?;

        let num_cpus = cpuinfo
            .lines()
            .filter(|line| line.starts_with("processor"))
            .count() as u32;

        if num_cpus == 0 {
            return Err("No processors found in /proc/cpuinfo".to_string());
        }

        let milli = num_cpus * 1000;
        Ok((num_cpus, milli))
    }

    /// Get memory information from the system (Linux only)
    fn get_memory_info() -> Result<u64, String> {
        use std::fs;
        let meminfo = fs::read_to_string("/proc/meminfo")
            .map_err(|e| format!("Failed to read /proc/meminfo: {}", e))?;

        for line in meminfo.lines() {
            if line.starts_with("MemTotal:") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    let kb = parts[1]
                        .parse::<u64>()
                        .map_err(|e| format!("Failed to parse memory value: {}", e))?;
                    return Ok(kb * 1024); // Convert KB to bytes
                }
            }
        }
        Err("MemTotal not found in /proc/meminfo".to_string())
    }

    /// Get storage information from the system (Linux only)
    fn get_storage_info() -> Result<u64, String> {
        use std::process::Command;
        let output = Command::new("df")
            .args(&["-B1", "/var/lib/containers/storage"])
            .output()
            .map_err(|e| format!("Failed to execute df: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().collect();

        if lines.len() >= 2 {
            let parts: Vec<&str> = lines[1].split_whitespace().collect();
            if parts.len() >= 2 {
                let bytes = parts[1]
                    .parse::<u64>()
                    .map_err(|e| format!("Failed to parse storage value: {}", e))?;
                return Ok(bytes);
            }
        }
        Err("Failed to parse df output".to_string())
    }

    /// Verify if the node has capacity for a workload request
    pub async fn verify_capacity(&self, request: &ResourceRequest) -> CapacityCheckResult {
        self.prune_expired_reservations().await;
        let (reserved_cpu, reserved_memory, reserved_storage) =
            self.total_reserved_resources().await;

        let resources = self.system_resources.read().await;

        let available_cpu = resources.available_cpu_milli().saturating_sub(reserved_cpu);
        let available_memory = resources
            .available_memory_bytes()
            .saturating_sub(reserved_memory);
        let available_storage = resources
            .available_storage_bytes()
            .saturating_sub(reserved_storage);

        let total_cpu_needed = request.total_cpu_milli();
        let total_memory_needed = request.total_memory_bytes();
        let total_storage_needed = request.total_storage_bytes();

        debug!(
            "Capacity check: Request(CPU={}m, Mem={} MB, Storage={} GB, Replicas={}) vs Available(CPU={}m, Mem={} MB, Storage={} GB)",
            total_cpu_needed,
            total_memory_needed / (1024 * 1024),
            total_storage_needed / (1024 * 1024 * 1024),
            request.replicas,
            available_cpu,
            available_memory / (1024 * 1024),
            available_storage / (1024 * 1024 * 1024)
        );

        // Check CPU capacity
        if total_cpu_needed > available_cpu {
            return CapacityCheckResult {
                has_capacity: false,
                rejection_reason: Some(format!(
                    "Insufficient CPU: need {}m, available {}m",
                    total_cpu_needed, available_cpu
                )),
                available_cpu_milli: available_cpu,
                available_memory_bytes: available_memory,
                available_storage_bytes: available_storage,
            };
        }

        // Check memory capacity
        if total_memory_needed > available_memory {
            return CapacityCheckResult {
                has_capacity: false,
                rejection_reason: Some(format!(
                    "Insufficient memory: need {} MB, available {} MB",
                    total_memory_needed / (1024 * 1024),
                    available_memory / (1024 * 1024)
                )),
                available_cpu_milli: available_cpu,
                available_memory_bytes: available_memory,
                available_storage_bytes: available_storage,
            };
        }

        // Check storage capacity
        if total_storage_needed > available_storage {
            return CapacityCheckResult {
                has_capacity: false,
                rejection_reason: Some(format!(
                    "Insufficient storage: need {} GB, available {} GB",
                    total_storage_needed / (1024 * 1024 * 1024),
                    available_storage / (1024 * 1024 * 1024)
                )),
                available_cpu_milli: available_cpu,
                available_memory_bytes: available_memory,
                available_storage_bytes: available_storage,
            };
        }

        // Check workload count limit
        if !resources.can_accept_more_workloads() {
            return CapacityCheckResult {
                has_capacity: false,
                rejection_reason: Some(format!(
                    "Maximum workload count reached: {} / {}",
                    resources.running_workloads, MAX_WORKLOADS_PER_NODE
                )),
                available_cpu_milli: available_cpu,
                available_memory_bytes: available_memory,
                available_storage_bytes: available_storage,
            };
        }

        // All checks passed
        CapacityCheckResult {
            has_capacity: true,
            rejection_reason: None,
            available_cpu_milli: available_cpu,
            available_memory_bytes: available_memory,
            available_storage_bytes: available_storage,
        }
    }

    /// Get current system resources (read-only access)
    pub async fn get_system_resources(&self) -> SystemResources {
        self.system_resources.read().await.clone()
    }
}

impl Default for CapacityVerifier {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn test_resource_request_totals() {
        let request = ResourceRequest::new(
            Some(100),
            Some(256 * 1024 * 1024),
            Some(1024 * 1024 * 1024),
            3,
        );

        assert_eq!(request.total_cpu_milli(), 300);
        assert_eq!(request.total_memory_bytes(), 3 * 256 * 1024 * 1024);
        assert_eq!(request.total_storage_bytes(), 3 * 1024 * 1024 * 1024);
    }

    #[test]
    fn test_resource_request_defaults() {
        let request = ResourceRequest::new(None, None, None, 1);

        assert_eq!(request.cpu_milli, DEFAULT_CPU_REQUEST_MILLI);
        assert_eq!(request.memory_bytes, DEFAULT_MEMORY_REQUEST_BYTES);
        assert_eq!(request.storage_bytes, DEFAULT_STORAGE_REQUEST_BYTES);
    }

    #[test]
    fn test_system_resources_available_cpu() {
        let resources = SystemResources {
            total_cpu_cores: 4,
            total_cpu_milli: 4000,
            total_memory_bytes: 8 * 1024 * 1024 * 1024,
            total_storage_bytes: 100 * 1024 * 1024 * 1024,
            allocated_cpu_milli: 1000,
            allocated_memory_bytes: 2 * 1024 * 1024 * 1024,
            allocated_storage_bytes: 10 * 1024 * 1024 * 1024,
            running_workloads: 2,
        };

        // 4000m * 90% = 3600m max allocatable
        // 3600m - 1000m allocated = 2600m available
        assert_eq!(resources.available_cpu_milli(), 2600);
    }

    #[test]
    fn test_system_resources_available_memory() {
        let resources = SystemResources {
            total_cpu_cores: 4,
            total_cpu_milli: 4000,
            total_memory_bytes: 8 * 1024 * 1024 * 1024, // 8 GB
            total_storage_bytes: 100 * 1024 * 1024 * 1024,
            allocated_cpu_milli: 1000,
            allocated_memory_bytes: 2 * 1024 * 1024 * 1024, // 2 GB allocated
            allocated_storage_bytes: 10 * 1024 * 1024 * 1024,
            running_workloads: 2,
        };

        // 8GB * 90% = 7.2GB max allocatable
        // 7.2GB - 512MB min free = 6.7GB
        // 6.7GB - 2GB allocated = 4.7GB available
        let available = resources.available_memory_bytes();
        let expected =
            (8 * 1024 * 1024 * 1024 * 90 / 100) - MIN_FREE_MEMORY_BYTES - (2 * 1024 * 1024 * 1024);
        assert_eq!(available, expected);
    }

    #[tokio::test]
    async fn test_capacity_verification_sufficient() {
        let verifier = CapacityVerifier::new();

        // Set up system resources
        {
            let mut resources = verifier.system_resources.write().await;
            resources.total_cpu_cores = 4;
            resources.total_cpu_milli = 4000;
            resources.total_memory_bytes = 8 * 1024 * 1024 * 1024;
            resources.total_storage_bytes = 100 * 1024 * 1024 * 1024;
            resources.allocated_cpu_milli = 0;
            resources.allocated_memory_bytes = 0;
            resources.allocated_storage_bytes = 0;
            resources.running_workloads = 0;
        }

        let request = ResourceRequest::new(
            Some(500),
            Some(1024 * 1024 * 1024),
            Some(5 * 1024 * 1024 * 1024),
            2,
        );

        let result = verifier.verify_capacity(&request).await;
        assert!(result.has_capacity, "Should have sufficient capacity");
        assert!(result.rejection_reason.is_none());
    }

    #[tokio::test]
    async fn test_capacity_verification_insufficient_cpu() {
        let verifier = CapacityVerifier::new();

        {
            let mut resources = verifier.system_resources.write().await;
            resources.total_cpu_cores = 2;
            resources.total_cpu_milli = 2000;
            resources.total_memory_bytes = 8 * 1024 * 1024 * 1024;
            resources.total_storage_bytes = 100 * 1024 * 1024 * 1024;
            resources.allocated_cpu_milli = 1500;
            resources.allocated_memory_bytes = 0;
            resources.allocated_storage_bytes = 0;
            resources.running_workloads = 1;
        }

        let request = ResourceRequest::new(
            Some(1000),
            Some(1024 * 1024 * 1024),
            Some(5 * 1024 * 1024 * 1024),
            1,
        );

        let result = verifier.verify_capacity(&request).await;
        assert!(!result.has_capacity, "Should not have sufficient CPU");
        assert!(result.rejection_reason.is_some());
        assert!(
            result
                .rejection_reason
                .unwrap()
                .contains("Insufficient CPU")
        );
    }

    #[tokio::test]
    async fn test_capacity_verification_insufficient_memory() {
        let verifier = CapacityVerifier::new();

        {
            let mut resources = verifier.system_resources.write().await;
            resources.total_cpu_cores = 4;
            resources.total_cpu_milli = 4000;
            resources.total_memory_bytes = 2 * 1024 * 1024 * 1024; // Only 2 GB
            resources.total_storage_bytes = 100 * 1024 * 1024 * 1024;
            resources.allocated_cpu_milli = 0;
            resources.allocated_memory_bytes = 1024 * 1024 * 1024; // 1 GB allocated
            resources.allocated_storage_bytes = 0;
            resources.running_workloads = 1;
        }

        let request = ResourceRequest::new(
            Some(500),
            Some(2 * 1024 * 1024 * 1024), // Request 2 GB
            Some(5 * 1024 * 1024 * 1024),
            1,
        );

        let result = verifier.verify_capacity(&request).await;
        assert!(!result.has_capacity, "Should not have sufficient memory");
        assert!(result.rejection_reason.is_some());
        assert!(
            result
                .rejection_reason
                .unwrap()
                .contains("Insufficient memory")
        );
    }

    #[tokio::test]
    async fn test_reserve_capacity_reduces_available_resources() {
        let verifier = CapacityVerifier::new();

        {
            let mut resources = verifier.system_resources.write().await;
            resources.total_cpu_cores = 8;
            resources.total_cpu_milli = 8000;
            resources.total_memory_bytes = 4 * 1024 * 1024 * 1024;
            resources.total_storage_bytes = 16 * 1024 * 1024 * 1024;
            resources.allocated_cpu_milli = 0;
            resources.allocated_memory_bytes = 0;
            resources.allocated_storage_bytes = 0;
            resources.running_workloads = 0;
        }

        let snapshot = verifier.get_system_resources().await;
        let request = ResourceRequest::new(
            Some(snapshot.available_cpu_milli()),
            Some(snapshot.available_memory_bytes()),
            Some(snapshot.available_storage_bytes()),
            1,
        );

        let initial = verifier.verify_capacity(&request).await;
        assert!(
            initial.has_capacity,
            "Expected initial capacity to be available"
        );

        verifier
            .reserve_capacity("req-test", Some("manifest-test"), &request)
            .await
            .expect("reservation should succeed");

        assert!(
            verifier
                .has_active_reservation_for_manifest("manifest-test")
                .await
        );

        let follow_up = verifier.verify_capacity(&request).await;
        assert!(
            !follow_up.has_capacity,
            "Reservation should reduce available capacity for identical request"
        );
    }

    #[tokio::test]
    async fn test_reservation_expires_after_hold() {
        let verifier = CapacityVerifier::new();

        {
            let mut resources = verifier.system_resources.write().await;
            resources.total_cpu_cores = 8;
            resources.total_cpu_milli = 8000;
            resources.total_memory_bytes = 4 * 1024 * 1024 * 1024;
            resources.total_storage_bytes = 16 * 1024 * 1024 * 1024;
            resources.allocated_cpu_milli = 0;
            resources.allocated_memory_bytes = 0;
            resources.allocated_storage_bytes = 0;
            resources.running_workloads = 0;
        }

        let request = ResourceRequest::new(
            Some(1000),
            Some(512 * 1024 * 1024),
            Some(2 * 1024 * 1024 * 1024),
            1,
        );

        verifier
            .reserve_capacity("req-expire", Some("manifest-expire"), &request)
            .await
            .expect("reservation should succeed");

        {
            let mut reservations = verifier.reservations.write().await;
            if let Some(reservation) = reservations.get_mut("req-expire") {
                reservation.expires_at = Instant::now() - Duration::from_secs(1);
            }
        }

        assert!(
            !verifier
                .has_active_reservation_for_manifest("manifest-expire")
                .await,
            "Expired reservation should no longer be reported as active"
        );
    }
}
