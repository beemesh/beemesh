---
mode: 'agent'
description: 'Use agent to implement workload applying'
tools: ['codebase', 'fetch', 'findTestFiles', 'githubRepo', 'problems', 'search', 'searchResults', 'usages', 'github']
---

# Overall Goal
The goal is to verify adherence and find missing or wrong implementation parts when scheduling workloads (beectl apply -f *). Mention any shortcomings of the description or where more implementation details must be specified. This workflow schedules workloads similar to k8s kube apply -f.

# Instructions
You are an expert software engineer tasked with planning changes to the codebase. You will be provided with a description of the changes needed, and you will use the tools available to you to gather information about the codebase, identify relevant files, and plan the changes.

# High-Level Scheduling workflow
1. Use runs beectl apply -f kube-manifest.yml
2. beectl / cli encrypts the manifest with a post-quantum-safe symmetric key with 3 shares and 2 required.
3. beectl requests from the machine plane through the restapi resources and the machine plane stores the encrypted manifest in the DHT
4. machine plane sends a gossipsub message to participating nodes and asks for a specified wait time for answers.
5. machine plane picks top proposals and forward and returns the peer ids and their signatures (also pqs) to the cli
6. the cli encrypts the key share asimmetrically for every node
7. the cli sends the reference to the encrypted manifest in the DHT incl. the encrypted key shares to the host api which then distributes the key shares to the respective peer node
8. The peer nodes form a new gossipsub topic for the workload id and the nodes that actually run the manifest (e.g. number of replicas) asks the the other nodes for the key share to decrypt the manifest by offering a capability token that was provided by the cli.
9. The machine schedules the workload through podman kube play -f *.