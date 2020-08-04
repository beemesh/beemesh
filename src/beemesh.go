package main

import (
	"context"
	"flag"
	"fmt"
	"io"
	"net"
	"sync"

	"github.com/ipfs/go-log"
	"github.com/libp2p/go-libp2p"
	"github.com/libp2p/go-libp2p-core/host"
	"github.com/libp2p/go-libp2p-core/network"
	"github.com/libp2p/go-libp2p-core/peer"
	"github.com/libp2p/go-libp2p-core/protocol"
	discovery "github.com/libp2p/go-libp2p-discovery"
	dht "github.com/libp2p/go-libp2p-kad-dht"
	multiaddr "github.com/multiformats/go-multiaddr"
)

var (
	err              error
	config           Config
	pod              host.Host
	routingDiscovery discovery.Discovery
	logger           = log.Logger("beemesh")
	ctx              = context.Background()
)

func init() {
	log.SetLogLevel("beemesh", "info")
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

	// libp2p.New constructs a new libp2p host
	pod, err = libp2p.New(ctx,
		libp2p.ListenAddrs([]multiaddr.Multiaddr(config.ListenAddresses)...),
		libp2p.NATPortMap(),
	)
	if err != nil {
		panic(err)
	}
	logger.Info("ID: ", pod.ID())
	logger.Info(pod.Addrs())

	// Set stream handler for incoming peer streams
	pod.SetStreamHandler(protocol.ID(config.ProtocolID), streamHandler)

	// Start DHT for peer discovery
	logger.Debug("Start the DHT")
	kdht, err := dht.New(ctx, pod, dht.Mode(dht.ModeServer))
	if err != nil {
		panic(err)
	}

	// Spawn background thread to refresh the peer table every five minutes
	logger.Debug("Bootstrapping the DHT")
	if err = kdht.Bootstrap(ctx); err != nil {
		panic(err)
	}

	// Connect to the bootstrap nodes
	logger.Debug("Connect boostrap nodes: ", config.BootstrapPeers)
	var wg sync.WaitGroup
	for _, peerAddr := range config.BootstrapPeers {
		peerinfo, _ := peer.AddrInfoFromP2pAddr(peerAddr)
		wg.Add(1)
		go func() {
			defer wg.Done()
			if err := pod.Connect(ctx, *peerinfo); err != nil {
				logger.Warn(err)
			} else {
				logger.Info("Connection established with bootstrap node:", *peerinfo)
			}
		}()
	}
	wg.Wait()

	// Announce application
	logger.Debug("Announce application ...")
	routingDiscovery = discovery.NewRoutingDiscovery(kdht)
	discovery.Advertise(ctx, routingDiscovery, config.AppID)
}

func main() {

	streams := make(chan network.Stream)

	go func(chan<- network.Stream) {
		for {
			logger.Debug("Find peers ...")
			peerChan, err := routingDiscovery.FindPeers(ctx, config.AppID)
			if err != nil {
				panic(err)
			}
			for peer := range peerChan {
				if peer.ID == pod.ID() {
					continue
				}
				logger.Debug("Found peer: ", peer)
				stream, err := pod.NewStream(ctx, peer.ID, protocol.ID(config.ProtocolID))
				if err != nil {
					logger.Debug("Peer failed: ", err)
					continue
				} else {
					logger.Debug("Peer connected: ", peer)
					streams <- stream
				}
			}
		}
	}(streams)

	// Proxy server connection
	go func() {
		listener, err := net.Listen("tcp", config.Proxy)
		if err != nil {
			logger.Errorf("No proxy connection established: %s", err)
			return
		}
		logger.Infof("Listening for proxy server connection: http://%s", listener.Addr())
		for {
			conn, err := listener.Accept()
			if err != nil {
				logger.Errorf("Accept error: %s", err)
				return
			}
			go connHandler(conn)
		}
	}()

	// Proxy peer stream
	func() {
		listener, err := net.Listen("tcp", "127.0.0.1:0")
		if err != nil {
			logger.Errorf("No proxy peer stream established: %s", err)
			return
		}
		logger.Infof("Listening for proxy peer stream: http://%s", listener.Addr())
		for {
			conn, err := listener.Accept()
			if err != nil {
				logger.Errorf("Accept error: %s", err)
				return
			}
			go forward(<-streams, conn)
		}
	}()
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
	forward(stream, serverConn)
}

// Forward data between stream and connection
func forward(stream network.Stream, conn net.Conn) {
	logger.Debug("Forward data between peer stream and connection ...")
	logger.Debugf("Forward stream to peer ID: %s", stream.Conn().RemotePeer())
	go func() {
		if _, err := io.Copy(conn, stream); err != nil {
			logger.Errorf("Error copying data from peer stream to connection: %v", err)
		}
	}()
	go func() {
		if _, err := io.Copy(stream, conn); err != nil {
			logger.Errorf("Error copying data from connection to peer stream: %v", err)
		}
	}()
}

// Handle incoming proxy connection
func connHandler(proxyConn net.Conn) {
	logger.Debug("Proxy incomming connection to server ...")
	serverConn, err := net.Dial("tcp", config.Server)
	if err != nil {
		logger.Errorf("Error connecting to server: %v", err)
		if err := serverConn.Close(); err != nil {
			logger.Errorf("Cannot close connection: %v", err)
		}
		return
	}
	go func() {
		if _, err := io.Copy(serverConn, proxyConn); err != nil {
			logger.Errorf("Error copying data from proxy connection to server: %v", err)
		}
	}()
	go func() {
		if _, err := io.Copy(proxyConn, serverConn); err != nil {
			logger.Errorf("Error copying data from server connection to proxy: %v", err)
		}
	}()
}
