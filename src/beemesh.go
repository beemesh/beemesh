package main

import (
	"context"
	"crypto/rand"
	"flag"
	"fmt"
	"io"
	"net"
	"net/http"
	"sync"

	"github.com/ipfs/go-log"
	"github.com/libp2p/go-libp2p"
	"github.com/libp2p/go-libp2p-core/crypto"
	"github.com/libp2p/go-libp2p-core/host"
	"github.com/libp2p/go-libp2p-core/network"
	"github.com/libp2p/go-libp2p-core/peer"
	"github.com/libp2p/go-libp2p-core/protocol"
	discovery "github.com/libp2p/go-libp2p-discovery"
	dht "github.com/libp2p/go-libp2p-kad-dht"
	quic "github.com/libp2p/go-libp2p-quic-transport"
	routing "github.com/libp2p/go-libp2p-routing"
	multiaddr "github.com/multiformats/go-multiaddr"
)

var (
	err              error
	config           Config
	pod              host.Host
	ctx              context.Context
	logger           = log.Logger("beemesh")
	kdht             *dht.IpfsDHT
	routingDiscovery discovery.Discovery
	peers            chan peer.AddrInfo
)

func init() {

	peers = make(chan peer.AddrInfo, 7)
	ctx = context.Background()
	log.SetLogLevel("beemesh", "info")

	// cli flags and helper
	help := flag.Bool("help", false, "Display Help")
	config, err = ParseFlags()
	if err != nil {
		panic(err)
	}
	if *help {
		fmt.Println("BeeMesh")
		flag.PrintDefaults()
		return
	}

	// Creates a new RSA key pair for this host.
	prvKey, _, err := crypto.GenerateKeyPairWithReader(crypto.RSA, 2048, rand.Reader)
	if err != nil {
		panic(err)
	}

	// libp2p.New constructs a new libp2p host
	pod, err = libp2p.New(
		ctx,
		libp2p.DefaultTransports,
		libp2p.Transport(quic.NewTransport),
		libp2p.ListenAddrStrings("/ip4/0.0.0.0/tcp/0", "/ip4/0.0.0.0/udp/0/quic"),
		libp2p.ListenAddrStrings("/ip4/0.0.0.0/tcp/0"),
		libp2p.ListenAddrs([]multiaddr.Multiaddr(config.ListenAddresses)...),
		libp2p.NATPortMap(),
		libp2p.Routing(func(pod host.Host) (routing.PeerRouting, error) {
			kdht, err := dht.New(ctx, pod, dht.Mode(dht.ModeServer))
			logger.Debug("Bootstrap the DHT ...")
			if err = kdht.Bootstrap(ctx); err != nil {
				panic(err)
			}
			logger.Debug("Announce application ...")
			routingDiscovery = discovery.NewRoutingDiscovery(kdht)
			discovery.Advertise(ctx, routingDiscovery, config.AppID)
			return kdht, err
		}),
		libp2p.Identity(prvKey),
	)
	if err != nil {
		panic(err)
	}

	logger.Info("ID: ", pod.ID())
	logger.Info(pod.Addrs())

	// Connect to the bootstrap nodes
	logger.Debug("Connect boostrap nodes: ", config.BootstrapPeers)
	var bootwg sync.WaitGroup
	for _, peerAddr := range config.BootstrapPeers {
		peerinfo, _ := peer.AddrInfoFromP2pAddr(peerAddr)
		bootwg.Add(1)
		go func() {
			defer bootwg.Done()
			if err := pod.Connect(ctx, *peerinfo); err != nil {
				logger.Warn(err)
			} else {
				logger.Info("Connection established with bootstrap node:", *peerinfo)
			}
		}()
	}
	bootwg.Wait()

	// Init MDNS
	logger.Debug("Init MDNS ...")
	notifee := mdnsInit(ctx, pod, config.AppID)

	// Set stream handler for incoming peer streams
	pod.SetStreamHandler(protocol.ID(config.ProtocolID), streamHandler)

	go func() {
		for {
			peersChan, err := routingDiscovery.FindPeers(ctx, config.AppID)
			if err != nil {
				panic(err)
			}
			for addrInfo := range peersChan {
				if addrInfo.ID == pod.ID() {
					continue
				}
				go notifee.HandlePeerFound(addrInfo)
			}
		}
	}()
}

func main() {

	// Sample service
	go func() {
		http.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
			fmt.Fprintf(w, "Pod: %q", pod.ID())
			logger.Info("Serve ...")
		})
		logger.Infof("Server listener: http://%s", config.Server)
		err := http.ListenAndServe(config.Server, nil)
		if err != nil {
			panic(err)
		}
	}()

	// Proxy peer stream
	listener, err := net.Listen("tcp", config.Proxy)
	if err != nil {
		logger.Errorf("No proxy peer stream established: %s", err)
		return
	}
	defer listener.Close()
	logger.Infof("Proxy listener: http://%s", listener.Addr())
	for {
		conn, err := listener.Accept()
		if err != nil {
			logger.Errorf("Accept error: %s", err)
			return
		}
		if len(peers) > 0 {
			addrInfo := (<-peers)
			stream, err := pod.NewStream(ctx, addrInfo.ID, protocol.ID(config.ProtocolID))
			if err != nil {
				logger.Debug("Forward to peer failed: ", err)
			} else {
				logger.Debug("Forward to peer: ", addrInfo)
				go forward(stream, conn)
				peers <- addrInfo
			}
		} else {
			conn.Close()
		}
	}
	logger.Info("Shutting down ...")
	if err := pod.Close(); err != nil {
		panic(err)
	}
}

// Handle incoming peer stream
func streamHandler(stream network.Stream) {
	logger.Debug("Forward incomming peer stream to server ...")
	serverConn, err := net.Dial("tcp", config.Server)
	if err != nil {
		logger.Errorf("Error connecting to server: %v", err)
		if err := stream.Close(); err != nil {
			logger.Errorf("Cannot close stream: %v", err)
		}
		return
	}
	logger.Infof("Serve ...")
	forward(stream, serverConn)
}

// Forward data between stream and connection
func forward(stream network.Stream, conn net.Conn) {
	logger.Debug("Forward data between peer stream and connection ...")
	errc := make(chan error, 1)
	go func() {
		if _, err := io.Copy(conn, stream); err != nil {
			logger.Errorf("Error copying data from peer stream to connection: %v", err)
			errc <- err
		}
	}()
	go func() {
		if _, err := io.Copy(stream, conn); err != nil {
			logger.Errorf("Error copying data from connection to peer stream: %v", err)
			errc <- err
		}
	}()
	<-errc
}
