package machine

import (
	"context"
	"encoding/json"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"strings"
	"time"

	v1 "k8s.io/api/apps/v1"
	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/runtime/serializer"

	"github.com/containers/podman/v4/pkg/bindings"
	podmanContainers "github.com/containers/podman/v4/pkg/bindings/containers"
	podmanPods "github.com/containers/podman/v4/pkg/bindings/pods"
	"github.com/containers/podman/v4/pkg/specgen"

	"beemesh/pkg/crypto"
	"beemesh/pkg/types"
)

// Client wraps a Podman API connection and its context.
type Client struct {
	conn   context.Context
	cancel context.CancelFunc
	socket string
	nodeID string
}

// NewClient creates a new Podman client connected to the local Podman socket.
func NewClient(socketPath, nodeID string) (*Client, error) {
	if socketPath == "" {
		socketPath = "unix:///run/podman/podman.sock"
	}
	// Ensure socket directory exists
	if _, err := os.Stat(filepath.Dir(socketPath)); os.IsNotExist(err) {
		return nil, fmt.Errorf("podman socket dir does not exist: %v", filepath.Dir(socketPath))
	}
	ctx, cancel := context.WithCancel(context.Background())
	conn, err := bindings.NewConnection(ctx, socketPath)
	if err != nil {
		cancel()
		return nil, fmt.Errorf("failed to connect to podman socket: %w", err)
	}
	log.Printf("[Podman] Connected to socket at %s", socketPath)
	return &Client{
		conn:   conn,
		cancel: cancel,
		socket: socketPath,
		nodeID: nodeID,
	}, nil
}

// Close shuts down the Podman client and releases resources.
func (c *Client) Close() {
	if c.cancel != nil {
		c.cancel()
	}
	log.Printf("[Podman] Closed connection to socket at %s", c.socket)
}

// CreatePod creates a new Podman pod given a name and optional labels.
func (c *Client) CreatePod(name string, labels map[string]string) (string, error) {
	spec := specgen.NewPodSpecGenerator()
	spec.Name = name
	spec.Labels = labels
	resp, err := podmanPods.CreatePodFromSpec(c.conn, spec)
	if err != nil {
		return "", fmt.Errorf("failed to create pod: %w", err)
	}
	log.Printf("[Podman] Created pod %q (ID=%s)", name, resp.ID)
	return resp.ID, nil
}

// CreateContainer creates a new container inside an existing pod.
func (c *Client) CreateContainer(podID, name, image string, cmd []string, env map[string]string, mounts map[string]string) (string, error) {
	if podID == "" {
		return "", fmt.Errorf("pod ID cannot be empty")
	}
	spec := specgen.NewSpecGenerator(image, false)
	spec.Name = name
	spec.Pod = podID
	spec.Command = cmd
	spec.Env = env
	// Add mounts
	for hostPath, containerPath := range mounts {
		spec.Mounts = append(spec.Mounts, specgen.Mount{
			Source:      hostPath,
			Destination: containerPath,
			Type:        "bind",
			Options:     []string{"rw"},
		})
	}
	resp, err := podmanContainers.CreateWithSpec(c.conn, spec)
	if err != nil {
		return "", fmt.Errorf("failed to create container: %w", err)
	}
	log.Printf("[Podman] Created container %q (ID=%s)", name, resp.ID)
	return resp.ID, nil
}

// StartContainer starts an existing container by ID.
func (c *Client) StartContainer(containerID string) error {
	if containerID == "" {
		return fmt.Errorf("container ID cannot be empty")
	}
	if err := podmanContainers.Start(c.conn, containerID, nil); err != nil {
		return fmt.Errorf("failed to start container: %w", err)
	}
	log.Printf("[Podman] Started container (ID=%s)", containerID)
	return nil
}

// DeployWorkloadFromTask deploys a workload described by a Task.
// This is the main entry point for the scheduler. It determines the type of manifest
// and delegates to the appropriate deployment function.
func (c *Client) DeployWorkloadFromTask(task types.Task) error {
	if task.Manifest == nil || len(task.Manifest) == 0 {
		return fmt.Errorf("task manifest cannot be empty")
	}

	// Determine the kind of Kubernetes resource from the manifest.
	kind, err := c.getManifestKind(task.Manifest)
	if err != nil {
		return fmt.Errorf("failed to determine manifest kind: %w", err)
	}

	switch kind {
	case "Deployment", "StatefulSet":
		return c.deployFromManifest(task)
	default:
		return fmt.Errorf("unsupported manifest kind: %s", kind)
	}
}

// getManifestKind extracts the 'kind' field from the raw manifest JSON.
func (c *Client) getManifestKind(manifest []byte) (string, error) {
	var obj map[string]interface{}
	if err := json.Unmarshal(manifest, &obj); err != nil {
		return "", err
	}
	if kind, ok := obj["kind"].(string); ok {
		return kind, nil
	}
	return "", fmt.Errorf("manifest is missing 'kind' field")
}

// deployFromManifest parses a Kubernetes Deployment or StatefulSet manifest
// and deploys its containers using Podman.
func (c *Client) deployFromManifest(task types.Task) error {
	// Create a Kubernetes scheme and codec to decode the manifest.
	scheme := runtime.NewScheme()
	_ = v1.AddToScheme(scheme)     // Add apps/v1 (Deployment, StatefulSet)
	_ = corev1.AddToScheme(scheme) // Add core/v1 (PodSpec, Container, etc.)
	codecFactory := serializer.NewCodecFactory(scheme)
	deserializer := codecFactory.UniversalDeserializer()

	// Decode the manifest into a runtime.Object.
	obj, groupVersionKind, err := deserializer.Decode(task.Manifest, nil, nil)
	if err != nil {
		return fmt.Errorf("failed to decode manifest: %w", err)
	}

	var podSpec *corev1.PodSpec
	var name, namespace string

	// Extract the PodSpec, name, and namespace based on the object type.
	switch typedObj := obj.(type) {
	case *v1.Deployment:
		podSpec = &typedObj.Spec.Template.Spec
		name = typedObj.Name
		namespace = typedObj.Namespace
	case *v1.StatefulSet:
		podSpec = &typedObj.Spec.Template.Spec
		name = typedObj.Name
		namespace = typedObj.Namespace
	default:
		return fmt.Errorf("unsupported resource type: %s", groupVersionKind.Kind)
	}

	if podSpec == nil {
		return fmt.Errorf("manifest is missing pod template spec")
	}
	if name == "" {
		name = "unnamed-workload"
	}
	if namespace == "" {
		namespace = task.Namespace
		if namespace == "" {
			namespace = "default"
		}
	}

	podName := fmt.Sprintf("%s-%s", namespace, name)
	log.Printf("[Podman] Deploying %s %s in namespace %s", groupVersionKind.Kind, name, namespace)

	// Create the Pod.
	podID, err := c.CreatePod(podName, map[string]string{
		"namespace":    namespace,
		"workload":     name,
		"k8s-kind":     groupVersionKind.Kind,
		"beemesh-task": task.TaskID,
	})
	if err != nil {
		return fmt.Errorf("failed to create pod: %w", err)
	}

	// Create and start containers from the PodSpec.
	for i, container := range podSpec.Containers {
		contName := fmt.Sprintf("%s-%d", podName, i+1)
		if container.Name != "" {
			contName = fmt.Sprintf("%s-%s", podName, container.Name)
		}

		// Build the container spec.
		spec := specgen.NewSpecGenerator(container.Image, false)
		spec.Name = contName
		spec.Pod = podID
		spec.Command = container.Command
		if len(container.Args) > 0 {
			spec.Command = append(spec.Command, container.Args...)
		}

		// Add environment variables.
		env := make(map[string]string)
		for _, e := range container.Env {
			env[e.Name] = e.Value
		}
		spec.Env = env

		// Add volume mounts.
		for _, volMount := range container.VolumeMounts {
			// For simplicity, we assume volumes are hostPath volumes defined in the manifest.
			// A full implementation would need to resolve PVCs or other volume types.
			hostPath := c.findHostPathForVolume(volMount.Name, podSpec.Volumes)
			if hostPath != "" {
				spec.Mounts = append(spec.Mounts, specgen.Mount{
					Source:      hostPath,
					Destination: volMount.MountPath,
					Type:        "bind",
					Options:     []string{"rw"},
				})
			}
		}

		// Handle ports.
		// For stateful workloads, the infra container will handle proxying.
		// For now, we expose ports directly on the container for simplicity.
		for _, port := range container.Ports {
			portStr := fmt.Sprintf("%d", port.ContainerPort)
			if port.Protocol == corev1.ProtocolUDP {
				portStr += "/udp"
			}
			spec.PortMappings = append(spec.PortMappings, specgen.PortMapping{
				ContainerPort: int(port.ContainerPort),
				HostPort:      int(port.ContainerPort), // Simple 1:1 mapping
				Protocol:      strings.ToLower(string(port.Protocol)),
			})
		}

		// Create and start the container.
		contID, err := c.CreateContainerFromSpec(spec)
		if err != nil {
			return fmt.Errorf("failed to create container %s: %w", contName, err)
		}

		if err := c.StartContainer(contID); err != nil {
			return fmt.Errorf("failed to start container %s: %w", contName, err)
		}

		log.Printf("[Podman] Created and started container %s (ID=%s) in pod %s", contName, contID, podName)
	}

	// Start the Beemesh infra container as a sidecar.
	// This is CRITICAL for the system to function (self-healing, networking).
	infraImage := os.Getenv("BEE_WORKLOAD_INFRA_IMAGE")
	if infraImage == "" {
		infraImage = "your-repo/beemesh-infra-container:latest" // Default value
	}

	privKey, err := c.generateAndEncodePrivateKey()
	if err != nil {
		return fmt.Errorf("failed to generate infra container key: %w", err)
	}

	peerID, err := c.derivePeerIDFromPrivateKey(privKey)
	if err != nil {
		return fmt.Errorf("failed to derive peer ID: %w", err)
	}

	infraSpec := specgen.NewSpecGenerator(infraImage, false)
	infraSpec.Name = podName + "-infra"
	infraSpec.Pod = podID
	infraSpec.Init = true // This makes it the "pause" container for the pod.

	// Pass configuration via environment variables.
	infraSpec.Env = map[string]string{
		"BEE_WORKLOAD_PEER_ID":     peerID,
		"BEE_WORKLOAD_PRIVATE_KEY": privKey,
		"BEE_WORKLOAD_NAMESPACE":   namespace,
		"BEE_WORKLOAD_NAME":        name,
		"BEE_WORKLOAD_POD_NAME":    podName,
		"BEE_WORKLOAD_REPLICAS":    fmt.Sprintf("%d", task.Replicas),
		"BEEMESH_BOOTSTRAP_PEERS":  os.Getenv("BEEMESH_BOOTSTRAP_PEERS"),
	}

	// Mount the manifest into the infra container so it can perform self-healing.
	manifestPath := filepath.Join("/tmp", podName, "manifest.yaml")
	if err := os.MkdirAll(filepath.Dir(manifestPath), 0755); err != nil {
		return fmt.Errorf("failed to create manifest dir: %w", err)
	}
	if err := os.WriteFile(manifestPath, task.Manifest, 0644); err != nil {
		return fmt.Errorf("failed to write manifest file: %w", err)
	}
	infraSpec.Mounts = append(infraSpec.Mounts, specgen.Mount{
		Source:      manifestPath,
		Destination: "/etc/beemesh/manifest.yaml",
		Type:        "bind",
		Options:     []string{"ro"},
	})

	infraID, err := c.CreateContainerFromSpec(infraSpec)
	if err != nil {
		return fmt.Errorf("failed to create infra container: %w", err)
	}

	if err := c.StartContainer(infraID); err != nil {
		return fmt.Errorf("failed to start infra container: %w", err)
	}

	log.Printf("[Podman] Workload %s deployed successfully (PodID=%s)", name, podID)
	return nil
}

// findHostPathForVolume is a helper to find the hostPath for a given volume name.
// This is a simplified implementation for demonstration.
func (c *Client) findHostPathForVolume(volumeName string, volumes []corev1.Volume) string {
	for _, vol := range volumes {
		if vol.Name == volumeName && vol.HostPath != nil {
			return vol.HostPath.Path
		}
	}
	return ""
}

// CreateContainerFromSpec is a helper that wraps the Podman bindings call.
func (c *Client) CreateContainerFromSpec(spec *specgen.SpecGenerator) (string, error) {
	resp, err := podmanContainers.CreateWithSpec(c.conn, spec)
	if err != nil {
		return "", fmt.Errorf("failed to create container: %w", err)
	}
	return resp.ID, nil
}

// generateAndEncodePrivateKey generates a new libp2p private key for the infra container
// and returns it as a base64-encoded string.
func (c *Client) generateAndEncodePrivateKey() (string, error) {
	privKey, err := crypto.GeneratePrivateKey()
	if err != nil {
		return "", err
	}
	return crypto.EncodePrivateKeyBase64(privKey)
}

// derivePeerIDFromPrivateKey takes a base64-encoded private key string and
// returns the corresponding peer ID as a string.
func (c *Client) derivePeerIDFromPrivateKey(b64PrivKey string) (string, error) {
	privKey, err := crypto.DecodePrivateKeyBase64(b64PrivKey)
	if err != nil {
		return "", err
	}
	peerID, err := crypto.PeerIDFromPrivateKey(privKey)
	if err != nil {
		return "", err
	}
	return peerID.String(), nil
}

// WaitForPod waits for a Podman pod to reach a ready state or timeout.
func (c *Client) WaitForPod(podID string, timeout time.Duration) error {
	start := time.Now()
	for {
		if time.Since(start) > timeout {
			return fmt.Errorf("timed out waiting for pod %s", podID)
		}
		pod, err := podmanPods.Inspect(c.conn, podID)
		if err != nil {
			return fmt.Errorf("failed to inspect pod %s: %w", podID, err)
		}
		if pod.State == "Running" {
			log.Printf("[Podman] Pod %s is running", podID)
			return nil
		}
		time.Sleep(2 * time.Second)
	}
}
