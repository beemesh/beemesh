# Research Appendix: Background & Related Work

## 1. No central control plane & decentralized scheduling

**What Beemesh does**

* No global control plane, no etcd / API server.
* Machineplane uses **ephemeral tenders + bids over pub/sub** instead of a single scheduler.
* Scheduling is **stateless and at‑least‑once** through a simple **tender → bid → award** exchange.

**Research grounding**

1. **Decentralized / distributed cluster schedulers**

   * **Sparrow** shows that *distributed, low‑latency* schedulers outperform centralized ones at scale by using randomized, stateless task placement (probe a few workers, not all).([People @ EECS][1])
   * **Hopper** extends this to *decentralized, speculation‑aware* scheduling for large analytics clusters, again moving away from a single global scheduler.([SIGCOMM Conferences][2])

   Beemesh’s “every node listens on `scheduler-tenders` and independently decides whether to bid” is very much in the same family: distributed decision‑making, no centralized queue, and low coordination overhead.

2. **Auction / market‑based scheduling**

   * Wellman et al. model **decentralized scheduling as an auction**, where jobs and machines exchange bids and prices to allocate resources efficiently.([ScienceDirect][3])
   * Systems like **Tycoon** and other market‑based cluster managers use bidding to achieve low‑latency, fair allocation of distributed resources.([arXiv][4])

   Beemesh’s “tender” (job announcement) and “bid with scoring metrics” aligns tightly with this body of work: nodes are autonomous agents making local decisions about whether and how to bid.

3. **Why remove the control plane at all?**

   * Kubernetes itself documents that production clusters are **practically limited to ~5,000 nodes, 150,000 pods** due to control‑plane scalability and etcd stress.([Kubernetes][5])
   * Large‑cluster analysis talks explicitly about *control‑plane SLOs* being the limiting factor and recommends federation for anything larger.([Sched][6])

   Beemesh’s premise — “scale is limited by the control plane, so remove it” — is an aggressive but logical extension of these findings.

---

## 2. Machine DHT & Workload DHT (decentralized discovery)

**What Beemesh does**

* Separate **Machine DHT** (node discovery) and **Workload DHT** (service discovery).
* No central registry; everything is **overlay‑based**.

**Research grounding**

1. **Distributed Hash Tables (DHTs)**

   * Chord, Kademlia and DHT surveys show that DHT overlays provide:

     * O(log N) routing,
     * graceful handling of churn,
     * and **decentralized key→node mapping** for millions of participants.([UCSB Computer Science][7])

   A Machine DHT that maps “machine IDs → capabilities” and a Workload DHT that maps “service IDs → peers/endpoints” fit exactly the canonical DHT role.

2. **DHT‑based pub/sub and multicast**

   * **Scribe** builds large‑scale multicast and group communication on top of Pastry (a DHT).([people.mpi-sws.org][8])
   * Triantafillou & Aekaterinidis build a **content‑based pub/sub system on Chord**, explicitly to avoid centralized brokers.([Mathematical Sciences Home Pages][9])
   * **Agele** and related systems use *decentralized broker overlays* to limit pub/sub traffic while keeping latency low.([dkl.cs.arizona.edu][10])

   Beemesh’s use of pub/sub topics (tenders, proposals, status) over a DHT‑based fabric is well aligned with this pattern of “DHT + overlay pub/sub” in the literature.

3. **Gossip‑based membership & failure detection**

   * SWIM and SCAMP provide **fully decentralized membership and failure detection** with constant per‑node message load and fast detection.([Cornell Bowers CS][11])
   * Van Renesse’s **gossip‑style failure detection service** shows that gossip can scale membership and failure detection to large clusters.([Cornell Bowers CS][12])

   While Beemesh’s docs use “DHT” instead of “gossip” terminology, the underlying goals (partial views, scalable membership, self‑organization) are identical to these protocols.

---

## 3. “Consumer‑scoped consistency” & state‑inside‑workloads

**What Beemesh does**

* Machines are **A/P** (availability + partition tolerance): they never store durable state.
* Each stateful workload **carries its own consensus** (e.g., its own Raft‑backed database).
* Replica counts are “**at least**” at the Machineplane; correctness is enforced inside the Workplane.

**Research grounding**

1. **CAP & AP/CP trade‑offs**

   * Gilbert & Lynch’s formal proof of Brewer’s CAP conjecture shows that in a partition you must trade off **Consistency vs Availability**.([CS Princeton][13])
   * In intermittently connected environments (mobility, IoT), systems often deliberately choose AP + eventual consistency to stay available.([Wikipedia][14])

   Beemesh’s separation is a direct CAP‑inspired design:

   * Machineplane = A/P
   * Workplane (per‑workload consensus) = C/P where needed.

2. **State with the application: Dynamo, Bayou, Coda**

   * **Dynamo**: each Amazon service runs its *own* Dynamo instance, prioritizing availability over strict consistency and using **application‑assisted conflict resolution**.([All Things Distributed][15])
   * **Bayou**: weakly consistent replicated storage for **weakly connected clients**, pushing conflict detection & resolution to the application level.([Department of Computer Science][16])
   * **Coda**: introduces **disconnected operation**, where clients keep local state and reconcile later, to survive network partitions.([People @ EECS][17])

   Beemesh generalizes this pattern: every stateful workload is its own Dynamo/Bayou/Coda‑style system; the fabric never pretends to maintain a globally consistent view.

3. **Per‑workload consensus (Raft / Paxos)**

   * **Raft** and **Paxos** are the standard consensus algorithms for replicated logs and strongly consistent services; modern databases embed these internally rather than relying on external control planes.([Raft][18])

   When Beemesh says “stateful workloads carry their own consensus,” it’s invoking this model: a PostgreSQL‑with‑Raft‑replication or similar system running inside the workload envelope, not in the infrastructure.

---

## 4. Security: separate machine & workload identities, zero trust

**What Beemesh does**

* Each **machine** has a unique cryptographic identity (Machine Peer ID).
* Each **workload** has its *own* cryptographic identity (Workload Peer ID).
* All comms (machine↔machine, workload↔workload) use **mutually authenticated, encrypted streams**.
* Machine and workload credentials never mix.

**Research grounding**

1. **Zero Trust principles**

   * NIST SP 800‑207 formalizes **Zero Trust Architecture**, moving from perimeter security to continuous identity‑centric verification of every user, device, and application.([NIST Publications][19])

   Beemesh’s “no shared credentials, distinct trust domains for infra vs workloads” is very close to textbook zero‑trust tenets.

2. **Workload identity (SPIFFE/SPIRE, service meshes)**

   * **SPIFFE/SPIRE** define **workload‑centric, cryptographically verifiable identities** (SPIFFE IDs, SVIDs) that are *decoupled from infrastructure* and used for mTLS between services.([SPIFFE][20])
   * Istio and other meshes use per‑workload certificates and mutual TLS for service‑to‑service auth, often backed by SPIFFE.([Istio][21])

   Beemesh essentially bakes a SPIFFE‑like idea into the fabric, but:

   * distinguishes **machine IDs** and **workload IDs**, and
   * makes mutually authenticated streams **mandatory**, not an optional add‑on.

3. **Why separating machine/workload identity matters**

   * Zero‑trust workload identity work (e.g. Red Hat’s zero‑trust workload operator, multi‑cloud SPIFFE deployments) stresses the importance of **not tying workload identity to host IP or VM identity**, especially in dynamic, multi‑cloud and ephemeral setups.([Red Hat Docs][22])

   This is exactly the principle behind Beemesh’s split Machineplane / Workplane trust domains.

---

## 5. Edge, IoT, fog: “everything is transient”

**What Beemesh does**

* Assumes **all machines are transient** and disposable.
* Targets edge, IoT, multicloud, air‑gapped environments.
* Machineplane is explicitly **stateless / rebuildable** as long as you have manifests or surviving workloads.

**Research grounding**

1. **Fog / edge computing characteristics**

   * Bonomi et al. introduce **Fog Computing** as extending the cloud to the edge, with:

     * low latency,
     * geographic distribution,
     * huge numbers of nodes,
     * mobility & wireless links.([SIGCOMM Conferences][23])
   * Surveys on fog/edge computing emphasize **heterogeneity, resource constraints, and frequent churn**, and call out the need for decentralized resource management and fault tolerance.([Pure Admin][24])

   Beemesh’s “fungible machines, state only in workloads, no central control plane” is almost a direct design response to these fog/edge constraints.

2. **Dependability & fault tolerance at the edge**

   * Work on fog dependability highlights challenges of failures, partitions, and placement strategies for highly distributed IoT nodes.([Science Gate][25])

   The **self‑healing Workplane** and disposable Machineplane are a concrete system design to address these dependability concerns.

---

## 6. Self‑healing & autonomic behavior

**What Beemesh does**

* Workplane continuously reconciles **desired replica counts** and respawns workloads.
* Replica counts are treated as “at least” targets with autonomous remediation when they drop below the floor.
* Machineplane is designed for **rebuild > repair**, with minimal human intervention.

**Research grounding**

1. **Autonomic computing & self‑healing**

   * Kephart & Chess’s **“The Vision of Autonomic Computing”** defines self‑configuration, self‑optimization, self‑protection, and **self‑healing** as core goals of future systems.([ACM Digital Library][26])
   * Surveys of autonomic computing systems and self‑healing systems identify:

     * runtime monitoring,
     * local fault detection,
     * automated recovery,
     * and minimal operator involvement as key design principles.([SciSpace][27])

   Beemesh’s Workplane is effectively an autonomic controller *inside each workload*, rather than one big control brain for the cluster.

2. **Gossip & failure detection as self‑healing substrate**

   * SWIM, SCAMP, and gossip‑style failure detectors explicitly target **self‑healing membership** in the face of frequent node failures, using epidemic dissemination to converge cluster state.([Cornell Bowers CS][11])

   A Machineplane that can lose arbitrary nodes and re‑form from remaining participants or external bootstrap sources is squarely in this tradition.

---

## 7. How Beemesh differs / adds novelty

Putting it together:

* **Every piece** of Beemesh has strong backing in prior work:

  * DHTs and gossip for membership & discovery.
  * Decentralized / auction‑inspired scheduling.
  * CAP‑aware AP infra + per‑service CP state (Dynamo/Bayou/Coda).
  * Per‑workload cryptographic identity and mTLS (SPIFFE / service meshes).
  * Autonomic / self‑healing behaviors for fault tolerance.
  * Edge/fog design assumptions (heterogeneous, highly dynamic, partitioned).

* What appears **novel** (and thus research‑worthy) is the *combination*:

  1. **Two fully separate planes** (Machineplane A/P, Workplane C/P) with their own DHTs and trust domains.
  2. A **stateless tender → bid → award scheduling semantics**: the fabric is intentionally at‑least‑once, and correctness is guaranteed by workload‑level protocols and the awarded placements.
  3. Treating **all infrastructure as disposable**, including the “control” substrate itself, and pushing durability entirely to workloads and external bootstrap mechanisms.
  4. Making **mutually authenticated, encrypted streams** the default at *both* infra and workload layers rather than adding a mesh as a sidecar.

Those choices don’t contradict the literature; they combine its building blocks in a more extreme, mesh‑native way.

---

## 8. Ready‑to‑cite bibliography by theme

Here’s a compact list you can lift directly into a “Grounding in Research” / “Related Work” section, grouped by concept.

**Decentralized scheduling & resource markets**

* Ousterhout et al., *Sparrow: Distributed, Low Latency Scheduling*, SOSP 2013.([People @ EECS][1])
* Ren et al., *Hopper: Decentralized Speculation‑aware Cluster Scheduling at Scale*, SIGCOMM 2015.([SIGCOMM Conferences][2])
* Wellman et al., *Auction Protocols for Decentralized Scheduling*, Games and Economic Behavior, 2001.([ScienceDirect][3])
* Lai et al., *Tycoon: A Distributed Market-based Resource Allocation System*, 2004.([arXiv][4])

**DHTs, pub/sub overlays, and gossip**

* Stoica et al., *Chord: A Scalable Peer-to-Peer Lookup Service for Internet Applications*, SIGCOMM 2001.([UCSB Computer Science][7])
* Maymounkov & Mazieres, *Kademlia: A Peer-to-Peer Information System Based on the XOR Metric*, IPTPS 2002.([ResearchGate][28])
* *Distributed Hash Table* overview.([Wikipedia][29])
* Castro et al., *Scribe: A Large-scale and Decentralized Application-level Multicast Infrastructure*, JSAC 2002.([people.mpi-sws.org][8])
* Triantafillou & Aekaterinidis, *Content-based Publish–Subscribe over Structured P2P Networks*.([Mathematical Sciences Home Pages][9])
* Das et al., *SWIM: Scalable Weakly-consistent Infection-style Process Group Membership*, 2002.([Cornell Bowers CS][11])
* Ganesh et al., *SCAMP: Peer-to-Peer Lightweight Membership Service for Large-Scale Group Communication*.([people.maths.bris.ac.uk][30])
* Van Renesse et al., *A Gossip-Style Failure Detection Service*, Middleware 1998.([Cornell Bowers CS][12])

**Consistency, CAP, and per‑workload state**

* Gilbert & Lynch, *Brewer’s Conjecture and the Feasibility of Consistent, Available, Partition-tolerant Web Services*, SIGACT News 2002.([CS Princeton][13])
* DeCandia et al., *Dynamo: Amazon’s Highly Available Key-value Store*, SOSP 2007.([All Things Distributed][15])
* Terry et al., *Managing Update Conflicts in Bayou, a Weakly Connected Replicated Storage System*, USENIX 1995.([Department of Computer Science][16])
* Kistler & Satyanarayanan, *Disconnected Operation in the Coda File System*, TOCS / SOSP.([People @ EECS][17])
* Ongaro & Ousterhout, *In Search of an Understandable Consensus Algorithm (Raft)*, USENIX ATC 2014.([Raft][18])
* Lamport, *Paxos Made Simple*, 2001.([Leslie Lamport][31])

**Security, zero trust, and workload identity**

* NIST SP 800‑207, *Zero Trust Architecture*, 2020.([NIST Publications][19])
* SPIFFE website and docs: *Secure Production Identity Framework for Everyone*.([SPIFFE][20])
* SPIRE concepts: *SPIRE is a production-ready implementation of SPIFFE APIs for node/workload attestation and identity issuance*.([SPIFFE][32])
* Istio security model with mutual TLS and SPIFFE IDs.([Istio][21])

**Edge, IoT, fog computing**

* Bonomi et al., *Fog Computing and Its Role in the Internet of Things*, MCC Workshop 2012.([SIGCOMM Conferences][23])
* Anawar et al., *Fog Computing: An Overview of Big IoT Data Analytics*, Wireless Communications and Mobile Computing, 2018.([Wiley Online Library][33])
* Hong & Varghese, *Resource Management in Fog/Edge Computing: A Survey*, ACM CSUR 2019.([Pure Admin][24])
* Alraddady et al., *Dependability in Fog Computing: Challenges and Solutions*, 2021.([Science Gate][25])

**Self‑healing & autonomic systems**

* Kephart & Chess, *The Vision of Autonomic Computing*, IEEE Computer 2003.([ResearchGate][34])
* Huebscher & McCann, *A Survey of Autonomic Computing — Degrees, Models, and Applications*.([cs.colostate.edu][35])
* Psaier & Dustdar, *A Survey on Self-healing Systems: Approaches and Systems*.([ACM Digital Library][36])

---

[1]: https://people.eecs.berkeley.edu/~matei/papers/2013/sosp_sparrow.pdf?utm_source=chatgpt.com "Sparrow: Distributed, Low Latency Scheduling - People @EECS"
[2]: https://conferences.sigcomm.org/sigcomm/2015/pdf/papers/p379.pdf?utm_source=chatgpt.com "Decentralized Speculation-aware Cluster Scheduling at ..."
[3]: https://www.sciencedirect.com/science/article/pii/S0899825600908224?utm_source=chatgpt.com "Auction Protocols for Decentralized Scheduling"
[4]: https://arxiv.org/abs/cs/0404013?utm_source=chatgpt.com "Tycoon: A Distributed Market-based Resource Allocation System"
[5]: https://kubernetes.io/docs/setup/best-practices/cluster-large/?utm_source=chatgpt.com "Considerations for large clusters"
[6]: https://static.sched.com/hosted_files/kccna18/92/Kubernetes%20Scalability_%20A%20multi-dimensional%20analysis.pdf?utm_source=chatgpt.com "Kubernetes Scalability:"
[7]: https://sites.cs.ucsb.edu/~rich/class/cs293b-cloud/papers/chord.pdf?utm_source=chatgpt.com "Chord: A Scalable Peer-to-peer Lookup Service for Internet ..."
[8]: https://people.mpi-sws.org/~druschel/publications/Scribe-jsac.pdf?utm_source=chatgpt.com "Scribe: A large-scale and decentralized application-level ..."
[9]: https://homepage.divms.uiowa.edu/~ghosh/triantafillou.pdf?utm_source=chatgpt.com "Publish-Subscribe Over Structured P2P Networks"
[10]: https://dkl.cs.arizona.edu/publications/papers/debs09.pdf?utm_source=chatgpt.com "Towards Efficient Event Aggregation in a Decentralized ..."
[11]: https://www.cs.cornell.edu/projects/Quicksilver/public_pdfs/SWIM.pdf?utm_source=chatgpt.com "SWIM: Scalable Weakly-consistent Infection-style Process ..."
[12]: https://www.cs.cornell.edu/home/rvr/papers/GossipFD.pdf?utm_source=chatgpt.com "A Gossip-Style Failure Detection Service - CS@Cornell"
[13]: https://www.cs.princeton.edu/courses/archive/spr22/cos418/papers/cap.pdf?utm_source=chatgpt.com "Brewer's Conjecture and the Feasibility of Consistent, ..."
[14]: https://en.wikipedia.org/wiki/CAP_theorem?utm_source=chatgpt.com "CAP theorem"
[15]: https://www.allthingsdistributed.com/files/amazon-dynamo-sosp2007.pdf?utm_source=chatgpt.com "Dynamo: Amazon's Highly Available Key-value Store"
[16]: https://www.cs.utexas.edu/~lorenzo/corsi/cs380d/papers/p172-terry.pdf?utm_source=chatgpt.com "Managing Update Conflicts in Bayou, a Weakly Connected ..."
[17]: https://people.eecs.berkeley.edu/~brewer/cs262b/Coda-TOCS.pdf?utm_source=chatgpt.com "Disconnected Operation in the Coda File System"
[18]: https://raft.github.io/raft.pdf?utm_source=chatgpt.com "In Search of an Understandable Consensus Algorithm"
[19]: https://nvlpubs.nist.gov/nistpubs/specialpublications/NIST.SP.800-207.pdf?utm_source=chatgpt.com "Zero Trust Architecture - NIST Technical Series Publications"
[20]: https://spiffe.io/?utm_source=chatgpt.com "SPIFFE – Secure Production Identity Framework for Everyone"
[21]: https://istio.io/latest/docs/concepts/security/?utm_source=chatgpt.com "Security"
[22]: https://docs.redhat.com/en/documentation/openshift_container_platform/4.18/html/security_and_compliance/zero-trust-workload-identity-manager?utm_source=chatgpt.com "Chapter 10. Zero Trust Workload Identity Manager"
[23]: https://conferences.sigcomm.org/sigcomm/2012/paper/mcc/p13.pdf?utm_source=chatgpt.com "Fog Computing and Its Role in the Internet of Things"
[24]: https://pureadmin.qub.ac.uk/ws/files/168704756/Fog_EdgeResourceManagement_Survey_CSUR_2019.pdf?utm_source=chatgpt.com "Resource Management in Fog/Edge Computing: A Survey ..."
[25]: https://www.science-gate.com/IJAAS/Articles/2021/2021-8-4/1021833ijaas202104010.pdf?utm_source=chatgpt.com "Dependability in fog computing: Challenges and solutions"
[26]: https://dl.acm.org/doi/10.1109/MC.2003.1160055?utm_source=chatgpt.com "The Vision of Autonomic Computing"
[27]: https://scispace.com/pdf/a-survey-of-autonomic-computing-systems-6oj7wluwfa.pdf?utm_source=chatgpt.com "A Survey of Autonomic Computing Systems"
[28]: https://www.researchgate.net/publication/2492563_Kademlia_A_Peer-to-peer_Information_System_Based_on_the_XOR_Metric?utm_source=chatgpt.com "A Peer-to-peer Information System Based on the XOR Metric"
[29]: https://en.wikipedia.org/wiki/Distributed_hash_table?utm_source=chatgpt.com "Distributed hash table"
[30]: https://people.maths.bris.ac.uk/~maajg/scamp-ngc.pdf?utm_source=chatgpt.com "SCAMP: Peer-to-peer lightweight membership service for ..."
[31]: https://lamport.azurewebsites.net/pubs/paxos-simple.pdf?utm_source=chatgpt.com "Paxos Made Simple - Leslie Lamport"
[32]: https://spiffe.io/docs/latest/spire-about/spire-concepts/?utm_source=chatgpt.com "SPIRE Concepts"
[33]: https://onlinelibrary.wiley.com/doi/10.1155/2018/7157192?utm_source=chatgpt.com "Fog Computing: An Overview of Big IoT Data Analytics"
[34]: https://www.researchgate.net/publication/2955831_The_Vision_Of_Autonomic_Computing?utm_source=chatgpt.com "(PDF) The Vision Of Autonomic Computing"
[35]: https://www.cs.colostate.edu/~france/CS614/Readings/Readings2008/AdaptiveSoftware/a7-huebscher-autonomicSysSurvey.pdf?utm_source=chatgpt.com "7 A survey of Autonomic Computing—Degrees, Models, ..."
[36]: https://dl.acm.org/doi/abs/10.1007/s00607-010-0107-y?utm_source=chatgpt.com "A survey on self-healing systems: approaches and systems"
