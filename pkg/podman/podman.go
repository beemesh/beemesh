package podman

import (
	"context"
	"fmt"
	"log"
	"os"
	"strings"

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

func NewClient() (*Client, error) {
	conn, err := bindings.NewConnection(context.Background(), "unix:///run/podman/podman.sock")
	if err != nil {
		return nil, fmt.Errorf("failed to create Podman connection: %v", err)
	}
	nodeID := os.Getenv("BEEMESH_NODE_ID")
	if nodeID == "" {
		return nil, fmt.Errorf("BEEMESH_NODE_ID not set")
	}
	return &Client{conn: conn, ctx: context.Background(), nodeID: nodeID}, nil
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
		return c.deployPodmanPod(dep.Name, dep.Spec.Template.Spec, int(*dep.Spec.Replicas), false)
	case "StatefulWorkload":
		ss, ok := task.Spec.(*v1.StatefulSet)
		if !ok {
			return fmt.Errorf("invalid stateful workload spec")
		}
		return c.deployPodmanPod(ss.Name, ss.Spec.Template.Spec, int(*ss.Spec.Replicas), true)
	default:
		return fmt.Errorf("unsupported kind: %s", task.Kind)
	}
}

func (c *Client) deployPodmanPod(name string, spec v1.PodSpec, replicas int, stateful bool) error {
	for i := 0; i < replicas; i++ {
		podName := name
		if stateful {
			podName = fmt.Sprintf("%s-%d", name, i)
		}
		podSpec := entities.PodCreateOptions{
			Name:      podName,
			Network:   []string{"private"},
			Labels:    map[string]string{"beemesh.workload": name},
			Infra:     false,
			Share:     []string{"none"},
			ShareParent: false,
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
				Name:       fmt.Sprintf("%s-%s", podName, container.Name),
				Image:      container.Image,
				Command:    container.Command,
				Env:        env,
				Ports:      []entities.ContainerPort{},
				Mounts:     volumeMounts,
				Pod:        podName,
				Labels:     podSpec.Labels,
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
