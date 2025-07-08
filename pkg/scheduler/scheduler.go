package scheduler

import (
	"context"
	"fmt"
	"log"
	"time"

	"beemesh/pkg/machine"
	"beemesh/pkg/registry"
	"beemesh/pkg/types"
	"github.com/google/uuid"
	"k8s.io/api/apps/v1"
	"sigs.k8s.io/yaml"
)

type Scheduler struct {
	client *machine.Client
}

func NewScheduler(client *machine.Client) *Scheduler {
	return &Scheduler{client: client}
}

func (s *Scheduler) Schedule(ctx context.Context, task types.Task) error {
	if task.Destination != "scheduler" {
		return s.client.PublishTask(ctx, task)
	}
	metricsChan := s.client.ReceiveMetrics(ctx)
	var metrics []types.HostMetrics
	timeout := time.After(5 * time.Second)
	select {
	case m := <-metricsChan:
		metrics = append(metrics, m)
	case <-timeout:
		return fmt.Errorf("no metrics available for scheduling")
	}
	var targetNode string
	for _, m := range metrics {
		if m.CPUFree >= task.CPURequest && m.MemoryFree >= task.MemoryRequest {
			targetNode = m.NodeID
			break
		}
	}
	if targetNode == "" {
		return fmt.Errorf("no suitable node found for task %s", task.TaskID)
	}
	task.Destination = targetNode
	log.Printf("Scheduled task %s to node %s", task.TaskID, targetNode)
	return s.client.PublishTask(ctx, task)
}

func (s *Scheduler) ProcessWorkload(ctx context.Context, manifest []byte, kind string) error {
	taskID := uuid.New().String()
	var task types.Task
	switch kind {
	case "stateless":
		var dep v1.Deployment
		if err := yaml.Unmarshal(manifest, &dep); err != nil {
			return fmt.Errorf("failed to parse Deployment: %v", err)
		}
		task = types.Task{
			TaskID:        taskID,
			Kind:          "StatelessWorkload",
			Name:          dep.Name,
			Spec:          &dep,
			Destination:   "scheduler",
			CPURequest:    100,
			MemoryRequest: 50 * 1024 * 1024,
		}
		if err := s.storeManifest(ctx, dep.Name, manifest); err != nil {
			return fmt.Errorf("failed to store manifest: %v", err)
		}
	case "stateful":
		var ss v1.StatefulSet
		if err := yaml.Unmarshal(manifest, &ss); err != nil {
			return fmt.Errorf("failed to parse StatefulSet: %v", err)
		}
		task = types.Task{
			TaskID:        taskID,
			Kind:          "StatefulWorkload",
			Name:          ss.Name,
			Spec:          &ss,
			Destination:   "scheduler",
			CPURequest:    200,
			MemoryRequest: 100 * 1024 * 1024,
		}
		if err := s.storeManifest(ctx, ss.Name, manifest); err != nil {
			return fmt.Errorf("failed to store manifest: %v", err)
		}
	default:
		return fmt.Errorf("unsupported kind: %s", kind)
	}
	return s.Schedule(ctx, task)
}

func (s *Scheduler) storeManifest(ctx context.Context, workloadName string, manifest []byte) error {
	registry, err := registry.NewRegistry(ctx, s.client, s.client.nodeID)
	if err != nil {
		return fmt.Errorf("failed to create registry: %v", err)
	}
	defer registry.Close()
	return registry.StoreManifest(ctx, workloadName, manifest)
}
