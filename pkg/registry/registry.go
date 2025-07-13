package registry

import (
	"context"
	"fmt"
	"log"
	"os"
	"strings"

	"beemesh/pkg/types"

	"github.com/ipfs/go-cid"
	dht "github.com/libp2p/go-libp2p-kad-dht"
	"github.com/libp2p/go-libp2p/core/peer"
	ma "github.com/multiformats/go-multiaddr"
	mh "github.com/multiformats/go-multihash"
)

type Registry struct {
	client *Client
	dht    *dht.IpfsDHT
	nodeID string
}

func NewRegistry(ctx context.Context, client *Client, nodeID string) (*Registry, error) {
	var bootstrappers []peer.AddrInfo
	bootstrapStr := os.Getenv("BEEMESH_BOOTSTRAP_PEERS")
	if bootstrapStr != "" {
		for _, s := range strings.Split(bootstrapStr, ",") {
			s = strings.TrimSpace(s)
			if s == "" {
				continue
			}
			ai, err := peer.AddrInfoFromString(s)
			if err != nil {
				log.Printf("Invalid bootstrap peer %s: %v", s, err)
				continue
			}
			bootstrappers = append(bootstrappers, ai)
		}
	}
	d, err := dht.New(ctx, client.Host(), dht.BootstrapPeers(bootstrappers...))
	if err != nil {
		return nil, fmt.Errorf("failed to create DHT: %v", err)
	}
	if err := d.Bootstrap(ctx); err != nil {
		return nil, fmt.Errorf("failed to bootstrap DHT: %v", err)
	}
	return &Registry{client: client, dht: d, nodeID: nodeID}, nil
}

func (r *Registry) Client() *Client {
	return r.client
}

func (r *Registry) Close() error {
	return r.dht.Close()
}

func (r *Registry) RegisterService(ctx context.Context, namespace string, service types.Service) error {
	keyStr := fmt.Sprintf("/ns/%s/%s/%s", namespace, service.ProtocolID, service.Name)
	hashed, err := mh.Sum([]byte(keyStr), mh.SHA2_256, -1)
	if err != nil {
		return fmt.Errorf("failed to hash key: %v", err)
	}
	cidKey := cid.NewCidV1(cid.Raw, hashed)
	if err := r.dht.Provide(ctx, cidKey, true); err != nil {
		return fmt.Errorf("failed to provide service %s in namespace %s: %v", service.Name, namespace, err)
	}
	log.Printf("Provided service %s at key %s in namespace %s", service.Name, keyStr, namespace)
	return nil
}

func (r *Registry) ResolveService(ctx context.Context, namespace, protocolID string) ([]string, error) {
	keyStr := fmt.Sprintf("/ns/%s/%s", namespace, protocolID)
	hashed, err := mh.Sum([]byte(keyStr), mh.SHA2_256, -1)
	if err != nil {
		return nil, fmt.Errorf("failed to hash key: %v", err)
	}
	cidKey := cid.NewCidV1(cid.Raw, hashed)
	providers, err := r.dht.FindProviders(ctx, cidKey)
	if err != nil {
		return nil, fmt.Errorf("failed to resolve service %s in namespace %s: %v", protocolID, namespace, err)
	}
	addresses := make([]string, 0, len(providers))
	for _, p := range providers {
		for _, addr := range p.Addrs {
			peerComp, err := ma.NewComponent("p2p", p.ID.String())
			if err != nil {
				continue
			}
			fullMa := addr.Encapsulate(peerComp)
			addresses = append(addresses, fullMa.String())
		}
	}
	return addresses, nil
}

func (r *Registry) StoreManifest(ctx context.Context, namespace, workloadName string, manifest []byte) error {
	key := fmt.Sprintf("/ns/%s/beemesh/manifest/%s", namespace, workloadName)
	if err := r.dht.PutValue(ctx, key, manifest); err != nil {
		return fmt.Errorf("failed to store manifest %s in namespace %s: %v", workloadName, namespace, err)
	}
	log.Printf("Stored manifest for %s in DHT under namespace %s", workloadName, namespace)
	return nil
}

func (r *Registry) GetManifest(ctx context.Context, namespace, workloadName string) ([]byte, error) {
	key := fmt.Sprintf("/ns/%s/beemesh/manifest/%s", namespace, workloadName)
	value, err := r.dht.GetValue(ctx, key)
	if err != nil {
		return nil, fmt.Errorf("failed to get manifest %s in namespace %s: %v", workloadName, namespace, err)
	}
	return value, nil
}

func (r *Registry) StoreRBACPolicy(ctx context.Context, namespace, policyName string, policy []byte) error {
	key := fmt.Sprintf("/ns/%s/rbac/%s", namespace, policyName)
	if err := r.dht.PutValue(ctx, key, policy); err != nil {
		return fmt.Errorf("failed to store RBAC policy %s in namespace %s: %v", policyName, namespace, err)
	}
	log.Printf("Stored RBAC policy %s in DHT under namespace %s", policyName, namespace)
	return nil
}

func (r *Registry) GetRBACPolicy(ctx context.Context, namespace, policyName string) ([]byte, error) {
	key := fmt.Sprintf("/ns/%s/rbac/%s", namespace, policyName)
	value, err := r.dht.GetValue(ctx, key)
	if err != nil {
		return nil, fmt.Errorf("failed to get RBAC policy %s in namespace %s: %v", policyName, namespace, err)
	}
	return value, nil
}

func (r *Registry) StoreConfigMap(ctx context.Context, namespace, configName string, data []byte) error {
	key := fmt.Sprintf("/ns/%s/config/%s", namespace, configName)
	if err := r.dht.PutValue(ctx, key, data); err != nil {
		return fmt.Errorf("failed to store ConfigMap %s in namespace %s: %v", configName, namespace, err)
	}
	return nil
}

func (r *Registry) GetConfigMap(ctx context.Context, namespace, configName string) ([]byte, error) {
	key := fmt.Sprintf("/ns/%s/config/%s", namespace, configName)
	return r.dht.GetValue(ctx, key)
}
