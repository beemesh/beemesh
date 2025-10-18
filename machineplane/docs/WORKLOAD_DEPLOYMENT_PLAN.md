# Workload Deployment Plan

## Overview

This document outlines the comprehensive plan for implementing workload deployment in the Beemesh machine plane using `podman kube play -f`, provider announcements, and a mockable testing infrastructure.

## Architecture Components

### 1. Runtime Engine System (`runtime/`)

A pluggable runtime system that supports multiple container technologies:

- **Runtime Engine Trait**: Common interface for all runtime engines
- **Podman Engine**: Production implementation using `podman kube play -f`
- **Docker Engine**: Future support for Docker Compose
- **Mock Engine**: Testing implementation with verification capabilities

#### Key Features:
- Manifest validation before deployment
- Resource limit enforcement
- Port mapping extraction
- Workload lifecycle management (deploy, status, logs, remove)
- Pluggable architecture for future extensions (VMs, etc.)

### 2. Provider Announcement System (`provider/`)

Efficient discovery system for finding which nodes host which manifests:

- **Provider Manager**: Coordinates announcements and discovery
- **DHT Integration**: Uses libp2p Kademlia for provider records
- **Announcement System**: Nodes announce themselves as manifest providers
- **Discovery System**: Query and cache provider information

#### Advantages over Gossipsub:
- More efficient for targeted queries
- Built-in TTL and expiration handling
- Scalable to large networks
- Reduced network chatter

### 3. Workload Manager (`workload_manager.rs`)

Central coordinator that integrates runtime engines with provider announcements:

- **Unified Interface**: Single API for workload operations
- **Provider Integration**: Automatic announcements after successful deployment
- **Health Monitoring**: Background status checking
- **Event System**: Notifications for deployment events

### 4. Integration Layer (`workload_integration.rs`)

Updates existing `apply_message.rs` to use the new system:

- **Enhanced Apply Handler**: Uses workload manager for deployments
- **Backward Compatibility**: Maintains existing DHT storage
- **Manifest Decryption**: Integrates with key reconstruction
- **Error Handling**: Proper error responses

## Implementation Details

### Podman Integration

The system uses `podman kube play` to deploy Kubernetes manifests:

```bash
podman kube play --replicas=3 --memory=1GB --cpus=2.0 manifest.yaml
```

Key features:
- Automatic replica scaling
- Resource limit enforcement
- Environment variable injection
- Network configuration
- Volume mounting support

### Provider Announcement Flow

1. **Deployment Success**: After successful workload deployment
2. **Generate Metadata**: Create provider announcement with node capabilities
3. **DHT Storage**: Store provider record with TTL
4. **Kademlia Providing**: Start providing the manifest key
5. **Background Refresh**: Periodic re-announcements

### Discovery Flow

1. **Query Initiation**: Request providers for a manifest ID
2. **Cache Check**: Look for cached providers first
3. **DHT Query**: Query Kademlia for providers
4. **Result Processing**: Filter, sort, and cache results
5. **Return Providers**: Ordered list of available providers

### Mock Engine for Testing

The mock engine provides comprehensive testing capabilities:

#### Verification Methods:
- `get_workload_manifest(id)`: Retrieve stored manifest content
- `get_workload_config(id)`: Retrieve deployment configuration
- `workload_count()`: Get number of deployed workloads
- `clear_workloads()`: Reset for clean tests

#### Configuration Options:
- Success rate simulation
- Deployment delays
- Port mapping simulation
- Resource limit testing

## Testing Strategy

### Unit Tests
- Individual component testing
- Mock engine verification
- Error condition handling

### Integration Tests
- End-to-end deployment workflows
- Provider announcement verification
- Multi-node discovery testing

### Example Test Workflow:
```rust
// Deploy workload using mock engine
let deployment = manager.deploy_workload(
    &mut swarm, 
    "test-manifest", 
    &manifest_content, 
    &config
).await?;

// Verify deployment through mock engine
let stored_manifest = mock_engine.get_workload_manifest(&deployment.workload_id);
assert_eq!(stored_manifest.unwrap(), manifest_content);

// Verify provider announcement
let providers = manager.discover_providers(&mut swarm, "test-manifest").await?;
assert!(!providers.is_empty());
```

## Configuration

### Workload Manager Config
```rust
WorkloadManagerConfig {
    announce_as_provider: true,        // Announce after deployment
    provider_ttl_seconds: 3600,        // 1 hour announcement TTL
    preferred_runtime: Some("podman"), // Preferred engine
    validate_manifests: true,          // Validate before deploy
    max_concurrent_deployments: 10,    // Deployment limit
}
```

### Runtime Engine Selection
1. Try preferred engine if specified and available
2. Fall back to default engine (auto-detected best available)
3. Error if no engines available

### Provider Discovery
- 30-second timeout for discovery queries
- 5-minute cache TTL for provider results
- Maximum 50 providers tracked per manifest
- Automatic cleanup of expired providers

## File Organization

```
machine/src/
├── runtime/
│   ├── mod.rs              # Runtime engine trait and registry
│   ├── engines/
│   │   ├── mod.rs          # Engine exports
│   │   ├── podman.rs       # Podman engine implementation
│   │   └── docker.rs       # Docker engine implementation
│   └── mock.rs             # Mock engine for testing
├── provider/
│   ├── mod.rs              # Provider manager and types
│   ├── announcements.rs    # Provider announcement utilities
│   └── discovery.rs        # Provider discovery utilities
├── workload_manager.rs     # Main workload coordinator
├── workload_integration.rs # Integration with apply_message.rs
└── workload_manager_test.rs # Comprehensive tests
```

## Provider Announcement vs Gossipsub

### Provider Announcement Advantages:
- **Targeted Queries**: Only query for specific manifests
- **Efficient Storage**: DHT provides persistent, distributed storage
- **Automatic Expiration**: Built-in TTL handling
- **Scalability**: Better performance with large numbers of nodes
- **Reduced Bandwidth**: No broadcast messages for discovery

### Gossipsub Use Cases:
- **Real-time Updates**: Live status updates, events
- **Broadcast Messages**: Network-wide announcements
- **Pub/Sub Patterns**: Subscribe to specific topics

## Deployment Process

### 1. Manifest Reception
- Receive apply request via libp2p request-response
- Verify envelope signature and nonce
- Parse FlatBuffer apply request

### 2. Manifest Processing
- Extract and decrypt manifest content
- Validate manifest format (Kubernetes YAML)
- Generate stable manifest ID

### 3. Workload Deployment
- Select appropriate runtime engine
- Apply resource limits and configuration
- Execute deployment (e.g., `podman kube play`)
- Verify successful deployment

### 4. Provider Announcement
- Create provider metadata (engine type, capabilities)
- Store provider record in DHT
- Start providing manifest key via Kademlia
- Schedule periodic re-announcements

### 5. Response and Monitoring
- Send success/failure response to requester
- Start background health monitoring
- Emit deployment events for logging/metrics

## Future Extensions

### Virtual Machine Support
- Add VM runtime engine
- Support for VM manifest formats (cloud-init, etc.)
- VM-specific resource management

### Advanced Scheduling
- Resource-aware provider selection
- Load balancing across providers
- Affinity/anti-affinity rules

### Security Enhancements
- Provider capability verification
- Secure manifest distribution
- Access control for deployments

### Monitoring and Observability
- Deployment metrics collection
- Health check automation
- Performance monitoring

## Error Handling

### Runtime Engine Errors
- Engine not available: Fall back to alternative
- Deployment failure: Clean up partial deployments
- Invalid manifest: Return validation errors

### Provider System Errors
- DHT unavailable: Cache previous results
- Announcement failure: Retry with backoff
- Discovery timeout: Return cached or empty results

### Network Errors
- Peer disconnection: Handle gracefully
- Message corruption: Verify signatures
- Timeout handling: Configurable timeouts

## Backward Compatibility

The system maintains backward compatibility with existing code:

- **DHT Storage**: Continue storing applied manifests in DHT
- **API Compatibility**: Existing apply message format supported
- **Gradual Migration**: Can be deployed alongside existing system

## Performance Considerations

### Deployment Performance
- Parallel deployments (configurable limit)
- Async operations throughout
- Efficient manifest validation

### Discovery Performance
- Provider result caching
- Concurrent DHT queries
- Smart query timeouts

### Memory Usage
- Bounded cache sizes
- Periodic cleanup tasks
- Efficient data structures

## Security Model

### Manifest Security
- Signature verification on all messages
- Encrypted manifest distribution
- Key reconstruction for decryption

### Provider Security
- Signed provider announcements
- Peer identity verification
- TTL-based automatic expiration

### Runtime Security
- Container isolation (via Podman/Docker)
- Resource limit enforcement
- Network security policies

This comprehensive plan provides a robust, testable, and extensible foundation for workload deployment in the Beemesh machine plane.