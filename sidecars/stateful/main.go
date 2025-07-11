package main

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"github.com/google/uuid"
	"github.com/hashicorp/raft"
	"io"
	"k8s.io/api/apps/v1"
	"log"
	"net/http"
	"net/url"
	"os"
	"sigs.k8s.io/yaml"
	"strconv"
	"strings"
	"time"

	"beemesh/pkg/proxy"
	"beemesh/pkg/types"
)

func main() {
	nodeID := os.Getenv("BEEMESH_RAFT_NODE_ID")
	if nodeID == "" {
		log.Fatal("BEEMESH_RAFT_NODE_ID not set")
	}
	parentKind := os.Getenv("BEEMESH_WORKLOAD_KIND")
	parentName := os.Getenv("BEEMESH_WORKLOAD_NAME")
	desiredReplicasStr := os.Getenv("BEEMESH_WORKLOAD_REPLICAS")
	proxyPort := os.Getenv("BEEMESH_WORKLOAD_PROXY_PORT")
	raftPeers := os.Getenv("BEEMESH_RAFT_PEERS")
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
		ProtocolID: "/beemesh/sidecar/stateful/1.0",
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

	raftConfig := raft.DefaultConfig()
	raftConfig.LocalID = raft.ServerID(nodeID)
	raftNode, err := raft.NewRaft(raftConfig, nil, nil, nil, nil) // Placeholder, implement fully
	if err != nil {
		log.Fatalf("Failed to initialize Raft: %v", err)
	}

	go monitorRaftPeers(ctx, parentName, desiredReplicas, raftPeers, raftNode, namespace)

	proxy, err := proxy.NewProxy(parentKind, parentName, desiredReplicasStr, 8080)
	if err != nil {
		log.Fatalf("Failed to create proxy: %v", err)
	}
	log.Printf("Running stateful sidecar: %s, proxying to localhost:8080 and Raft to localhost:7000 in namespace %s", nodeID, namespace)
	log.Fatal(proxy.ListenAndServe(proxyPort))
}

func monitorRaftPeers(ctx context.Context, workloadName string, desiredReplicas int, raftPeers string, raftNode *raft.Raft, namespace string) {
	ticker := time.NewTicker(30 * time.Second)
	defer ticker.Stop()

	peerList := strings.Split(raftPeers, ",")
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			u, _ := url.Parse("http://host.containers.internal:8080/v1/resolve_service")
			q := u.Query()
			q.Set("protocol_id", "/beemesh/sidecar/stateful/1.0")
			u.RawQuery = q.Encode()
			req, err := http.NewRequest("GET", u.String(), nil)
			if err != nil {
				log.Printf("Failed to create request: %v", err)
				continue
			}
			req.Header.Set("X-Namespace", namespace)
			resp, err := http.DefaultClient.Do(req)
			if err != nil {
				log.Printf("Failed to query Raft peers: %v", err)
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
			activePeers := make(map[string]bool)
			for _, addr := range addresses {
				if addr != "" {
					peerName := strings.Split(addr, ".")[0]
					activePeers[peerName] = true
				}
			}
			hasQuorum := raftNode.State() == raft.Leader || raftNode.State() == raft.Follower
			if len(activePeers) < desiredReplicas || !hasQuorum {
				log.Printf("Detected %d/%d Raft peers for %s, quorum: %v, cloning missing in namespace %s", len(activePeers), desiredReplicas, workloadName, hasQuorum, namespace)
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
				var ss v1.StatefulSet
				if err := yaml.Unmarshal(manifestBytes, &ss); err != nil {
					log.Printf("Failed to parse manifest: %v", err)
					continue
				}
				for i := 0; i < desiredReplicas; i++ {
					podName := fmt.Sprintf("%s-%d", workloadName, i)
					if !activePeers[podName] {
						task := types.Task{
							TaskID:        uuid.New().String(),
							Kind:          "StatefulWorkload",
							Name:          podName,
							Spec:          &ss,
							Destination:   "scheduler",
							CPURequest:    200,
							MemoryRequest: 100 * 1024 * 1024,
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
							log.Printf("Published clone task %s for %s in namespace %s", task.TaskID, podName, namespace)
						}
					}
				}
			}
		}
	}
}
