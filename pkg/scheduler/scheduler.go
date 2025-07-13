package scheduler

import (
	"context"
	"fmt"
	"log"
	"sort"
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
	return s.client.PublishTask(ctx, task)
}

func (s *Scheduler) ProcessWorkload(ctx context.Context, manifest []byte, kind, namespace string) error {
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
			Replicas:      int(*dep.Spec.Replicas),
			PerCPURequest: 100,
			PerMemRequest: 50 * 1024 * 1024,
			WorkloadKind:  kind,
		}
		if err := s.storeManifest(ctx, namespace, dep.Name, manifest); err != nil {
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
			Replicas:      int(*ss.Spec.Replicas),
			PerCPURequest: 200,
			PerMemRequest: 100 * 1024 * 1024,
			WorkloadKind:  kind,
		}
		if err := s.storeManifest(ctx, namespace, ss.Name, manifest); err != nil {
			return fmt.Errorf("failed to store manifest: %v", err)
		}
	default:
		return fmt.Errorf("unsupported kind: %s", kind)
	}
	return s.Schedule(ctx, task)
}

func (s *Scheduler) storeManifest(ctx context.Context, namespace, workloadName string, manifest []byte) error {
	registry, err := registry.NewRegistry(ctx, s.client, s.client.nodeID)
	if err != nil {
		return fmt.Errorf("failed to create registry: %v", err)
	}
	defer registry.Close()
	return registry.StoreManifest(ctx, namespace, workloadName, manifest)
}
