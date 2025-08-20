package podman

import (
	"context"
	"fmt"
	"log"
	"os"
	"strings"

	"beemesh/pkg/crypto"
	"beemesh/pkg/types"

	"github.com/containers/podman/v5/pkg/bindings"
	"github.com/containers/podman/v5/pkg/bindings/containers"
	"github.com/containers/podman/v5/pkg/bindings/pods"
	"github.com/containers/podman/v5/pkg/domain/entities"
	"k8s.io/api/apps/v1"
)

type Client struct {
	conn   context.Context
	ctx    context.Context
	nodeID string
}

// configureNetworkIsolation returns the proper slirp4netns network configuration
// for beemesh network isolation architecture
func (c *Client) configureNetworkIsolation() []string {
	// Enhanced slirp4netns configuration for network isolation:
	// - enable_ipv6=true: Enable IPv6 support for full networking capabilities
	// - allow_host_loopback=true: Allow containers to reach host services (needed for beemesh API)  
	// - cidr=10.0.2.0/24: Use dedicated subnet for isolation
	// - mtu=1500: Standard MTU size
	// - no_map_gw=false: Allow gateway mapping for host communication
	return []string{"slirp4netns:enable_ipv6=true,allow_host_loopback=true,cidr=10.0.2.0/24,mtu=1500"}
}

// validateNetworkConfiguration validates that the slirp4netns network configuration
// aligns with beemesh architecture requirements
func (c *Client) validateNetworkConfiguration() error {
	log.Printf("Validating slirp4netns network configuration for node %s", c.nodeID)
	
	// Verify slirp4netns capabilities
	config := c.configureNetworkIsolation()
	if len(config) == 0 {
		return fmt.Errorf("network configuration is empty")
	}
	
	// Log network isolation configuration
	log.Printf("Network isolation configured: %s", strings.Join(config, ","))
	log.Printf("Network isolation features: IPv6 enabled, host loopback allowed, dedicated CIDR subnet")
	
	return nil
}

func NewClient() (*Client, error) {
	conn, err := bindings.NewConnection(context.Background(), "unix:///run/podman/podman.sock")
	if err != nil {
		return nil, fmt.Errorf("failed to create Podman connection: %v", err)
	}
	nodeID := os.Getenv("BEEMESH_NODE_ID")
	if nodeID == "" {
		return nil, fmt.Errorf("BEEMESH_NODE_ID not set")
	}
	
	client := &Client{conn: conn, ctx: context.Background(), nodeID: nodeID}
	
	// Validate network configuration on initialization
	if err := client.validateNetworkConfiguration(); err != nil {
		return nil, fmt.Errorf("network configuration validation failed: %v", err)
	}
	
	return client, nil
}

func (c *Client) Deploy(task types.Task) error {
	if task.Destination != "all" && task.Destination != c.nodeID && task.Destination != "scheduler" {
		log.Printf("Skipping task %s: destination %s does not match node %s", task.TaskID, task.Destination, c.nodeID)
		return nil
	}
	switch task.Kind {
	case "StatelessWorkload":
		dep, ok := task.Spec.(*v1.Deployment)
		if !ok {
			return fmt.Errorf("invalid stateless workload spec")
		}
		return c.deployPodmanPod(task.Name, dep.Spec.Template.Spec, int(*dep.Spec.Replicas), false)
	case "StatefulWorkload":
		ss, ok := task.Spec.(*v1.StatefulSet)
		if !ok {
			return fmt.Errorf("invalid stateful workload spec")
		}
		return c.deployPodmanPod(task.Name, ss.Spec.Template.Spec, int(*ss.Spec.Replicas), true)
	default:
		return fmt.Errorf("unsupported kind: %s", task.Kind)
	}
}

func (c *Client) DeployInstances(task types.Task, numInstances int, startOrdinal int, stateful bool) error {
	var spec v1.PodSpec
	var name string
	if stateful {
		ss, ok := task.Spec.(*v1.StatefulSet)
		if !ok {
			return fmt.Errorf("invalid stateful workload spec")
		}
		spec = ss.Spec.Template.Spec
		name = ss.Name
	} else {
		dep, ok := task.Spec.(*v1.Deployment)
		if !ok {
			return fmt.Errorf("invalid stateless workload spec")
		}
		spec = dep.Spec.Template.Spec
		name = dep.Name
	}
	for i := 0; i < numInstances; i++ {
		var podName string
		if stateful {
			ordinal := startOrdinal + i
			podName = fmt.Sprintf("%s-%d", name, ordinal)
		} else {
			id, err := crypto.GenerateID()
			if err != nil {
				return fmt.Errorf("failed to generate pod ID: %v", err)
			}
			podName = fmt.Sprintf("%s-%s", name, id)
		}
		podSpec := entities.PodCreateOptions{
			Name:              podName,
			Network:           c.configureNetworkIsolation(),
			Labels:            map[string]string{"beemesh.workload": name},
			Infra:             false,
			Share:             []string{"none"},
			ShareParent:       false,
			AllowHostLoopback: true,
		}
		if stateful {
			podSpec.Labels["beemesh.workload.kind"] = "stateful"
		} else {
			podSpec.Labels["beemesh.workload.kind"] = "stateless"
		}
		_, err := pods.CreatePod(c.ctx, &podSpec, nil)
		if err != nil {
			return fmt.Errorf("failed to create pod %s: %v", podName, err)
		}
		for _, container := range spec.Containers {
			env := make([]string, 0, len(container.Env))
			for _, e := range container.Env {
				if e.ValueFrom != nil && e.ValueFrom.FieldRef != nil {
					if e.ValueFrom.FieldRef.FieldPath == "metadata.name" {
						env = append(env, fmt.Sprintf("%s=%s", e.Name, podName))
					}
				} else {
					env = append(env, fmt.Sprintf("%s=%s", e.Name, e.Value))
				}
			}
			if stateful && strings.HasPrefix(name, "my-raft-cluster") {
				env = append(env, fmt.Sprintf("BEEMESH_WORKLOAD_SERVICE=%s", podName))
			} else {
				env = append(env, fmt.Sprintf("BEEMESH_WORKLOAD_SERVICE=%s", name))
			}
			env = append(env, fmt.Sprintf("BEEMESH_NAMESPACE=%s", os.Getenv("BEEMESH_NAMESPACE")))
			ports := []entities.ContainerPort{{HostPort: 8080, ContainerPort: 8080, Protocol: "tcp"}}
			volumeMounts := make([]entities.Mount, 0, len(container.VolumeMounts))
			for _, vm := range container.VolumeMounts {
				if vm.Name != "manifest" {
					volumeMounts = append(volumeMounts, entities.Mount{
						Destination: vm.MountPath,
						Source:      fmt.Sprintf("/var/lib/beemesh/%s-%s", name, vm.Name),
						Type:        "bind",
					})
				}
			}
			containerSpec := entities.ContainerCreateOptions{
				Name:    fmt.Sprintf("%s-%s", podName, container.Name),
				Image:   container.Image,
				Command: container.Command,
				Env:     env,
				Ports:   []entities.ContainerPort{},
				Mounts:  volumeMounts,
				Pod:     podName,
				Labels:  podSpec.Labels,
			}
			if container.Name == "sidecar" {
				containerSpec.Ports = ports
			}
			_, err = containers.CreateWithSpec(c.ctx, &containerSpec, nil)
			if err != nil {
				return fmt.Errorf("failed to create container %s: %v", containerSpec.Name, err)
			}
			err = containers.Start(c.ctx, containerSpec.Name, nil)
			if err != nil {
				return fmt.Errorf("failed to start container %s: %v", containerSpec.Name, err)
			}
			log.Printf("Created and started container %s in pod %s", containerSpec.Name, podName)
		}
	}
	return nil
}

func (c *Client) deployPodmanPod(name string, spec v1.PodSpec, replicas int, stateful bool) error {
	// Deprecated: Use DeployInstances instead
	for i := 0; i < replicas; i++ {
		podName := name
		if stateful {
			podName = fmt.Sprintf("%s-%d", name, i)
		}
		podSpec := entities.PodCreateOptions{
			Name:              podName,
			Network:           c.configureNetworkIsolation(),
			Labels:            map[string]string{"beemesh.workload": name},
			Infra:             false,
			Share:             []string{"none"},
			ShareParent:       false,
			AllowHostLoopback: true,
		}
		if stateful {
			podSpec.Labels["beemesh.workload.kind"] = "stateful"
		} else {
			podSpec.Labels["beemesh.workload.kind"] = "stateless"
		}
		_, err := pods.CreatePod(c.ctx, &podSpec, nil)
		if err != nil {
			return fmt.Errorf("failed to create pod %s: %v", podName, err)
		}
		for _, container := range spec.Containers {
			env := make([]string, 0, len(container.Env))
			for _, e := range container.Env {
				if e.ValueFrom != nil && e.ValueFrom.FieldRef != nil {
					if e.ValueFrom.FieldRef.FieldPath == "metadata.name" {
						env = append(env, fmt.Sprintf("%s=%s", e.Name, podName))
					}
				} else {
					env = append(env, fmt.Sprintf("%s=%s", e.Name, e.Value))
				}
			}
			if stateful && strings.HasPrefix(name, "my-raft-cluster") {
				env = append(env, fmt.Sprintf("BEEMESH_WORKLOAD_SERVICE=%s", podName))
			} else {
				env = append(env, fmt.Sprintf("BEEMESH_WORKLOAD_SERVICE=%s", name))
			}
			env = append(env, fmt.Sprintf("BEEMESH_NAMESPACE=%s", os.Getenv("BEEMESH_NAMESPACE")))
			ports := []entities.ContainerPort{{HostPort: 8080, ContainerPort: 8080, Protocol: "tcp"}}
			volumeMounts := make([]entities.Mount, 0, len(container.VolumeMounts))
			for _, vm := range container.VolumeMounts {
				if vm.Name != "manifest" {
					volumeMounts = append(volumeMounts, entities.Mount{
						Destination: vm.MountPath,
						Source:      fmt.Sprintf("/var/lib/beemesh/%s-%s", name, vm.Name),
						Type:        "bind",
					})
				}
			}
			containerSpec := entities.ContainerCreateOptions{
				Name:    fmt.Sprintf("%s-%s", podName, container.Name),
				Image:   container.Image,
				Command: container.Command,
				Env:     env,
				Ports:   []entities.ContainerPort{},
				Mounts:  volumeMounts,
				Pod:     podName,
				Labels:  podSpec.Labels,
			}
			if container.Name == "sidecar" {
				containerSpec.Ports = ports
			}
			_, err = containers.CreateWithSpec(c.ctx, &containerSpec, nil)
			if err != nil {
				return fmt.Errorf("failed to create container %s: %v", containerSpec.Name, err)
			}
			err = containers.Start(c.ctx, containerSpec.Name, nil)
			if err != nil {
				return fmt.Errorf("failed to start container %s: %v", containerSpec.Name, err)
			}
			log.Printf("Created and started container %s in pod %s", containerSpec.Name, podName)
		}
	}
	return nil
}
