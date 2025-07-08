package main

import (
	"context"
	"fmt"
	"log"
	"os"
	"strings"
	"time"

	"beemesh/pkg/machine"
	"beemesh/pkg/proxy"
	"beemesh/pkg/registry"
	"beemesh/pkg/types"
	"github.com/google/uuid"
	"github.com/hashicorp/raft"
	"github.com/libp2p/go-libp2p/core/network"
	"github.com/libp2p/go-libp2p/core/peer"
	"github.com/multiformats/go-multiaddr"
	"k8s.io/api/apps/v1"
	"sigs.k8s.io/yaml"
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
		ProtocolID: "/beemesh/sidecar/stateful/1.0",
		IP:         serviceName,
		Port:       8080,
		Libp2pAddr: maddr.String(),
	}
	if err := registry.RegisterService(ctx, service); err != nil {
		log.Fatalf("Failed to register stateful sidecar service: %v", err)
	}

	raftConfig := raft.DefaultConfig()
	raftConfig.LocalID = raft.ServerID(nodeID)
	raftNode, err := raft.NewRaft(raftConfig, nil, nil, nil, nil)
	if err != nil {
		log.Fatalf("Failed to initialize Raft: %v", err)
	}

	client.Host().SetStreamHandler("/beemesh/raft/1.0", func(s network.Stream) {
		localConn, err := net.Dial("tcp", "localhost:7000")
		if err != nil {
			log.Printf("Failed to connect to local Raft: %v", err)
			s.Close()
			return
		}
		defer localConn.Close()
		go io.Copy(localConn, s)
		io.Copy(s, localConn)
	})

	client.ReceiveTasks(ctx, func(task types.Task) {
		log.Printf("Stateful sidecar received task %s: %s/%s", task.TaskID, task.Kind, task.Name)
	})

	go monitorRaftPeers(ctx, client, registry, parentName, desiredReplicas, raftPeers, raftNode)

	proxy, err := proxy.NewProxy(registry, parentKind, parentName, desiredReplicasStr, 8080)
	if err != nil {
		log.Fatalf("Failed to create proxy: %v", err)
	}
	log.Printf("Running stateful sidecar: %s, proxying to localhost:8080 and Raft to localhost:7000", nodeID)
	log.Fatal(proxy.ListenAndServe(proxyPort))
}

func monitorRaftPeers(ctx context.Context, client *machine.Client, reg *registry.Registry, workloadName string, desiredReplicas int, raftPeers string, raftNode *raft.Raft) {
	ticker := time.NewTicker(30 * time.Second)
	defer ticker.Stop()

	peerList := strings.Split(raftPeers, ",")
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			addresses, err := reg.ResolveService(ctx, "/beemesh/sidecar/stateful/1.0")
			if err != nil {
				log.Printf("Failed to query Raft peers: %v", err)
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
				log.Printf("Detected %d/%d Raft peers for %s, quorum: %v, cloning missing", len(activePeers), desiredReplicas, workloadName, hasQuorum)
				manifestBytes, err := reg.GetManifest(ctx, workloadName)
				if err != nil {
					log.Printf("Failed to get manifest for %s: %v", workloadName, err)
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
						if err := client.PublishTask(ctx, task); err != nil {
							log.Printf("Failed to publish clone task %s: %v", task.TaskID, err)
						} else {
							log.Printf("Published clone task %s for %s", task.TaskID, podName)
						}
					}
				}
			}
		}
	}
}
