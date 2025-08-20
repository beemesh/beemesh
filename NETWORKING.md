# Beemesh slirp4netns Network Implementation

## Overview

This document describes the slirp4netns networking implementation in Beemesh, which provides network isolation and secure communication for container workloads.

## Architecture

Beemesh uses slirp4netns to provide user-mode networking with enhanced isolation:

### Network Isolation
- Pods run in isolated private network namespaces using slirp4netns
- Each pod gets its own private network (10.0.2.0/24 CIDR)
- Sidecars act as the sole entry/exit points for pod networking
- Application containers have no direct external network access

### slirp4netns Configuration

The enhanced slirp4netns configuration includes:

```go
slirp4netns:enable_ipv6=true,allow_host_loopback=true,cidr=10.0.2.0/24,mtu=1500
```

#### Configuration Parameters:
- `enable_ipv6=true`: Enables IPv6 support for dual-stack networking
- `allow_host_loopback=true`: Allows containers to communicate with host services (required for beemesh API)
- `cidr=10.0.2.0/24`: Dedicated private subnet for network isolation
- `mtu=1500`: Standard MTU size for optimal network performance

### Container Network Isolation

#### Sidecar Containers
- Only sidecar containers have port mappings (8080:8080)
- Sidecars register services with the beemesh registry
- All traffic is proxied through libp2p streams
- Communication with host beemesh service via `host.containers.internal:8080`

#### Application Containers  
- No port mappings or external network access
- Can only communicate locally within the pod
- Must use sidecar for any external communication
- Isolated from other pods and external networks

## Implementation Details

### Pod Creation
```go
podSpec := entities.PodCreateOptions{
    Name:              podName,
    Network:           c.configureNetworkIsolation(),
    Infra:             false,
    Share:             []string{"none"},
    ShareParent:       false,
    AllowHostLoopback: true,
}
```

### Container Creation
- Sidecar containers: `containerSpec.Ports = [{HostPort: 8080, ContainerPort: 8080}]`
- App containers: `containerSpec.Ports = []` (no ports)

### Network Validation
Network configuration is validated during client initialization to ensure proper slirp4netns setup.

## Security Benefits

1. **Network Perimeter Removal**: Pods cannot directly access external networks
2. **Traffic Interception**: All communications flow through monitored sidecars
3. **Service Discovery**: Services registered/discovered through encrypted libp2p DHT
4. **Traffic Encryption**: Inter-pod communication via libp2p encrypted streams

## Compatibility

This implementation is compatible with:
- Rootless Podman containers
- IPv4/IPv6 dual-stack networking  
- Standard container networking interfaces
- Kubernetes-style pod specifications

## Future Enhancements

- CNI plugin integration for advanced networking
- Network policy enforcement
- Traffic monitoring and metrics
- Cross-host networking via libp2p routing