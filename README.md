## Bee
BeeMesh comes as a single binary for data centers, edge and mobile computing. Simply join and start deploy on your devices.

## Problem Statement
Kubernetes aggregates infrastructure as a single uniform computer. Unity is achieved by a cluster algorithm. The continuous allocation of workload is done according to this set of rules and repeated as needed. Such uniform computers aka clusters, usually follow the rules of perimeter security.

Kubernetes follows a scale-out approach when it comes to increase the available resources for workloads. The infrastructure can also be requested larger eg. scaled-up for a more advantageous utilization rate.

Kubernetes connectivity can be extended with basic network virtualization in different forms. Several clusters are disjoint and therefore require further extensions such as service meshes with replicated control planes or cluster federation as needed. Traditional services are excluded and must be considered separately. Despite the overall intention, this infrastructure-focused connectivity does not meet today’s requirements for a fine-grained, resilient software design.

The overall design decisions and ranking made for a) clustering and b) connectivity, although individually exemplary and modularly implemented, lead to limitations in terms of scaling and connectivity.

## Architecture
![BeeMesh Binary](https://www.beemesh.io/assets/img/prototype.png)

BeeMesh prioritises connectivity and dissolves clustering in its present form. Removing the infrastructure clustering eliminates the scaling limits. This favours today’s service and data-centric security concepts, life cycle and reduces the administrative efforts.

The underlyings naturally prefer participiants beeing alive for longer over newer entrants. Ranking the peer to peer mesh over clustering removes pile up complexity. BeeMesh is designed for massive scale-out and long-lasting processing and functions in mind.

Clustering is required solely by stateful workload. As such, the problem context shrinks and becomes disposable. The whole architecture encourages stateless zero trust based microservices.

## Policies
Peer to peer mesh policies allows you to make long-lasting processing or functions act as a resilient system through controlling how they communicate with each other as well as with external services.

## API
A Kubernetes compliant API is encouraged so that workloads can be shifted smoothly.

## Building Blocks
* Peer-to-peer Networking: [libp2p](https://libp2p.io/)
* Workload Clustering: [libp2p-raft](https://github.com/libp2p/go-libp2p-raft)
* Standalone pods: [Podman](https://github.com/containers/libpod)
* Lightweight Kubernetes: [k3s.io](https://k3s.io/)
* Example P2P Database: [OrbitDB](https://github.com/orbitdb)
