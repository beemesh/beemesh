package main

import (
	"context"
	"encoding/base64"
	"flag"
	"io/ioutil"
	"log"
	"os"
	"os/signal"
	"syscall"

	"sigs.k8s.io/yaml"

	"beemesh/internal/workload"
)

type manifestKind struct {
	Kind string `yaml:"kind"`
}

func main() {
	// CLI flags for local development
	var (
		namespace  = flag.String("namespace", os.Getenv("BEE_NAMESPACE"), "Kubernetes namespace for this workload")
		workloadID = flag.String("workload", os.Getenv("BEE_WORKLOAD_NAME"), "Workload name")
		podID      = flag.String("pod", os.Getenv("BEE_POD_NAME"), "Pod name")
		replicas   = flag.Int("replicas", 1, "Desired replicas for self-healing")
		manifest   = flag.String("manifest", os.Getenv("BEE_MANIFEST_PATH"), "Path to workload manifest YAML")
		apiURL     = flag.String("machine-api", os.Getenv("BEE_MACHINE_API"), "Machine Plane API URL (default http://localhost:8080)")
		keyB64     = flag.String("privkey", os.Getenv("BEE_PRIVATE_KEY_B64"), "Base64 encoded workload private key")
		bootstrap  = flag.String("bootstrap", os.Getenv("BEE_BOOTSTRAP_PEERS"), "Comma-separated list of libp2p bootstrap peer multiaddrs")
	)
	flag.Parse()

	if *namespace == "" || *workloadID == "" || *podID == "" || *manifest == "" || *keyB64 == "" {
		log.Fatal("Missing required configuration. Ensure namespace, workload, pod, manifest, and privkey are set.")
	}

	// Decode private key from base64
	privKeyBytes, err := base64.StdEncoding.DecodeString(*keyB64)
	if err != nil {
		log.Fatalf("failed to decode private key: %v", err)
	}

	// Read manifest file
	manifestBytes, err := ioutil.ReadFile(*manifest)
	if err != nil {
		log.Fatalf("failed to read manifest file: %v", err)
	}

	// Detect if stateful or stateless
	var mk manifestKind
	if err := yaml.Unmarshal(manifestBytes, &mk); err != nil {
		log.Fatalf("failed to parse manifest: %v", err)
	}
	isStateful := mk.Kind == "StatefulSet"

	// Build workload config
	cfg := workload.Config{
		Namespace:            *namespace,
		WorkloadName:         *workloadID,
		PodName:              *podID,
		Replicas:             *replicas,
		PrivateKey:           privKeyBytes,
		BeemeshAPI:           *apiURL,
		BootstrapPeerStrings: splitCommaList(*bootstrap),
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	// Initialize workload plane
	w, err := workload.NewWorkload(ctx, cfg, manifestBytes, isStateful)
	if err != nil {
		log.Fatalf("failed to initialize workload: %v", err)
	}

	log.Printf("[workload] Starting workload %s/%s (pod: %s, stateful: %v)",
		cfg.Namespace, cfg.WorkloadName, cfg.PodName, isStateful)

	// Start background controllers
	w.Start(ctx)

	// Wait for shutdown signal
	waitForSignal()
	log.Println("[workload] shutting down gracefully...")
	if err := w.Close(); err != nil {
		log.Printf("error closing workload: %v", err)
	}
}

func waitForSignal() {
	ch := make(chan os.Signal, 1)
	signal.Notify(ch, syscall.SIGINT, syscall.SIGTERM)
	<-ch
}

func splitCommaList(s string) []string {
	if s == "" {
		return nil
	}
	var out []string
	for _, p := range os.ExpandEnv(s) {
		out = append(out, string(p))
	}
	return out
}
