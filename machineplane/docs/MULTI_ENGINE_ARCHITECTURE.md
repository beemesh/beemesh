# Multi-Engine Runtime Architecture

## Overview

This document describes the updated architecture for workload deployment in the Beemesh machine plane that supports multiple runtime engines (Podman, Docker, VMs) dynamically based on manifest type and annotations.

## Key Design Principles

### 1. **Multiple Engines per Machine**
Each machine node maintains a registry of available runtime engines rather than a single global workload manager. This allows:
- Dynamic engine selection based on manifest type
- Support for heterogeneous workloads on the same node
- Graceful fallback when preferred engines are unavailable

### 2. **Manifest-Driven Engine Selection**
Runtime engine selection is determined by:
- **Annotations**: `beemesh.io/runtime-engine: "podman"`
- **Manifest Format**: Kubernetes YAML → Podman, Docker Compose → Docker
- **Availability Fallback**: Best available engine if preferred is unavailable

### 3. **Zero Global State**
No single global workload manager. Instead:
- Global runtime registry with all available engines
- Global provider manager for announcements
- Per-workload engine assignment and tracking

## Architecture Components

### Runtime Registry (`runtime/`)
```
RuntimeRegistry
├── engines: HashMap<String, Box<dyn RuntimeEngine>>
├── default_engine: Option<String>
└── Methods:
    ├── register(engine)
    ├── get_engine(name) -> Option<&dyn RuntimeEngine>
    ├── check_available_engines() -> HashMap<String, bool>
    └── set_default_engine(name)
```

**Available Engines:**
- `PodmanEngine`: Production Kubernetes manifest deployment
- `DockerEngine`: Docker Compose support
- `MockEngine`: Testing with verification capabilities
- `VMEngine`: Future virtual machine support

### Provider Manager (`provider/`)
```
ProviderManager
├── local_providers: HashMap<String, ProviderInfo>
├── remote_providers: HashMap<String, Vec<ProviderInfo>>
└── Methods:
    ├── announce_provider(manifest_id, metadata)
    ├── discover_providers(manifest_id) -> Vec<ProviderInfo>
    └── stop_providing(manifest_id)
```

**Provider Metadata Includes:**
- Runtime engine used: `"runtime_engine": "podman"`
- Node capabilities: `"capabilities": "podman,docker"`
- Resource information: `"resource.cpu": "4", "resource.memory": "8GB"`
- Workload ID: `"workload_id": "beemesh-manifest-123-456"`

### Workload Integration (`workload_integration.rs`)
Central orchestration logic that:
1. **Receives Apply Requests** via libp2p
2. **Selects Runtime Engine** based on manifest analysis
3. **Deploys Workload** using selected engine
4. **Announces Provider** with engine-specific metadata
5. **Maintains Backward Compatibility** with existing DHT storage

## Engine Selection Logic

### 1. **Annotation-Based Selection**
```yaml
apiVersion: v1
kind: Pod
metadata:
  name: my-pod
  annotations:
    beemesh.io/runtime-engine: "podman"  # Explicit engine selection
    beemesh.io/resources.cpu: "2.0"      # Resource hints
    beemesh.io/resources.memory: "4GB"   # Memory requirements
```

### 2. **Format-Based Selection**
```yaml
# Kubernetes YAML → Podman Engine
apiVersion: v1
kind: Pod
metadata:
  name: nginx-pod
spec:
  containers:
  - name: nginx
    image: nginx:latest
```

```yaml
# Docker Compose → Docker Engine
version: '3.8'
services:
  web:
    image: nginx:latest
    ports:
      - "80:80"
```

### 3. **Availability Fallback**
```rust
async fn select_runtime_engine(manifest_content: &[u8]) -> Result<String, Error> {
    // 1. Check for explicit annotation
    if let Some(engine) = extract_engine_annotation(manifest_content) {
        if is_engine_available(engine).await {
            return Ok(engine);
        }
    }
    
    // 2. Infer from manifest format
    let inferred_engine = infer_engine_from_format(manifest_content)?;
    if is_engine_available(&inferred_engine).await {
        return Ok(inferred_engine);
    }
    
    // 3. Fallback to best available
    select_best_available_engine().await
}
```

## Provider Announcement Strategy

### Engine-Specific Announcements
Each deployment announces provider capability with engine metadata:

```rust
// Provider announcement includes engine information
let metadata = HashMap::from([
    ("runtime_engine", "podman"),
    ("workload_id", "beemesh-manifest-abc-123"),
    ("node_type", "beemesh-machine"),
    ("capabilities", "podman,docker,mock"),
    ("resource.cpu", "4"),
    ("resource.memory", "8GB"),
]);

provider_manager.announce_provider(swarm, manifest_id, metadata)?;
```

### Discovery with Engine Filtering
Clients can discover providers with specific engine requirements:

```rust
let providers = provider_manager.discover_providers(swarm, manifest_id).await?;

// Filter by engine type
let podman_providers: Vec<_> = providers
    .iter()
    .filter(|p| p.metadata.get("runtime_engine") == Some(&"podman".to_string()))
    .collect();
```

## Deployment Flow

### 1. **Apply Request Reception**
```
libp2p::apply_message() 
    ↓
workload_integration::handle_apply_message_with_workload_manager()
    ↓
process_manifest_deployment()
```

### 2. **Engine Selection & Deployment**
```
select_runtime_engine(manifest_content)
    ↓
runtime_registry.get_engine(engine_name)
    ↓
engine.deploy_workload(manifest_id, content, config)
```

### 3. **Provider Announcement**
```
Create metadata with engine info
    ↓
provider_manager.announce_provider(manifest_id, metadata)
    ↓
DHT storage with TTL
```

### 4. **Backward Compatibility**
```
store_applied_manifest_in_dht()  // Legacy DHT storage
    ↓
Continue existing task tracking
```

## Testing Strategy

### Mock Engine Verification
The mock engine provides comprehensive testing without container dependencies:

```rust
// Deploy with mock engine
let workload = mock_engine.deploy_workload(manifest_id, content, config).await?;

// Verify exact manifest content stored
let stored_manifest = mock_engine.get_workload_manifest(&workload.id)?;
assert_eq!(stored_manifest, original_manifest_content);

// Verify deployment configuration
let stored_config = mock_engine.get_workload_config(&workload.id)?;
assert_eq!(stored_config.replicas, expected_replicas);

// Test workload operations
let logs = mock_engine.get_workload_logs(&workload.id, Some(5)).await?;
let status = mock_engine.get_workload_status(&workload.id).await?;
mock_engine.remove_workload(&workload.id).await?;
```

### Integration Testing
1. **Engine Selection Testing**: Verify correct engine chosen for different manifest types
2. **Provider Announcement Testing**: Confirm metadata includes engine information
3. **Backward Compatibility Testing**: Ensure legacy systems continue working
4. **Multi-Engine Testing**: Deploy different workloads with different engines simultaneously

## Benefits of Multi-Engine Architecture

### 1. **Flexibility**
- **Workload-Specific Engines**: Kubernetes pods use Podman, Docker services use Docker
- **Future Extensibility**: Easy to add VM engines, container-free engines, etc.
- **Resource Optimization**: Different engines for different resource requirements

### 2. **Resilience**
- **Graceful Degradation**: Fallback engines when preferred ones unavailable
- **Engine Isolation**: Problems with one engine don't affect others
- **Heterogeneous Clusters**: Different nodes can have different engine capabilities

### 3. **Provider Discovery Efficiency**
- **Engine-Aware Discovery**: Find providers with specific engine capabilities
- **Resource-Based Selection**: Choose providers based on available resources
- **Capability Matching**: Match workload requirements with provider capabilities

### 4. **Development & Testing**
- **Mock Engine**: Full testing without container dependencies
- **Isolated Testing**: Test each engine independently
- **Verification**: Direct inspection of deployed workloads in tests

## Migration Path

### Phase 1: Backward Compatible (Current)
- Runtime registry alongside existing system
- Legacy DHT storage maintained
- Existing tests continue to pass

### Phase 2: Enhanced Discovery
- Provider announcements include engine metadata
- Discovery queries can filter by engine type
- Enhanced provider selection logic

### Phase 3: Full Multi-Engine
- Complete engine selection based on annotations
- Engine-specific optimizations
- Advanced resource management

## Example Deployments

### Kubernetes Pod with Podman
```yaml
apiVersion: v1
kind: Pod
metadata:
  name: web-server
  annotations:
    beemesh.io/runtime-engine: "podman"
spec:
  containers:
  - name: nginx
    image: nginx:latest
```
**Result**: Deployed via Podman, announced as `"runtime_engine": "podman"`

### Docker Compose Service
```yaml
version: '3.8'
services:
  api:
    image: node:16
    ports:
      - "3000:3000"
```
**Result**: Deployed via Docker, announced as `"runtime_engine": "docker"`

### Testing with Mock Engine
```yaml
apiVersion: v1
kind: Pod
metadata:
  name: test-pod
  annotations:
    beemesh.io/runtime-engine: "mock"
spec:
  containers:
  - name: test
    image: test:latest
```
**Result**: Simulated deployment, full verification capabilities

## Future Extensions

### Virtual Machine Support
```yaml
apiVersion: beemesh.io/v1
kind: VirtualMachine
metadata:
  name: ubuntu-vm
  annotations:
    beemesh.io/runtime-engine: "qemu"
spec:
  image: ubuntu:22.04
  vcpus: 2
  memory: 4Gi
  disk: 20Gi
```

### Serverless Functions
```yaml
apiVersion: beemesh.io/v1
kind: Function
metadata:
  name: data-processor
  annotations:
    beemesh.io/runtime-engine: "wasm"
spec:
  runtime: python:3.9
  handler: main.process
  memory: 512Mi
```

This multi-engine architecture provides the flexibility to support diverse workload types while maintaining simplicity in testing and development through the mock engine approach.