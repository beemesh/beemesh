package workload

import (
	"context"
	"fmt"

	"github.com/ipfs/go-cid"
	dht "github.com/libp2p/go-libp2p-kad-dht"
	"github.com/libp2p/go-libp2p/core/peer"
	"github.com/multiformats/go-multihash"
)

// dhtKeyForPeer stores a peer's primary multiaddr under a simple namespace key.
func dhtKeyForPeer(namespace, workload, peerID string) string {
	return fmt.Sprintf("/beemesh/workload/peer/%s/%s/%s", namespace, workload, peerID)
}

// dhtCIDForService creates a content key used for Provide/FindProviders.
func dhtCIDForService(namespace, workload string) (cid.Cid, error) {
	key := fmt.Sprintf("/ns/%s/workload/service/%s", namespace, workload)
	h, err := multihash.Sum([]byte(key), multihash.SHA2_256, -1)
	if err != nil {
		return cid.Cid{}, err
	}
	return cid.NewCidV1(cid.Raw, h), nil
}

// RegisterService records this host under the service discovery namespace (Provide) and stores an address hint in the DHT.
func RegisterService(ctx context.Context, d *dht.IpfsDHT, h peer.AddrInfo, namespace, workload string) error {
	addrs, err := peer.AddrInfoToP2pAddrs(&h)
	if err != nil {
		return fmt.Errorf("addr info: %w", err)
	}
	if len(addrs) == 0 {
		return fmt.Errorf("no addresses to publish")
	}
	// Store a simple pointer from peer key -> first addr (hint)
	if err := d.PutValue(ctx, dhtKeyForPeer(namespace, workload, h.ID.String()), []byte(addrs[0].String())); err != nil {
		return fmt.Errorf("put peer hint: %w", err)
	}
	// Provide under service key so others can find providers
	c, err := dhtCIDForService(namespace, workload)
	if err != nil {
		return err
	}
	return d.Provide(ctx, c, true)
}

// FindServicePeers lists provider peers for a given service.
func FindServicePeers(ctx context.Context, d *dht.IpfsDHT, namespace, workload string) ([]peer.AddrInfo, error) {
	c, err := dhtCIDForService(namespace, workload)
	if err != nil {
		return nil, err
	}
	providers, err := d.FindProviders(ctx, c)
	if err != nil {
		return nil, err
	}
	return providers, nil
}
