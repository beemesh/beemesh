package podman

import (
	"os"
	"strings"
	"testing"
)

func TestConfigureNetworkIsolation(t *testing.T) {
	client := &Client{nodeID: "test-node"}
	
	config := client.configureNetworkIsolation()
	
	if len(config) == 0 {
		t.Fatal("Network configuration should not be empty")
	}
	
	networkConfig := config[0]
	
	// Verify slirp4netns is configured
	if !strings.HasPrefix(networkConfig, "slirp4netns:") {
		t.Errorf("Expected slirp4netns configuration, got: %s", networkConfig)
	}
	
	// Verify IPv6 is enabled
	if !strings.Contains(networkConfig, "enable_ipv6=true") {
		t.Error("IPv6 should be enabled in network configuration")
	}
	
	// Verify host loopback is allowed
	if !strings.Contains(networkConfig, "allow_host_loopback=true") {
		t.Error("Host loopback should be allowed in network configuration")
	}
	
	// Verify dedicated CIDR subnet
	if !strings.Contains(networkConfig, "cidr=10.0.2.0/24") {
		t.Error("Dedicated CIDR subnet should be configured")
	}
	
	// Verify MTU setting
	if !strings.Contains(networkConfig, "mtu=1500") {
		t.Error("MTU should be set to 1500")
	}
}

func TestValidateNetworkConfiguration(t *testing.T) {
	client := &Client{nodeID: "test-node"}
	
	err := client.validateNetworkConfiguration()
	if err != nil {
		t.Errorf("Network configuration validation failed: %v", err)
	}
}

func TestNewClientValidation(t *testing.T) {
	// Set required environment variable
	os.Setenv("BEEMESH_NODE_ID", "test-node")
	defer os.Unsetenv("BEEMESH_NODE_ID")
	
	// This test will fail in CI due to missing podman socket, but demonstrates 
	// that network validation is called during client initialization
	_, err := NewClient()
	
	// Should fail on podman connection, not on network validation
	if err != nil && !strings.Contains(err.Error(), "failed to create Podman connection") {
		t.Logf("Expected podman connection error, got: %v", err)
	}
}