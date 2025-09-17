package common

import (
	"encoding/base64"
	"fmt"

	libp2pcrypto "github.com/libp2p/go-libp2p/core/crypto"
	peer "github.com/libp2p/go-libp2p/core/peer"
)

// GeneratePrivateKey generates a new libp2p private key using the Ed25519 algorithm.
func GeneratePrivateKey() (libp2pcrypto.PrivKey, error) {
	priv, _, err := libp2pcrypto.GenerateEd25519Key(nil)
	if err != nil {
		return nil, fmt.Errorf("failed to generate private key: %w", err)
	}
	return priv, nil
}

// MarshalPrivateKey converts a libp2p private key into a byte slice for storage.
func MarshalPrivateKey(priv libp2pcrypto.PrivKey) ([]byte, error) {
	if priv == nil {
		return nil, fmt.Errorf("private key cannot be nil")
	}
	data, err := libp2pcrypto.MarshalPrivateKey(priv)
	if err != nil {
		return nil, fmt.Errorf("failed to marshal private key: %w", err)
	}
	return data, nil
}

// UnmarshalPrivateKey converts a raw byte slice into a libp2p private key.
func UnmarshalPrivateKey(data []byte) (libp2pcrypto.PrivKey, error) {
	if len(data) == 0 {
		return nil, fmt.Errorf("private key data is empty")
	}
	priv, err := libp2pcrypto.UnmarshalPrivateKey(data)
	if err != nil {
		return nil, fmt.Errorf("failed to unmarshal private key: %w", err)
	}
	return priv, nil
}

// EncodePrivateKeyBase64 encodes a private key as a base64 string.
// Useful for storing keys in environment variables or Kubernetes secrets.
func EncodePrivateKeyBase64(priv libp2pcrypto.PrivKey) (string, error) {
	data, err := MarshalPrivateKey(priv)
	if err != nil {
		return "", err
	}
	return base64.StdEncoding.EncodeToString(data), nil
}

// DecodePrivateKeyBase64 decodes a base64 string into a libp2p private key.
func DecodePrivateKeyBase64(b64 string) (libp2pcrypto.PrivKey, error) {
	data, err := base64.StdEncoding.DecodeString(b64)
	if err != nil {
		return nil, fmt.Errorf("failed to decode base64 private key: %w", err)
	}
	return UnmarshalPrivateKey(data)
}

// PeerIDFromPrivateKey derives the peer ID from a given private key.
func PeerIDFromPrivateKey(priv libp2pcrypto.PrivKey) (peer.ID, error) {
	if priv == nil {
		return "", fmt.Errorf("private key cannot be nil")
	}
	id, err := peer.IDFromPrivateKey(priv)
	if err != nil {
		return "", fmt.Errorf("failed to derive peer ID: %w", err)
	}
	return id, nil
}

// PublicKeyBytes extracts the raw byte slice from a public key.
func PublicKeyBytes(pub libp2pcrypto.PubKey) ([]byte, error) {
	if pub == nil {
		return nil, fmt.Errorf("public key cannot be nil")
	}
	data, err := libp2pcrypto.MarshalPublicKey(pub)
	if err != nil {
		return nil, fmt.Errorf("failed to marshal public key: %w", err)
	}
	return data, nil
}

// PublicKeyFromBytes reconstructs a libp2p public key from its raw bytes.
func PublicKeyFromBytes(data []byte) (libp2pcrypto.PubKey, error) {
	if len(data) == 0 {
		return nil, fmt.Errorf("public key data is empty")
	}
	pub, err := libp2pcrypto.UnmarshalPublicKey(data)
	if err != nil {
		return nil, fmt.Errorf("failed to unmarshal public key: %w", err)
	}
	return pub, nil
}

// PublicKeyBase64 encodes a public key to a base64 string.
func PublicKeyBase64(pub libp2pcrypto.PubKey) (string, error) {
	data, err := PublicKeyBytes(pub)
	if err != nil {
		return "", err
	}
	return base64.StdEncoding.EncodeToString(data), nil
}

// PublicKeyFromBase64 decodes a base64-encoded public key.
func PublicKeyFromBase64(b64 string) (libp2pcrypto.PubKey, error) {
	data, err := base64.StdEncoding.DecodeString(b64)
	if err != nil {
		return nil, fmt.Errorf("failed to decode base64 public key: %w", err)
	}
	return PublicKeyFromBytes(data)
}
