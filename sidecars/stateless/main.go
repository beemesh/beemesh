package main

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"github.com/google/uuid"
	"io"
	"k8s.io/api/apps/v1"
	"log"
	"net/http"
	"net/url"
	"os"
	"sigs.k8s.io/yaml"
	"strconv"
	"time"

	"beemesh/pkg/proxy"
	"beemesh/pkg/types"
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
	namespace := os.Getenv("BEEMESH_NAMESPACE")
	if namespace == "" {
		namespace = "default"
	}

	desiredReplicas, err := strconv.Atoi(desiredReplicasStr)
	if err != nil {
		log.Fatalf("Invalid BEEMESH_WORKLOAD_REPLICAS: %v", err)
	}

	ctx := context.Background()

	service := types.Service{
		Name:       fmt.Sprintf("%s-%s", parentKind, parentName),
		ProtocolID: "/beemesh/sidecar/stateless/1.0",
		IP:         serviceName,
		Port:       8080,
		Libp2pAddr: nodeID,
	}
	data, err := json.Marshal(service)
	if err != nil {
		log.Fatalf("Failed to marshal service: %v", err)
	}
	req, err := http.NewRequest("POST", "http://host.containers.internal:8080/v1/register_service", bytes.NewReader(data))
	if err != nil {
		log.Fatalf("Failed to create request: %v", err)
	}
	req.Header.Set("X-Namespace", namespace)
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		log.Fatalf("Failed to register service: %v", err)
	}
	resp.Body.Close()

	go monitorReplicas(ctx, parentName, desiredReplicas, namespace)

	proxy, err := proxy.NewProxy(parentKind, parentName, desiredReplicasStr, 80)
	if err != nil {
		log.Fatalf("Failed to create proxy: %v", err)
	}
	log.Printf("Running stateless sidecar: %s, proxying to localhost:80 in namespace %s", nodeID, namespace)
	log.Fatal(proxy.ListenAndServe(proxyPort))
}

func monitorReplicas(ctx context.Context, workloadName string, desiredReplicas int, namespace string) {
	ticker := time.NewTicker(30 * time.Second)
	defer ticker.Stop()

	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			u, _ := url.Parse("http://host.containers.internal:8080/v1/resolve_service")
			q := u.Query()
			q.Set("protocol_id", "/beemesh/sidecar/stateless/1.0")
			u.RawQuery = q.Encode()
			req, err := http.NewRequest("GET", u.String(), nil)
			if err != nil {
				log.Printf("Failed to create request: %v", err)
				continue
			}
			req.Header.Set("X-Namespace", namespace)
			resp, err := http.DefaultClient.Do(req)
			if err != nil {
				log.Printf("Failed to query replicas: %v", err)
				continue
			}
			body, err := io.ReadAll(resp.Body)
			resp.Body.Close()
			if err != nil {
				log.Printf("Failed to read response: %v", err)
				continue
			}
			var addresses []string
			if err := json.Unmarshal(body, &addresses); err != nil {
				log.Printf("Failed to unmarshal addresses: %v", err)
				continue
			}
			activeReplicas := 0
			for _, addr := range addresses {
				if addr != "" {
					activeReplicas++
				}
			}
			if activeReplicas < desiredReplicas {
				log.Printf("Detected %d/%d replicas for %s, cloning %d in namespace %s", activeReplicas, desiredReplicas, workloadName, desiredReplicas-activeReplicas, namespace)
				u, _ = url.Parse("http://host.containers.internal:8080/v1/get_manifest")
				q = u.Query()
				q.Set("workload_name", workloadName)
				u.RawQuery = q.Encode()
				req, err = http.NewRequest("GET", u.String(), nil)
				if err != nil {
					log.Printf("Failed to create request: %v", err)
					continue
				}
				req.Header.Set("X-Namespace", namespace)
				resp, err = http.DefaultClient.Do(req)
				if err != nil {
					log.Printf("Failed to get manifest for %s: %v", workloadName, err)
					continue
				}
				manifestBytes, err := io.ReadAll(resp.Body)
				resp.Body.Close()
				if err != nil {
					log.Printf("Failed to read manifest: %v", err)
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
					data, err := json.Marshal(task)
					if err != nil {
						log.Printf("Failed to marshal task: %v", err)
						continue
					}
					req, err = http.NewRequest("POST", "http://host.containers.internal:8080/v1/publish_task", bytes.NewReader(data))
					if err != nil {
						log.Printf("Failed to create request: %v", err)
						continue
					}
					req.Header.Set("X-Namespace", namespace)
					resp, err = http.DefaultClient.Do(req)
					if err != nil {
						log.Printf("Failed to publish clone task %s: %v", task.TaskID, err)
					} else {
						resp.Body.Close()
						log.Printf("Published clone task %s for %s in namespace %s", task.TaskID, workloadName, namespace)
					}
				}
			}
		}
	}
}
