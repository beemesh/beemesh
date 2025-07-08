package registry

import (
	"context"
	"fmt"
	"log"

	"beemesh/pkg/types"
	"github.com/libp2p/go-libp2p/core/host"
	"github.com/libp2p/go-libp2p/core/peer"
	dht "github.com/libp2p/go-libp2p-kad-dht"
)

type Registry struct {
	client *Client
	dht    *dht.IpfsDHT
	nodeID string
}

func NewRegistry(ctx context.Context, client *Client, nodeID string) (*Registry, error) {
	dht, err := dht.New(ctx, client.Host())
	if err != nil {
		return nil, fmt.Errorf("failed to create DHT: %v", err)
	}
	if err := dht.Bootstrap(ctx); err != nil {
		return nil, fmt.Errorf("failed to bootstrap DHT: %v", err)
	}
	return &Registry{client: client, dht: dht, nodeID: nodeID}, nil
}

func (c *Client) Host() host.Host {
	return c.host
}

func (r *Registry) Client() *Client {
	return r.client
}

func (r *Registry) Close() error {
	return r.dht.Close()
}

func (r *Registry) RegisterService(ctx context.Context, service types.Service) error {
	key := fmt.Sprintf("%s/%s", service.ProtocolID, service.Name)
	value := []byte(service.Libp2pAddr)
	if err := r.dht.PutValue(ctx, key, value); err != nil {
		return fmt.Errorf("failed to register service %s: %v", service.Name, err)
	}
	log.Printf("Registered service %s at %s", service.Name, service.Libp2pAddr)
	return nil
}

func (r *Registry) ResolveService(ctx context.Context, protocolID string) ([]string, error) {
	records, err := r.dht.FindProviders(ctx, protocolID)
	if err != nil {
		return nil, fmt.Errorf("failed to resolve service %s: %v", protocolID, err)
	}
	addresses := make([]string, 0, len(records))
	for _, record := range records {
		for _, addr := range record.Addrs {
			addresses = append(addresses, addr.String())
		}
	}
	return addresses, nil
}

func (r *Registry) StoreManifest(ctx context.Context, workloadName string, manifest []byte) error {
	key := fmt.Sprintf("/beemesh/manifest/%s", workloadName)
	if err := r.dht.PutValue(ctx, key, manifest); err != nil {
		return fmt.Errorf("failed to store manifest %s: %v", workloadName, err)
	}
	log.Printf("Stored manifest for %s in DHT", workloadName)
	return nil
}

func (r *Registry) GetManifest(ctx context.Context, workloadName string) ([]byte, error) {
	key := fmt.Sprintf("/beemesh/manifest/%s", workloadName)
	value, err := r.dht.GetValue(ctx, key)
	if err != nil {
		return nil, fmt.Errorf("failed to get manifest %s: %v", workloadName, err)
	}
	return value, nil
}
