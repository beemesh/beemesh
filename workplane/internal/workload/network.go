package workload

import (
	"context"
	"fmt"
	"strings"

	libp2p "github.com/libp2p/go-libp2p"
	dht "github.com/libp2p/go-libp2p-kad-dht"
	"github.com/libp2p/go-libp2p/core/crypto"
	"github.com/libp2p/go-libp2p/core/host"
	"github.com/libp2p/go-libp2p/core/peer"
	libp2ptls "github.com/libp2p/go-libp2p/p2p/security/tls"
	"github.com/libp2p/go-libp2p/p2p/transport/tcp"
)

// Network owns the libp2p host and the Workload DHT.
type Network struct {
	Host host.Host
	DHT  *dht.IpfsDHT

	Namespace    string
	WorkloadName string
}

// NewNetwork creates a TLS-authenticated libp2p host and a Kademlia DHT (server mode), then bootstraps it.
func NewNetwork(ctx context.Context, cfg Config) (*Network, error) {
	priv, err := crypto.UnmarshalPrivateKey(cfg.PrivateKey)
	if err != nil {
		return nil, fmt.Errorf("unmarshal private key: %w", err)
	}

	h, err := libp2p.New(
		libp2p.Identity(priv),
		libp2p.Security(libp2ptls.ID, libp2ptls.New),
		libp2p.Transport(tcp.NewTCPTransport),
	)
	if err != nil {
		return nil, fmt.Errorf("libp2p host: %w", err)
	}

	var boots []peer.AddrInfo
	for _, raw := range cfg.BootstrapPeerStrings {
		raw = strings.TrimSpace(raw)
		if raw == "" {
			continue
		}
		ai, err := peer.AddrInfoFromString(raw)
		if err != nil {
			// keep going; invalid entries shouldn't abort the process
			continue
		}
		boots = append(boots, ai)
	}

	kdht, err := dht.New(ctx, h, dht.BootstrapPeers(boots...), dht.Mode(dht.ModeServer))
	if err != nil {
		_ = h.Close()
		return nil, fmt.Errorf("workload dht: %w", err)
	}
	if err := kdht.Bootstrap(ctx); err != nil {
		_ = kdht.Close()
		_ = h.Close()
		return nil, fmt.Errorf("bootstrap dht: %w", err)
	}

	return &Network{
		Host:         h,
		DHT:          kdht,
		Namespace:    cfg.Namespace,
		WorkloadName: cfg.WorkloadName,
	}, nil
}

// Close shuts down host and DHT.
func (n *Network) Close() error {
	if n.DHT != nil {
		_ = n.DHT.Close()
	}
	if n.Host != nil {
		return n.Host.Close()
	}
	return nil
}
