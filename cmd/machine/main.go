package main

import (
	"beemesh/pkg/machine/podman"
	"beemesh/pkg/machine/scheduler"
	"beemesh/pkg/types"
	"context"
	"encoding/json"
	"fmt"
	"log"
	"net/http"
	"os"
	"os/signal"
	"strings"
	"time"

	"github.com/gorilla/mux"
	"github.com/prometheus/client_golang/promhttp"

	// libp2p imports
	"github.com/libp2p/go-libp2p"
	dht "github.com/libp2p/go-libp2p-kad-dht"
	pubsub "github.com/libp2p/go-libp2p-pubsub"
	"github.com/libp2p/go-libp2p/core/host"
	"github.com/libp2p/go-libp2p/core/peer"
	discovery "github.com/libp2p/go-libp2p/p2p/discovery/mdns"
	libp2ptls "github.com/libp2p/go-libp2p/p2p/security/tls"
)

func main() {
	// Context with graceful shutdown
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, os.Kill)
	defer stop()

	nodeID := os.Getenv("BEEMESH_NODE_ID")
	if nodeID == "" {
		log.Fatal("BEEMESH_NODE_ID environment variable is required")
	}

	// Create a basic libp2p host
	machineHost, err := libp2p.New(
		libp2p.Security(libp2ptls.ID, libp2ptls.New),
	)
	if err != nil {
		log.Fatalf("Failed to create libp2p host: %v", err)
	}
	defer machineHost.Close()

	log.Printf("Libp2p host started. ID: %s", machineHost.ID().Pretty())

	// Setup GossipSub pubsub
	ps, err := pubsub.NewGossipSub(ctx, machineHost)
	if err != nil {
		log.Fatalf("Failed to create pubsub: %v", err)
	}

	// Join pubsub topics
	topicTasks, _ := ps.Join("scheduler-tasks")
	topicProposals, _ := ps.Join("scheduler-proposals")

	// Subscribe to task topic
	taskSub, err := topicTasks.Subscribe()
	if err != nil {
		log.Fatalf("Failed to subscribe to tasks topic: %v", err)
	}

	// --- NEW CODE: Subscribe to the 'scheduler-proposals' topic ---
	proposalSub, err := topicProposals.Subscribe()
	if err != nil {
		log.Fatalf("Failed to subscribe to proposals topic: %v", err)
	}

	// Create a buffered channel for proposals.
	proposalsChan := make(chan types.Proposal, 100)

	// Start a goroutine to feed proposals into the channel.
	go func() {
		log.Println("[Proposals] Listening for incoming proposals...")
		for {
			msg, err := proposalSub.Next(ctx)
			if err != nil {
				if ctx.Err() != nil {
					log.Println("[Proposals] Context cancelled, stopping listener")
					close(proposalsChan) // Signal to scheduler that no more proposals are coming.
					return
				}
				log.Printf("[Proposals] Read error: %v", err)
				time.Sleep(500 * time.Millisecond)
				continue
			}

			var proposal types.Proposal
			if err := json.Unmarshal(msg.Data, &proposal); err != nil {
				log.Printf("[Proposals] Failed to unmarshal proposal: %v", err)
				continue
			}

			// Send the proposal to the channel.
			select {
			case proposalsChan <- proposal:
				// Successfully sent.
			case <-ctx.Done():
				log.Println("[Proposals] Context cancelled while sending proposal")
				close(proposalsChan)
				return
			}
		}
	}()

	// DHT bootstrap peers
	var bootstrapPeers []peer.AddrInfo
	if peers := os.Getenv("BEEMESH_BOOTSTRAP_PEERS"); peers != "" {
		for _, s := range strings.Split(peers, ",") {
			s = strings.TrimSpace(s)
			if s == "" {
				continue
			}
			ai, err := peer.AddrInfoFromString(s)
			if err != nil {
				log.Printf("[DHT] Invalid peer %s: %v", s, err)
				continue
			}
			bootstrapPeers = append(bootstrapPeers, ai)
		}
	}

	// Create and bootstrap the DHT
	machineDHT, err := dht.New(ctx, machineHost,
		dht.BootstrapPeers(bootstrapPeers...),
		dht.Mode(dht.ModeServer),
	)
	if err != nil {
		log.Fatalf("Failed to create DHT: %v", err)
	}
	defer machineDHT.Close()

	if err := machineDHT.Bootstrap(ctx); err != nil {
		log.Fatalf("Failed to bootstrap DHT: %v", err)
	}

	// Optional mDNS discovery for local peers
	if os.Getenv("BEEMESH_ENABLE_MDNS") == "true" {
		service, err := discovery.NewMdnsService(machineHost, "_beemesh-machine._udp", &discoveryNotifee{h: machineHost, ctx: ctx})
		if err != nil {
			log.Printf("[mDNS] Failed to create service: %v", err)
		} else {
			if err := service.Start(); err != nil {
				log.Printf("[mDNS] Failed to start service: %v", err)
			} else {
				defer service.Close()
				log.Println("[mDNS] Discovery enabled")
			}
		}
	}

	// Podman client
	socketPath := os.Getenv("PODMAN_SOCKET")
	if socketPath == "" {
		socketPath = "unix:///run/podman/podman.sock"
	}

	podmanClient, err := podman.NewClient(socketPath, nodeID)
	if err != nil {
		log.Fatalf("Failed to create Podman client: %v", err)
	}
	defer podmanClient.Close()

	// Scheduler
	sched := scheduler.NewScheduler(nil) // directly pass nil if not wrapping
	sched.SetPodmanClient(podmanClient)

	// Listen for tasks from pubsub
	go func() {
		log.Println("[Tasks] Listening for incoming tasks...")
		for {
			msg, err := taskSub.Next(ctx)
			if err != nil {
				if ctx.Err() != nil {
					log.Println("[Tasks] Context cancelled, stopping listener")
					return
				}
				log.Printf("[Tasks] Read error: %v", err)
				time.Sleep(500 * time.Millisecond)
				continue
			}
			var task types.Task
			if err := json.Unmarshal(msg.Data, &task); err != nil {
				log.Printf("[Tasks] Failed to unmarshal task: %v", err)
				continue
			}
			// Handle tasks directly
			switch {
			case task.Destination == "scheduler":
				// --- MODIFIED CODE: Pass the proposalsChan to the scheduler ---
				log.Printf("[Tasks] Received scheduler task: %s", task.TaskID)
				// We pass 'nil' for the Node client since it's not used in the current scheduler logic.
				sched := scheduler.NewScheduler(nil)
				sched.SetPodmanClient(podmanClient)
				// Call DistributedSchedule in a new goroutine to avoid blocking the task listener.
				go func(t types.Task) {
					sched.DistributedSchedule(ctx, t, proposalsChan)
				}(task)
				// --- END OF MODIFIED CODE ---
			case task.Destination == "", task.Destination == "all", task.Destination == nodeID:
				// Direct deployment
				if err := podmanClient.DeployWorkloadFromTask(task); err != nil {
					log.Printf("[Tasks] Failed to deploy task %s: %v", task.TaskID, err)
				} else {
					log.Printf("[Tasks] Successfully deployed task %s", task.TaskID)
				}
			default:
				log.Printf("[Tasks] Ignoring task %s with unknown destination %s", task.TaskID, task.Destination)
			}
		}
	}()

	router := mux.NewRouter()
	router.Handle("/metrics", promhttp.Handler())
	router.HandleFunc("/api/v1/namespaces/{namespace}/pods", func(w http.ResponseWriter, r *http.Request) {
		namespace := mux.Vars(r)["namespace"]
		fmt.Fprintf(w, "Listing pods in namespace %s (stub)", namespace)
	}).Methods("GET")

	router.HandleFunc("/v1/publish_task", func(w http.ResponseWriter, r *http.Request) {
		var task types.Task
		if err := json.NewDecoder(r.Body).Decode(&task); err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}
		data, _ := json.Marshal(task)
		if err := topicTasks.Publish(ctx, data); err != nil {
			http.Error(w, fmt.Sprintf("Failed to publish task: %v", err), http.StatusInternalServerError)
			return
		}
		fmt.Fprintf(w, "Task published: %s", task.TaskID)
	}).Methods("POST")

	server := &http.Server{Addr: ":8080", Handler: router}
	go func() {
		log.Println("API server listening on :8080")
		if err := server.ListenAndServe(); err != nil && err != http.ErrServerClosed {
			log.Fatalf("HTTP server error: %v", err)
		}
	}()

	<-ctx.Done()
	log.Println("Shutdown signal received. Stopping services...")

	shutdownCtx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := server.Shutdown(shutdownCtx); err != nil {
		log.Printf("HTTP server forced to shutdown: %v", err)
	}

	log.Println("Node exited cleanly.")
}

type discoveryNotifee struct {
	h   host.Host
	ctx context.Context
}

func (n *discoveryNotifee) HandlePeerFound(pi peer.AddrInfo) {
	go func() {
		ctx, cancel := context.WithTimeout(n.ctx, 10*time.Second)
		defer cancel()
		if err := n.h.Connect(ctx, pi); err != nil {
			log.Printf("[mDNS] Failed to connect to peer %s: %v", pi.ID, err)
		} else {
			log.Printf("[mDNS] Connected to peer %s", pi.ID)
		}
	}()
}
