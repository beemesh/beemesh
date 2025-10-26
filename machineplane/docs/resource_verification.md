# Resource Verification Proposal

## Overview

This document describes the resource verification system implemented in the machine crate. The system verifies whether a node has sufficient resources to host requested workloads before accepting them for scheduling.

## Problem Statement

Previously, the machine crate always returned `true` for capacity requests (see `scheduler_message.rs:39`), accepting all workloads regardless of available resources. This could lead to:

- Resource exhaustion on nodes
- Failed workload deployments
- Poor scheduling decisions
- Node instability

## Solution Architecture

### Components

1. **ResourceVerifier** (`machine/src/resource_verifier.rs`)
   - Core verification logic
   - System resource collection
   - Capacity checking algorithms

2. **Protocol Constants** (`protocol/src/libp2p_constants.rs`)
   - Resource allocation limits
   - Default values
   - Safety margins

3. **Integration** (`machine/src/libp2p_beemesh/behaviour/scheduler_message.rs`)
   - Scheduler message handler integration
   - **Silent drop behavior**: Only responds when capacity is available

## Capacity Response Behavior

**Important**: The scheduler message handler only sends a response when the node has sufficient capacity. If capacity is unavailable, the request is silently dropped without sending any response.

This design allows the scheduler to:
- Collect responses only from nodes with capacity
- Use a timeout-based approach to gather available nodes
- Avoid processing explicit rejection messages
- Naturally filter out overloaded nodes

```rust
if has_capacity {
    // Send CapacityReply with ok=true and available resources
    send_response(channel, capacity_reply);
} else {
    // Silently drop the request - no response sent
    log_info("Capacity unavailable - not sending response");
}
```

## Metrics Included

### System-Level Metrics

#### CPU
- **Total CPU Cores**: Number of physical/logical CPU cores from `/proc/cpuinfo`
- **Total CPU Millicores**: Total cores × 1000 (Kubernetes-style millicores)
- **Allocated CPU Millicores**: Sum of CPU allocated to running workloads
- **Available CPU Millicores**: `(total × MAX_CPU_ALLOCATION_PERCENT) - allocated`

#### Memory
- **Total Memory Bytes**: Total system memory from `/proc/meminfo` (MemTotal)
- **Allocated Memory Bytes**: Sum of memory allocated to running workloads
- **Available Memory Bytes**: `(total × MAX_MEMORY_ALLOCATION_PERCENT) - MIN_FREE_MEMORY_BYTES - allocated`

#### Storage
- **Total Storage Bytes**: Root partition size from `df -B1 /`
- **Allocated Storage Bytes**: Sum of storage allocated to running workloads
- **Available Storage Bytes**: `(total × MAX_STORAGE_ALLOCATION_PERCENT) - MIN_FREE_STORAGE_BYTES - allocated`

#### Workload Count
- **Running Workloads**: Number of currently running workloads
- **Max Workloads Per Node**: Optional limit on total workloads (0 = unlimited)

### Per-Workload Request Metrics

When a workload is requested, the following are evaluated:

- **CPU Millicores**: Requested CPU per replica (default: 100m if not specified)
- **Memory Bytes**: Requested memory per replica (default: 128 MB if not specified)
- **Storage Bytes**: Requested storage per replica (default: 1 GB if not specified)
- **Replicas**: Number of instances to deploy (minimum: 1)

### Calculated Totals

For each workload request:
- **Total CPU Required**: `cpu_milli × replicas`
- **Total Memory Required**: `memory_bytes × replicas`
- **Total Storage Required**: `storage_bytes × replicas`

## Resource Allocation Constants

All constants are defined in `protocol/src/libp2p_constants.rs`:

| Constant | Value | Purpose |
|----------|-------|---------|
| `MAX_CPU_ALLOCATION_PERCENT` | 90% | Maximum CPU that can be allocated (10% headroom) |
| `MAX_MEMORY_ALLOCATION_PERCENT` | 90% | Maximum memory that can be allocated (10% headroom) |
| `MAX_STORAGE_ALLOCATION_PERCENT` | 90% | Maximum storage that can be allocated (10% headroom) |
| `MIN_FREE_MEMORY_BYTES` | 512 MB | Minimum memory to keep free for system operations |
| `MIN_FREE_STORAGE_BYTES` | 1 GB | Minimum storage to keep free for system operations |
| `MAX_WORKLOADS_PER_NODE` | 0 | Maximum workloads per node (0 = unlimited) |
| `RESOURCE_CHECK_TIMEOUT_MS` | 1000 ms | Timeout for resource availability checks |
| `DEFAULT_CPU_REQUEST_MILLI` | 100m | Default CPU if not specified in manifest |
| `DEFAULT_MEMORY_REQUEST_BYTES` | 128 MB | Default memory if not specified in manifest |
| `DEFAULT_STORAGE_REQUEST_BYTES` | 1 GB | Default storage if not specified in manifest |

## Verification Logic

### Capacity Check Algorithm

```rust
fn verify_capacity(request: ResourceRequest) -> CapacityCheckResult {
    1. Calculate available resources:
       - available_cpu = (total_cpu × 90%) - allocated_cpu
       - available_memory = (total_memory × 90%) - 512MB - allocated_memory
       - available_storage = (total_storage × 90%) - 1GB - allocated_storage
    
    2. Calculate total requirements:
       - total_cpu_needed = request.cpu_milli × request.replicas
       - total_memory_needed = request.memory_bytes × request.replicas
       - total_storage_needed = request.storage_bytes × request.replicas
    
    3. Verify each resource:
       if total_cpu_needed > available_cpu:
           return REJECTED (insufficient CPU)
       if total_memory_needed > available_memory:
           return REJECTED (insufficient memory)
       if total_storage_needed > available_storage:
           return REJECTED (insufficient storage)
       if MAX_WORKLOADS_PER_NODE > 0 && running_workloads >= MAX_WORKLOADS_PER_NODE:
           return REJECTED (workload limit reached)
    
    4. If accepted: send response with available metrics
       If rejected: silently drop request (no response)
}
```

### Safety Margins

The system implements multiple safety margins to prevent resource exhaustion:

1. **Percentage-based limits**: Only 90% of total resources can be allocated
2. **Minimum reserves**: Guarantees minimum free memory (512 MB) and storage (1 GB)
3. **Workload count limits**: Optional cap on number of workloads per node
4. **Saturating arithmetic**: All calculations use saturating operations to prevent overflow

## System Resource Collection

All resource collection is Linux-specific:

### CPU Detection
- Read `/proc/cpuinfo`
- Count lines starting with "processor"
- Convert to millicores (count × 1000)

### Memory Detection
- Read `/proc/meminfo`
- Parse "MemTotal:" line
- Convert from KB to bytes

### Storage Detection
- Execute `df -B1 /` command
- Parse output for root partition total size
- Returns total bytes available

### Workload Tracking
- Query global runtime registry
- Enumerate all running workloads
- Extract resource allocations from workload metadata
- Sum allocated resources

## Integration Points

### Initialization

The resource verifier is initialized during workload manager startup:

```rust
// In workload_integration::initialize_workload_manager()
let verifier = get_global_resource_verifier();
verifier.update_system_resources().await;
```

### Capacity Requests

When a scheduler sends a capacity request:

```rust
// In scheduler_message.rs
let resource_request = ResourceRequest::new(
    Some(cap_req.cpu_milli()),
    Some(cap_req.memory_bytes()),
    Some(cap_req.storage_bytes()),
    cap_req.replicas(),
);

let check_result = verifier.verify_capacity(&resource_request).await;

// Only send response if capacity is available
if check_result.has_capacity {
    build_capacity_reply_with(request_id, responder_peer, |params| {
        params.ok = true;
        params.cpu_milli = check_result.available_cpu_milli;
        params.memory_bytes = check_result.available_memory_bytes;
        params.storage_bytes = check_result.available_storage_bytes;
    });
    send_response(channel, reply);
}
// Otherwise, silently drop the request
```

### Capacity Reply Enhancement

When sent, the capacity reply includes actual available resources:

- `ok`: Always `true` (only sent when capacity is available)
- `cpu_available_milli`: Actual available CPU in millicores
- `memory_available_bytes`: Actual available memory in bytes
- `storage_available_bytes`: Actual available storage in bytes

## Example Scenarios

### Scenario 1: Sufficient Capacity (Response Sent)

**System State:**
- Total: 4 cores (4000m), 8 GB memory, 100 GB storage
- Allocated: 1000m CPU, 2 GB memory, 10 GB storage

**Request:**
- CPU: 500m per replica
- Memory: 1 GB per replica
- Storage: 5 GB per replica
- Replicas: 2

**Calculation:**
- Available CPU: (4000 × 90%) - 1000 = 2600m
- Available Memory: (8GB × 90%) - 512MB - 2GB = 4.7GB
- Available Storage: (100GB × 90%) - 1GB - 10GB = 79GB
- Required CPU: 500m × 2 = 1000m ✓
- Required Memory: 1GB × 2 = 2GB ✓
- Required Storage: 5GB × 2 = 10GB ✓

**Result:** ✅ **Response sent with ok=true**

### Scenario 2: Insufficient Memory (No Response)

**System State:**
- Total: 4 cores (4000m), 2 GB memory, 100 GB storage
- Allocated: 0m CPU, 1 GB memory, 0 GB storage

**Request:**
- CPU: 500m per replica
- Memory: 2 GB per replica
- Storage: 5 GB per replica
- Replicas: 1

**Calculation:**
- Available CPU: (4000 × 90%) - 0 = 3600m ✓
- Available Memory: (2GB × 90%) - 512MB - 1GB = 308MB
- Required Memory: 2GB × 1 = 2GB ✗

**Result:** ❌ **No response sent** (request silently dropped)

### Scenario 3: Multiple Replicas (No Response)

**System State:**
- Total: 8 cores (8000m), 16 GB memory, 500 GB storage
- Allocated: 2000m CPU, 4 GB memory, 50 GB storage

**Request:**
- CPU: 1000m per replica
- Memory: 2 GB per replica
- Storage: 10 GB per replica
- Replicas: 5

**Calculation:**
- Available CPU: (8000 × 90%) - 2000 = 5200m
- Required CPU: 1000m × 5 = 5000m ✓
- Available Memory: (16GB × 90%) - 512MB - 4GB = 9.9GB
- Required Memory: 2GB × 5 = 10GB ✗

**Result:** ❌ **No response sent** (insufficient memory)

## Testing

The resource verifier includes comprehensive unit tests:

- `test_resource_request_totals`: Validates total resource calculations
- `test_resource_request_defaults`: Verifies default value application
- `test_system_resources_available_cpu`: Tests CPU availability calculation
- `test_system_resources_available_memory`: Tests memory availability calculation
- `test_capacity_verification_sufficient`: Tests acceptance scenario
- `test_capacity_verification_insufficient_cpu`: Tests CPU rejection
- `test_capacity_verification_insufficient_memory`: Tests memory rejection

Run tests:
```bash
cargo test --package machine --lib resource_verifier
```

## Scheduler Integration Notes

The scheduler should implement a timeout-based collection pattern:

1. Broadcast capacity request to all nodes
2. Collect responses for a fixed duration (e.g., DEFAULT_SELECTION_WINDOW_MS)
3. Select from nodes that responded (have capacity)
4. Nodes without capacity won't respond, naturally excluding themselves

This approach is more efficient than processing explicit rejections and aligns with distributed system patterns where non-responsive nodes are naturally excluded.

## Future Enhancements

Potential improvements to consider:

1. **Network Bandwidth Tracking**: Monitor and limit network usage per workload
2. **GPU Resources**: Add GPU detection and allocation tracking
3. **Port Availability**: Verify available ports before accepting workloads
4. **Historical Metrics**: Track resource usage patterns over time
5. **Predictive Scaling**: Use historical data to predict resource needs
6. **Container Runtime Integration**: Query actual container resource usage
7. **Cgroup Integration**: Read actual resource limits from cgroups
8. **Dynamic Limits**: Adjust allocation percentages based on node load
9. **Priority Classes**: Allow higher-priority workloads to override limits
10. **Resource Reservation**: Support reserved resources for critical workloads

## Implementation Notes

### Thread Safety
- Uses `Arc<RwLock<>>` for shared state
- Read-heavy operations use read locks
- Updates use write locks

### Performance
- Resource updates are cached, not queried on every request
- Fast path for capacity checks (no I/O during verification)
- Spawns separate thread to avoid "runtime within runtime" issues

### Threading Model
The capacity check in `scheduler_message.rs` uses a special threading approach to avoid blocking the tokio runtime:

```rust
// Spawn a separate thread to perform the async operation
std::thread::spawn(move || {
    handle.block_on(verifier.verify_capacity(&resource_request))
})
.join()
```

This is necessary because:
1. `scheduler_message()` is called from within libp2p's event loop (already in tokio runtime)
2. We cannot call `block_on()` directly from within an async runtime
3. By spawning a new OS thread, we can safely call `block_on()` without nesting runtimes
4. The thread join ensures we wait for the result before sending the response

### Linux-Specific Code
- All resource detection assumes Linux
- Uses `/proc` filesystem for CPU and memory
- Uses `df` command for storage information
- No platform abstraction needed (project targets Linux only)

### Error Handling
- Graceful fallback to defaults if detection fails
- Logs warnings for detection failures
- Never panics on resource collection errors

## Conclusion

This resource verification system provides a robust, production-ready solution for capacity management in the Beemesh machine plane. The silent drop behavior for nodes without capacity simplifies scheduler logic and improves efficiency by eliminating unnecessary rejection messages.

The implementation follows Kubernetes resource model conventions (millicores, bytes) and includes appropriate safety margins to prevent resource exhaustion while maximizing utilization.
