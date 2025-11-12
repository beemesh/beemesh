---
mode: 'agent'
description: 'Check principle violations'
tools: ['changes', 'codebase', 'fetch', 'findTestFiles', 'githubRepo', 'problems', 'runCommands', 'runTasks', 'runTests', 'search', 'searchResults', 'testFailure', 'usages']
---

# 🔍 Check principle violations

## 🎯 Objective
Analyze the codebase for the machineplane and the custom prompts in .github/prompts and validate them with the principles:
* zero-trust, any message must be signed and client content such as workload manifests must be encrypted with post-quantum-safe algorithms
* all customer content must be stored in the DHT (like workload manifests)
* the system is multi-tenant, tenancy is based on workload pubkey, each user should only be able to see their own deployments
* all encryption logic must be within the crypto crate
* the system is fully decentralized with libp2p and must be self-healing
* there should be no single point of failures (e.g. key-shares must be shared with multiple peers) 

Find any inconsistencies or missing documentation or invalid / contradicting implementions.
Highlight discrepancies and suggest changes for the specific files.
Store the output as principle_violations.md

