# BeeMesh
[![Coverage Status](https://coveralls.io/repos/github/beemesh/beemesh/badge.svg?branch=master)](https://coveralls.io/github/beemesh/beemesh?branch=master)

## Vision
Imagine a world where everyone understands everyone else. Work would be continuously organized based on ability, capacity, and trustworthiness. Additionally it would be loosely distributed to a reasonable extent to meet the actual demand. When statefulness is required, ephemeral consensus just lives alongside the work. Would such a computational model scale globally?

## Problem Statement
When thinking about scalability in distributed systems, the first thing that comes to mind is consensus algorithms. Well, this is not a necessary requirement. Scalability for stateless workloads can be achieved without consensus and is limited only by available resources. Therefore, consensus is only required for stateful workloads. Consensus requires an underlying protocol that can uniquely identify all participants and counter partitioning. Thus, to have a robust and consistent consensus, we first need robust messaging.

So what is the underlying messaging used in Kubernetes? Actually, none. The reality is that trustworthiness is predefined manually and Kubernetes doesn't care about messaging. Instead, Kubernetes implements a monolithic consensus that overall manages cluster state, regardless of whether the workload requires it. If consensus is compromised, then all work is also affected.

Kubernetes networking is extensible by additional virtualization. Several clusters are disjoint and therefore require further extensions such as service meshes with replicated control planes or cluster federation as needed. Traditional services are excluded and must be considered separately. Despite the overall intention, this infrastructure-focused connectivity does not meet today's requirements for a fine-grained, resilient software design as an underlying messaging for scalable consensus.

## Architecture
![BeeMesh Binary](assets/prototype.png)

BeeMesh prioritises connectivity and dissolves clustering in its present form. Removing the infrastructure clustering eliminates the scaling limits. This favours today's security concepts, life cycle and reduces the administrative efforts.

A Kademlia based DHT peer to peer mesh has been alive since 2005. Measurements from the year 2013 show a volume of 10 to 25 million subscribers with a daily volatility of around 10 million. The peer to peer mesh naturally prefers participiants beeing alive for longer over newer entrants. 

Priorisation the peer to peer mesh over infrastructure state management enables a massive scale-out of stateless based long-lasting processing and functions. Stateless workload is notified by a [publish/subscribe](https://docs.libp2p.io/concepts/publish-subscribe/) pattern where peers congregate around topics they are interested in. State management is solely required by stateful workload. As such, the problem context shrinks to a transient state machine exactly matching the workload lifecycle.

## Policies
Peer to peer mesh policies allows you to make long-lasting processing or functions act as a resilient system through controlling how they communicate with each other as well as with external services. As such, a [general-purpose policy engine](https://www.openpolicyagent.org) that enables unified, context-aware policy enforcement across the entire mesh should be taken in to consideration.

## API
A Kubernetes compliant API is encouraged so that workloads can be shifted smoothly.
