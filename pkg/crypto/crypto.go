package crypto

import (
	"crypto/rand"
	"crypto/sha256"
	"fmt"
)

func GenerateID() (string, error) {
	b := make([]byte, 16)
	_, err := rand.Read(b)
	if err != nil {
		return "", fmt.Errorf("failed to generate ID: %v", err)
	}
	return fmt.Sprintf("%x", sha256.Sum256(b))[:16], nil
}
