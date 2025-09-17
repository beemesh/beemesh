package workload

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"time"

	"github.com/libp2p/go-libp2p/core/host"
	"github.com/libp2p/go-libp2p/core/network"
	"github.com/libp2p/go-libp2p/core/peer"
)

const (
	// WorkloadProtocolID is the versioned protocol used for W↔W traffic.
	WorkloadProtocolID = "/beemesh/workload/comm/1.0.0"
	maxMessageSize     = 1 << 20 // 1 MiB
)

// RPCRequest / RPCResponse are simple JSON frames for demo purposes.
// Replace with protobuf for production use.
type RPCRequest struct {
	Method string                 `json:"method"`
	Body   map[string]interface{} `json:"body,omitempty"`
}

type RPCResponse struct {
	OK    bool                   `json:"ok"`
	Error string                 `json:"error,omitempty"`
	Body  map[string]interface{} `json:"body,omitempty"`
}

// RegisterStreamHandler wires the handler into the host with mutually-authenticated streams.
// NOTE: libp2p Security(TLS) already enforces mutual auth at the connection layer.
func RegisterStreamHandler(h host.Host, fn func(remote peer.ID, req RPCRequest) RPCResponse) {
	h.SetStreamHandler(WorkloadProtocolID, func(s network.Stream) {
		defer s.Close()

		_ = s.SetReadDeadline(time.Now().Add(10 * time.Second))
		data, err := io.ReadAll(io.LimitReader(s, maxMessageSize))
		if err != nil {
			return
		}
		var req RPCRequest
		if err := json.Unmarshal(data, &req); err != nil {
			_ = writeJSON(s, RPCResponse{OK: false, Error: "bad request"})
			return
		}

		resp := fn(s.Conn().RemotePeer(), req)
		_ = writeJSON(s, resp)
	})
}

func writeJSON(w io.Writer, v any) error {
	b, err := json.Marshal(v)
	if err != nil {
		return err
	}
	_, err = w.Write(b)
	return err
}

// SendRequest opens a stream to target and performs a request/response exchange.
func SendRequest(ctx context.Context, h host.Host, target peer.ID, req RPCRequest) (RPCResponse, error) {
	var zero RPCResponse

	s, err := h.NewStream(ctx, target, WorkloadProtocolID)
	if err != nil {
		return zero, fmt.Errorf("open stream: %w", err)
	}
	defer s.Close()

	_ = s.SetWriteDeadline(time.Now().Add(5 * time.Second))
	if err := writeJSON(s, req); err != nil {
		return zero, fmt.Errorf("write: %w", err)
	}
	_ = s.SetReadDeadline(time.Now().Add(10 * time.Second))
	data, err := io.ReadAll(io.LimitReader(s, maxMessageSize))
	if err != nil {
		return zero, fmt.Errorf("read: %w", err)
	}

	var resp RPCResponse
	if err := json.Unmarshal(data, &resp); err != nil {
		return zero, fmt.Errorf("decode: %w", err)
	}
	return resp, nil
}
