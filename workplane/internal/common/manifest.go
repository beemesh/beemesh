package common

import (
	"fmt"
	"strings"

	"k8s.io/api/apps/v1"
	"sigs.k8s.io/yaml"
)

// ParseManifestName extracts the metadata.name from a Kubernetes manifest.
// The manifest may contain one or more YAML documents separated by `---`.
//
// The `kind` argument can be either:
//   - "StatelessWorkload"  -> matches Kubernetes kind "Deployment"
//   - "StatefulWorkload"   -> matches Kubernetes kind "StatefulSet"
//   - A direct Kubernetes kind string like "Deployment" or "StatefulSet"
//
// If multiple YAML documents are provided, the first matching document is used.
// Returns an error if no match is found or if metadata.name is missing.
func ParseManifestName(manifest []byte, kind string) (string, error) {
	if len(manifest) == 0 {
		return "", fmt.Errorf("manifest content is empty")
	}

	// Normalize workload kind to actual Kubernetes resource kind
	targetKind := normalizeKind(kind)

	// Split YAML into multiple docs
	rawDocs := splitYAMLDocuments(string(manifest))
	if len(rawDocs) == 0 {
		return "", fmt.Errorf("no valid YAML documents found in manifest")
	}

	for _, doc := range rawDocs {
		doc = strings.TrimSpace(doc)
		if doc == "" {
			continue
		}

		// Parse the top-level kind only
		var kindOnly struct {
			Kind string `yaml:"kind"`
		}
		if err := yaml.Unmarshal([]byte(doc), &kindOnly); err != nil {
			continue // skip malformed docs
		}

		switch strings.ToLower(kindOnly.Kind) {
		case "deployment":
			if targetKind == "Deployment" || targetKind == "" {
				var dep v1.Deployment
				if err := yaml.Unmarshal([]byte(doc), &dep); err != nil {
					continue
				}
				if dep.ObjectMeta.Name != "" {
					return dep.ObjectMeta.Name, nil
				}
				return "", fmt.Errorf("deployment manifest missing metadata.name")
			}
		case "statefulset":
			if targetKind == "StatefulSet" || targetKind == "" {
				var ss v1.StatefulSet
				if err := yaml.Unmarshal([]byte(doc), &ss); err != nil {
					continue
				}
				if ss.ObjectMeta.Name != "" {
					return ss.ObjectMeta.Name, nil
				}
				return "", fmt.Errorf("statefulset manifest missing metadata.name")
			}
		}
	}

	return "", fmt.Errorf("could not find metadata.name for kind %q in manifest", targetKind)
}

// normalizeKind maps internal workload kinds to Kubernetes kinds.
// Example: "StatelessWorkload" -> "Deployment"
func normalizeKind(kind string) string {
	switch strings.ToLower(strings.TrimSpace(kind)) {
	case "statelessworkload":
		return "Deployment"
	case "statefulworkload":
		return "StatefulSet"
	default:
		return kind
	}
}

// splitYAMLDocuments splits a YAML string into individual documents using `---` separators.
// Returns a slice of document strings.
func splitYAMLDocuments(input string) []string {
	// Kubernetes manifests often separate multiple objects using "---"
	// We split on that, trimming whitespace to avoid empty docs.
	var docs []string
	for _, part := range strings.Split(input, "---") {
		trimmed := strings.TrimSpace(part)
		if trimmed != "" {
			docs = append(docs, trimmed)
		}
	}
	return docs
}
