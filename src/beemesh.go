package main

import (
	"context"
	"crypto/rand"
	"flag"
	"fmt"
	"io"
	"net"
	"net/http"
	"time"

	"github.com/ipfs/go-log"
	"github.com/libp2p/go-libp2p"
	"github.com/libp2p/go-libp2p-core/crypto"
	"github.com/libp2p/go-libp2p-core/host"
	"github.com/libp2p/go-libp2p-core/network"
	"github.com/libp2p/go-libp2p-core/peer"
	"github.com/libp2p/go-libp2p-core/protocol"
	discovery "github.com/libp2p/go-libp2p-discovery"
	dht "github.com/libp2p/go-libp2p-kad-dht"
	pubsub "github.com/libp2p/go-libp2p-pubsub"
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
	routingDiscovery discovery.Discovery
	peerChan         chan peer.ID
	//peers            sync.Map
)

func init() {

	ctx = context.Background()
	log.SetLogLevel("beemesh", "info")
	peerChan = make(chan peer.ID, 7)

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
		libp2p.ListenAddrStrings("/ip4/0.0.0.0/tcp/0", "/ip4/0.0.0.0/udp/0/quic", "/ip4/0.0.0.0/tcp/0/ws"),
		libp2p.ListenAddrs([]multiaddr.Multiaddr(config.ListenAddresses)...),
		libp2p.Routing(func(pod host.Host) (routing.PeerRouting, error) {
			kdht, err := dht.New(ctx, pod, dht.Mode(dht.ModeServer))
			// Bootstrap dht
			logger.Debug("Bootstrap the DHT ...")
			if err = kdht.Bootstrap(ctx); err != nil {
				panic(err)
			}
			// Announce application
			logger.Debug("Announce application as dht namespace ...")
			routingDiscovery = discovery.NewRoutingDiscovery(kdht)
			routingDiscovery.Advertise(ctx, config.AppID)
			return kdht, err
		}),
		libp2p.NATPortMap(),
		//libp2p.EnableNATService(),
		//libp2p.EnableAutoRelay(),
		libp2p.Identity(prvKey),
	)
	if err != nil {
		panic(err)
	}
	logger.Info("ID: ", pod.ID())
	logger.Info(pod.Addrs())

	// Set stream handler for incoming peer streams
	pod.SetStreamHandler(protocol.ID(config.ProtocolID), streamHandler)

	// Init MDNS
	logger.Debug("Init MDNS ...")
	notifee := mdnsInit(ctx, pod, config.AppID)

	// Subscribe the topic
	gossipSub, err := pubsub.NewGossipSub(ctx, pod)
	if err != nil {
		panic(err)
	}
	topic, err := gossipSub.Join(config.AppID)
	if err != nil {
		panic(err)
	}
	sub, err := topic.Subscribe()
	if err != nil {
		panic(err)
	}
	logger.Info("Topic subscribed: ", sub.Topic())

	// Connect to the bootstrap nodes
	logger.Debug("Connect boostrap nodes: ", config.BootstrapPeers)
	for _, multiaddr := range config.BootstrapPeers {
		if addrInfo, err := peer.AddrInfoFromP2pAddr(multiaddr); err == nil {
			go notifee.HandlePeerFound(*addrInfo)
		} else {
			logger.Panic(err)
		}
	}
	peersChan, err := routingDiscovery.FindPeers(ctx, config.AppID)
	if err != nil {
		panic(err)
	}
	for addrInfo := range peersChan {
		go notifee.HandlePeerFound(addrInfo)
	}

	go func() {
		for {
			for _, peerID := range gossipSub.ListPeers(sub.Topic()) {
				if pod.Network().Connectedness(peerID) != network.Connected {
					logger.Warn("Disconnected: ", peerID)
				} else {
					peerChan <- peerID
					logger.Debug("Connected: ", peerID)
				}
			}
			//pod.Network().ClosePeer()
			logger.Info("Peers: ", len(pod.Network().Peers()), len(peerChan))
			time.Sleep(time.Second * 30)
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
		peerID := (<-peerChan)
		if len(peerChan) > 0 {
			stream, err := pod.NewStream(ctx, peerID, protocol.ID(config.ProtocolID))
			if err != nil {
				logger.Debug("Forward to peer failed: ", err)
			} else {
				go forward(stream, conn)
			}
		} else {
			conn.Close()
		}
		peerChan <- peerID
	}
	logger.Info("Shutting down ...")
	if err := pod.Close(); err != nil {
		panic(err)
	}
}

// Handle incoming peer stream
func streamHandler(stream network.Stream) {
	logger.Debug("Forward incomming stream ...")
	serverConn, err := net.Dial("tcp", config.Server)
	if err != nil {
		logger.Errorf("Error connecting to server: %v", err)
		if err := stream.Close(); err != nil {
			logger.Errorf("Cannot close stream: %v", err)
		}
		return
	}
	forward(stream, serverConn)
}

// Forward data between stream and connection
func forward(stream network.Stream, conn net.Conn) {
	logger.Debug("Forward data ...")
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

/*

func lenSyncMap(syncMap sync.Map) int {
	len := 0
	syncMap.Range(func(key, value interface{}) bool {
		len++
		return true
	})
	return len
}

func randomKey(syncMap sync.Map) peer.ID {
	var peerID peer.ID
	syncMap.Range(func(key, value interface{}) bool {
		peerID = key.(peer.ID)
		return false
	})
	return peerID
}
*/
