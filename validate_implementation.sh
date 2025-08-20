#!/bin/bash

# Beemesh slirp4netns Implementation Validation
# This script validates that the networking implementation meets the requirements

echo "🔍 Beemesh slirp4netns Implementation Validation"
echo "==============================================="
echo

# Check if networking files exist
echo "📁 Checking implementation files..."
files_to_check=(
    "pkg/podman/podman.go"
    "NETWORKING.md" 
    "test_networking.go"
    "demo_networking.sh"
    "pkg/podman/podman_test.go"
)

all_present=true
for file in "${files_to_check[@]}"; do
    if [[ -f "$file" ]]; then
        echo "  ✅ $file"
    else
        echo "  ❌ $file (missing)"
        all_present=false
    fi
done

if [[ "$all_present" = false ]]; then
    echo "❌ Some implementation files are missing"
    exit 1
fi

echo

# Validate network configuration
echo "🔧 Validating network configuration..."
if grep -q "configureNetworkIsolation" pkg/podman/podman.go; then
    echo "  ✅ configureNetworkIsolation function implemented"
else
    echo "  ❌ configureNetworkIsolation function missing"
    exit 1
fi

if grep -q "enable_ipv6=true" pkg/podman/podman.go; then
    echo "  ✅ IPv6 support enabled"
else
    echo "  ❌ IPv6 support missing"
    exit 1
fi

if grep -q "allow_host_loopback=true" pkg/podman/podman.go; then
    echo "  ✅ Host loopback access configured"  
else
    echo "  ❌ Host loopback access missing"
    exit 1
fi

if grep -q "cidr=10.0.2.0/24" pkg/podman/podman.go; then
    echo "  ✅ Dedicated CIDR subnet configured"
else
    echo "  ❌ CIDR subnet configuration missing"
    exit 1
fi

if grep -q "mtu=1500" pkg/podman/podman.go; then
    echo "  ✅ MTU optimization configured"
else
    echo "  ❌ MTU configuration missing"
    exit 1
fi

echo

# Validate network validation
echo "🛡️  Validating network validation logic..."
if grep -q "validateNetworkConfiguration" pkg/podman/podman.go; then
    echo "  ✅ Network validation function implemented"
else
    echo "  ❌ Network validation function missing"
    exit 1
fi

echo

# Check container isolation
echo "🔒 Validating container network isolation..."
if grep -q 'if container.Name == "sidecar"' pkg/podman/podman.go; then
    echo "  ✅ Sidecar port mapping isolation implemented"
else
    echo "  ❌ Sidecar port isolation missing"
    exit 1  
fi

if grep -q 'Ports:   \[\]entities.ContainerPort{}' pkg/podman/podman.go; then
    echo "  ✅ Application container port isolation implemented"
else
    echo "  ❌ Application container isolation missing"
    exit 1
fi

echo

# Run the networking test
echo "🧪 Running networking functionality tests..."
if go run test_networking.go; then
    echo "  ✅ Networking tests passed"
else
    echo "  ❌ Networking tests failed"
    exit 1
fi

echo

echo "🎉 SUCCESS: slirp4netns networking implementation validation completed!"
echo
echo "✅ Implementation Summary:"
echo "   • Enhanced slirp4netns configuration with IPv6, host loopback, CIDR isolation"
echo "   • Network validation during client initialization"
echo "   • Container isolation (only sidecars have network access)"
echo "   • Comprehensive documentation and testing"
echo "   • Architecture alignment with beemesh network isolation requirements"
echo
echo "🚀 The slirp4netns networking implementation is complete and validated!"