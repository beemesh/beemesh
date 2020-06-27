## Bee
BeeMesh comes as a single binary for datacenters, edge and mobile computing. Simply join and start deploy yours.

## Problem Statement
Kubernetes aggregates infrastructure as a single uniform computer. Unity is achieved by a cluster algorithm. The continuous allocation of the workload is done according to this set of rules and repeated as needed. Such uniform computers, called clusters, usually follow the rules of perimeter security in the data center.

Kubernetes follows a scale-out approach when it comes to increasing the resources available for the workload. The infrastructure used can also be requested on a larger scale for a more advantageous utilization rate.

Kubernetes can be extended with network virtualization in different forms. Several clusters are disjoint and therefore require further extensions such as ISTIO and/or cluster federation. Traditional services are excluded and must be considered separately.

The design decisions and ranking for a) clustering and b) connectivity, although individually exemplary and modularly implemented, lead to limitations in terms of scaling and connectivity.

BeeMesh prioritises connectivity and dissolves the cluster in its present form. This favours any software for long-lasting processing and functions compatible to today's service and data-centric security concepts. Removing the infrastructure clustering eliminates the scaling limits. This favours the whole mesh life cycle and reduces the administrative efforts.


## Architecture
![BeeMesh Binary](https://www.beemesh.io/assets/img/prototype.png)

The underlyings naturally prefer participiants beeing alive for longer over newer entrants. Ranking the peer to peer service mesh over clustering removes the pile up complexity. BeeMesh is designed for massive scale-out in mind. 

Clustering is required solely by stateful workload. As such, the solution context shrinks and becomes disposable.

The whole architecture encourages stateless zero trust based microservices.


## API
A Kubernetes compliant API is encouraged so that workloads can be shifted smoothly.


## Building Blocks
* P2P: [https://libp2p.io/](https://libp2p.io/)
* Workload Clustering: [https://github.com/libp2p/go-libp2p-raft](https://github.com/libp2p/go-libp2p-raft)
* Lightweight Kubernetes: [https://k3s.io/](https://k3s.io/)
* Podman: https://github.com/containers/libpod
* Example P2P Database: https://github.com/orbitdb
