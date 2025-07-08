package main

import (
	"context"
	"fmt"
	"log"
	"os"
	"strconv"
	"time"

	"beemesh/pkg/machine"
	"beemesh/pkg/proxy"
	"beemesh/pkg/registry"
	"beemesh/pkg/types"
	"github.com/google/uuid"
	"github.com/libp2p/go-libp2p/core/peer"
	"github.com/multiformats/go-multiaddr"
	"k8s.io/api/apps/v1"
	"sigs.k8s.io/yaml"
)

func main() {
	nodeID := os.Getenv("BEEMESH_WORKLOAD_NODE_ID")
	if nodeID == "" {
		log.Fatal("BEEMESH_WORKLOAD_NODE_ID not set")
	}
	parentKind := os.Getenv("BEEMESH_WORKLOAD_KIND")
	parentName := os.Getenv("BEEMESH_WORKLOAD_NAME")
	desiredReplicasStr := os.Getenv("BEEMESH_WORKLOAD_REPLICAS")
	proxyPort := os.Getenv("BEEMESH_WORKLOAD_PROXY_PORT")
	serviceName := os.Getenv("BEEMESH_WORKLOAD_SERVICE")

	desiredReplicas, err := strconv.Atoi(desiredReplicasStr)
	if err != nil {
		log.Fatalf("Invalid BEEMESH_WORKLOAD_REPLICAS: %v", err)
	}

	ctx := context.Background()
	client, err := machine.NewClient(ctx, nodeID)
	if err != nil {
		log.Fatalf("Failed to create Machine-Plane client: %v", err)
	}
	defer client.Close()

	registry, err := registry.NewRegistry(ctx, client, nodeID)
	if err != nil {
		log.Fatalf("Failed to create service registry: %v", err)
	}
	defer registry.Close()

	maddr, err := multiaddr.NewMultiaddr(fmt.Sprintf("/ip4/127.0.0.1/tcp/%s/p2p/%s", proxyPort, client.Host().ID().String()))
	if err != nil {
		log.Fatalf("Failed to create libp2p multiaddr: %v", err)
	}
	service := types.Service{
		Name:       fmt.Sprintf("%s-%s", parentKind, parentName),
		ProtocolID: "/beemesh/sidecar/stateless/1.0",
		IP:         serviceName,
		Port:       8080,
		Libp2pAddr: maddr.String(),
	}
	if err := registry.RegisterService(ctx, service); err != nil {
		log.Fatalf("Failed to register stateless sidecar service: %v", err)
	}

	client.ReceiveTasks(ctx, func(task types.Task) {
		log.Printf("Stateless sidecar received task %s: %s/%s", task.TaskID, task.Kind, task.Name)
	})

	go monitorReplicas(ctx, client, registry, parentName, desiredReplicas)

	proxy, err := proxy.NewProxy(registry, parentKind, parentName, desiredReplicasStr, 80)
	if err != nil {
		log.Fatalf("Failed to create proxy: %v", err)
	}
	log.Printf("Running stateless sidecar: %s, proxying to localhost:80", nodeID)
	log.Fatal(proxy.ListenAndServe(proxyPort))
}

func monitorReplicas(ctx context.Context, client *machine.Client, reg *registry.Registry, workloadName string, desiredReplicas int) {
	ticker := time.NewTicker(30 * time.Second)
	defer ticker.Stop()

	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			addresses, err := reg.ResolveService(ctx, "/beemesh/sidecar/stateless/1.0")
			if err != nil {
				log.Printf("Failed to query replicas: %v", err)
				continue
			}
			activeReplicas := 0
			for _, addr := range addresses {
				if addr != "" {
					activeReplicas++
				}
			}
			if activeReplicas < desiredReplicas {
				log.Printf("Detected %d/%d replicas for %s, cloning %d", activeReplicas, desiredReplicas, workloadName, desiredReplicas-activeReplicas)
				manifestBytes, err := reg.GetManifest(ctx, workloadName)
				if err != nil {
					log.Printf("Failed to get manifest for %s: %v", workloadName, err)
					continue
				}
				var dep v1.Deployment
				if err := yaml.Unmarshal(manifestBytes, &dep); err != nil {
					log.Printf("Failed to parse manifest: %v", err)
					continue
				}
				for i := 0; i < desiredReplicas-activeReplicas; i++ {
					task := types.Task{
						TaskID:        uuid.New().String(),
						Kind:          "StatelessWorkload",
						Name:          workloadName,
						Spec:          &dep,
						Destination:   "scheduler",
						CPURequest:    100,
						MemoryRequest: 50 * 1024 * 1024,
						CloneRequest:  true,
					}
					if err := client.PublishTask(ctx, task); err != nil {
						log.Printf("Failed to publish clone task %s: %v", task.TaskID, err)
					} else {
						log.Printf("Published clone task %s for %s", task.TaskID, workloadName)
					}
				}
			}
		}
	}
}
