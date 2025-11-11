# **Beemesh: Global Mesh Computing**

> **Beemesh** is scale-out fabric orchestration that turns any device - cloud, on-prem, edge, or IoT - into an interchangeable compute resource.
> It scales out by eliminating the centralized control plane, enabling **secure, self-healing workloads** across highly dynamic environments.

---

## **1. Introduction**

Modern orchestrators like Kubernetes and Nomad are powerful but **inherently limited by centralization**. As clusters grow:

* Control planes become **scaling bottlenecks**.
* Consensus overhead increases.
* Infrastructure failures ripple through workloads.

Beemesh rethinks orchestration from the ground up:

* **No central control plane** - fully scale-out fabric coordination.
* **Workload-scoped consistency** - each application carries its own state management.
* **Separate security and discovery planes** - machines and workloads each operate on their own DHT with independent trust domains.
* **Scale out** - limited only by network capacity.

---

## **2. Why Beemesh**

| Problem in Legacy Systems                                                | Beemesh Solution                                                                   |
| ------------------------------------------------------------------------ | ---------------------------------------------------------------------------------- |
| Scaling beyond 5,000–10,000 nodes is hard due to control plane overhead. | **Fully decentralized coordination** using DHT + Pub/Sub.                          |
| Machine failures destabilize cluster state.                              | Machines are **fungible, disposable resources**; state lives **inside workloads**. |
| High operational complexity (etcd, API servers, Raft quorums).           | **Single lightweight daemon** (\~50–80 MB RAM).                                    |
| Weak identity and trust at scale.                                        | **Separate Peer IDs for machines and workloads**, mutually authenticated streams.  |
| Vendor lock-in to specific clouds or infra.                              | **Infrastructure-agnostic**, runs on anything.                                     |
| Day-2 toil for upgrades/patching of control planes.                      | **Decoupled, autonomous Machine Plane** → **rebuild > repair**, near-zero toil.    |

---

## **3. Core Concepts**

### **3.1 Two-Plane Architecture**

Beemesh divides orchestration into **two planes**:

| Plane             | Purpose                                                               | CAP Trade-off                                |
| ----------------- | --------------------------------------------------------------------- | -------------------------------------------- |
| **Machine Plane** | Node-level coordination, scheduling, and resource management.         | **A/P** – Availability & Partition Tolerance |
| **Work Plane**    | Application-level self-healing, service discovery, secure networking. | **C/P** – Consistency & Partition Tolerance  |

This separation eliminates cross-cutting concerns and allows **each plane to optimize independently**. Scoping each plane to its domain prevents the monolithic anti-patterns that control-plane–centric platforms preserve.

---

### **3.2 Separate DHTs**

Beemesh uses **two distinct Distributed Hash Tables (DHTs):**

| DHT              | Purpose                                                                                                                                        |
| ---------------- | ---------------------------------------------------------------------------------------------------------------------------------------------- |
| **Machine DHT**  | Node discovery and coordination; ephemeral task negotiations; stores only minimal, transient machine metadata.                                 |
| **Workload DHT** | Service discovery between workloads; maintains workload-level peer state and connectivity info; **no machine-level data** is ever stored here. |

> **Key benefit:** Machine failures do **not** pollute the workload discovery layer. Each DHT is optimized for its purpose, ensuring scalability and reliability.

---

### **3.3 Workload-Scoped Consistency**

Traditional systems bind **consistency to infrastructure**:

* Kubernetes uses `etcd` for cluster state.
* Nomad/Consul use Raft quorums for global consensus.

**Beemesh flips the model:**

* **Infrastructure is stateless** and ephemeral.
* Each **stateful workload carries its own consensus cluster** (e.g., a Raft group for a database).

> Infra failures never corrupt workload state and enable scale out to **tens of thousands of nodes**.

---

## **4. Security and Identity Model**

### **4.1 Unique Peer IDs**

Beemesh assigns **separate cryptographic identities** to machines and workloads:

| Entity       | Identity Details                                                                                                                                                                      |
| ------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Machine**  | Each machine has a **unique `libp2p` Peer ID** generated at first startup; backed by a public/private key pair; used for node discovery, scheduling, and Machine-Plane communication. |
| **Workload** | Each workload Pod has a **unique `libp2p` Peer ID**; backed by a separate public/private key pair; used only for service discovery and secure workload-to-workload traffic.           |

This separation ensures **complete isolation of trust boundaries** between infrastructure and workloads.

---

### **4.2 Secure Communication**

All communication in Beemesh is **mutually authenticated** and **end-to-end encrypted** using `libp2p` secure streams.

| Communication Type      | Authentication Domain            | Encryption                                     |
| ----------------------- | -------------------------------- | ---------------------------------------------- |
| **Machine ↔ Machine**   | Verified using Machine Peer IDs  | Encrypted via `libp2p.Security` (TLS or Noise) |
| **Workload ↔ Workload** | Verified using Workload Peer IDs | Encrypted via `libp2p.Security` (TLS or Noise) |

* Machine and workload communication **never share credentials**.
* Even if a machine is compromised, workloads remain isolated and protected.

---

## **5. How Beemesh Compares**

| System      | Control Plane                   | Scaling Limit                | State Model                         | Notes                                            |
| ----------- | ------------------------------- | ---------------------------- | ----------------------------------- | ------------------------------------------------ |
| Kubernetes  | Centralized (etcd + API server) | \~5,000 nodes / 150,000 pods | Strong consistency cluster-wide     | Rich ecosystem but control-plane limited         |
| Nomad       | Centralized Raft quorum         | Thousands of nodes           | Strong consistency global scheduler | Lighter than K8s but still infra-bound           |
| Swarm       | Raft manager nodes              | \~1,000 nodes                | Strong consistency cluster-wide     | Simple but infra-coupled                         |
| **Beemesh** | **None – scale-out fabric**     | **Tens of thousands+**       | **Consistency scoped to workload**  | Scale out; only stateful workloads run Raft |

---

## **6. Architecture**

### **6.1 High-Level Diagram**

```mermaid
flowchart TB
  %% ===== Work Plane (per-workload trust domain) =====
  subgraph "Work Plane (workload trust domain)" as WP
    WDHT[(Workload DHT - Service Discovery)]
    W1[Workload W1]
    W2[Workload W2]

    W1 -->|mTLS| W2
    W2 -->|mTLS| W1

    W1 -->|register/query| WDHT
    W2 -->|register/query| WDHT
  end

  %% ===== Machine Plane (infrastructure trust domain) =====
  subgraph "Machine Plane (infrastructure trust domain)" as MP
    MDHT[(Machine DHT - Node Discovery)]
    PS{{Pub/Sub - scheduler topics}}

    subgraph "Machine A" as M1
      D1[Machine Daemon A]
      R1[(Runtime A - Podman)]
    end

    subgraph "Machine B" as M2
      D2[Machine Daemon B]
      R2[(Runtime B - Podman)]
    end

    D1 -->|secure stream| D2
    D2 -->|secure stream| D1

    D1 -->|announce/lookup| MDHT
    D2 -->|announce/lookup| MDHT

    D1 -->|publish/subscribe| PS
    D2 -->|publish/subscribe| PS
  end

  D1 -->|deploy/start| R1
  D2 -->|deploy/start| R2

  R1 -.hosts.-> W1
  R2 -.hosts.-> W2
```

---

### **6.2 Machine Plane**

The **Machine Plane** manages infrastructure resources and scheduling, with **no persistent state**. It is **disposable, fully decoupled, and autonomous** - rebuild at will.

#### **Responsibilities (explicit)**

* Node discovery via **Machine DHT**.
* Decentralized workload scheduling using ephemeral task negotiations.
* Fabric-level coordination through secure, mutually authenticated streams.
* Resource offer/bidding, capability advertisement, and local policy enforcement.
* Lifecycle hooks to start/stop workloads via the runtime (e.g., **Podman**).

> **Operational impact:** no etcd, no API servers, no manager quorum, no backups - **near-zero operational toll.**

---

### **6.3 Ephemeral Scheduling Process**

Traditional schedulers maintain global cluster state and enforce consensus (Raft, etcd), creating bottlenecks.

Beemesh uses **ephemeral scheduling**: **tasks are never persisted** and scheduling happens dynamically across the scale-out fabric.

#### **Step-by-Step Flow**

1. **Task Publication**

   * A new workload task is published on the `scheduler-tasks` Pub/Sub topic.
   * **Task payload includes:** resource requirements (CPU, memory, etc.), priority and QoS hints, and a **workload manifest reference**.

2. **Local Resource Evaluation**

   * Each node listens to the task topic and evaluates: current resource availability, local policies (affinity rules, hardware constraints).

3. **Bidding Phase**

   * Nodes **submit bids** to the `scheduler-proposals` topic.
   * **Bids include:** node identity (Machine Peer ID) and **scoring metrics** (resource fit, network locality, capabilities).

4. **Selection Window**

   * A short window (e.g., **100–500 ms**) allows multiple bids to arrive.
   * Nodes independently select the **best bid** based on scoring rules.

5. **Workload Deployment**

   * The winning node deploys the workload using **Podman**.
   * Deployment is confirmed via a follow-up Pub/Sub event.

#### **Advantages of Ephemeral Scheduling**

| Advantage                      | Why It Matters                                                  |
| ------------------------------ | --------------------------------------------------------------- |
| **No single point of failure** | No central scheduler to crash or partition.                     |
| **Low coordination overhead**  | Tasks are transient and do not require consensus.               |
| **Partition tolerance**        | Nodes continue to schedule independently during network splits. |
| **High throughput**            | Scales naturally with node count.                               |

---

### **6.4 Machine-to-Machine Security**

* Each machine verifies scheduling message authenticity using **Machine Peer IDs**.
* All communications are encrypted with `libp2p.Security` (TLS or Noise).
* Rogue nodes can’t influence scheduling without a valid cryptographic identity.

---

### **6.5 Work Plane**

The **Work Plane** runs inside every Pod.

| Function              | Description                                                                                                        |
| --------------------- | ------------------------------------------------------------------------------------------------------------------ |
| **Self-Healing**      | Monitors replica counts and autonomously spawns replacements using local manifest data.                            |
| **Service Discovery** | Registers workloads in the **Workload DHT** for peer lookup and routing.                                           |
| **Secure Networking** | Pod-to-pod traffic flows through mutually authenticated encrypted `libp2p` streams.                                |
| **Manifest Storage**  | **Stores manifests locally for offline recovery** during partitions; enables autonomous re-spawn even if isolated. |

**Workload-to-Workload Security**

* Each Pod uses a **unique Workload Peer ID**.
* Secure communication is **independent** of machine identities (separate trust domains).
* Prevents cross-plane privilege escalation.

---

## **7. Key Features**

* **Scalability**
  Two independent DHTs prevent machine churn from disrupting workloads.

* **Security by Design**

  * Unique Peer IDs for machines and workloads.
  * Mutually authenticated encrypted streams for both planes.

* **Self-Healing Workloads**
  Pods replicate themselves without relying on external control planes.

* **Lightweight Footprint**

  * Machine Plane Daemon: \~**50–80 MB RAM**.
  * Work Plane overhead: \~**20–40 MB** including the Beemesh container.

* **Kubernetes-Compatible**
  Accepts Kubernetes `Deployment` and `StatefulSet` manifests.

* **Pluggable Runtime**
  Deploy via **Podman** today; runtimes are swappable by design.

* **Disposable Machine Plane**
  Rebuild nodes freely; **image rotation** > patching. Perfect for **spot/preemptible** mixes.

---

## **8. True Multicloud: Concrete Scenarios & Patterns**

Beemesh treats **on-prem + Azure + AWS + GCP** as just more peers in the mesh. Mix and match freely.

### 8.1 Cross-cloud burst (Analytics/Batch)

* **Baseline** on-prem; **burst** into **AWS EC2** spot and **GCP Compute Engine** preemptibles when queue depth spikes.
* **Why it works:** ephemeral scheduling + disposable Machine Plane → no shared cluster state to extend or replicate across providers.

### 8.2 Data-gravity aware microservices

* **Frontends** in **Azure** (close to users), **stateful stores** pinned to **on-prem** or **AWS**.
* **Discovery** uses the **Workload DHT**, so services find the closest healthy peer automatically.

### 8.3 Active-active global services

* Run replicas across **AWS + Azure + GCP** regions simultaneously. If a provider browns out, the Work Plane routes to healthy workloads - **no failover controller** required.

### 8.4 Sovereign + public hybrid

* Keep sensitive workloads **air-gapped on-prem**; run stateless API edges in **GCP** and **Azure**.
* Periodic or courier-based sync patterns supported; the platform **does not require** public control endpoints.

### 8.5 HPC/ML fleet sharing

* **GPU islands** across providers (e.g., A100s in Azure, H100s in GCP, local Hoppers on-prem) appear as one pool of offers.
* Jobs pick best fit (capabilities, price, proximity to data) via the bidding mechanism.

> **Tip:** Because the Machine Plane is decoupled and disposable, you can aggressively **mix spot/preemptibles** with on-demand and on-prem **without risking control-plane stability**.

---

## **9. Air-Gapped & Disconnected Enterprises**

* **Works fully offline**: discovery and scheduling occur entirely inside the isolated network.
* **Bootstrap** via pre-shared keys/offline identity distribution - **no external CA dependency** required.
* **Controlled sync**: when/if connectivity exists, bridge selected workloads or artifacts via tightly scoped gateways - **by policy, not necessity**.
* **DR without SaaS**: replicate only **workload state** that matters; machines remain disposable.

---

## **10. Use Cases**

| Scenario                     | Why Beemesh Works                                                            |
| ---------------------------- | ---------------------------------------------------------------------------- |
| **Edge & IoT Networks**      | Operates in unreliable, partitioned networks with minimal resources.         |
| **Multicloud Microservices** | One service mesh across on-prem + Azure + AWS + GCP - no vendor lock-in.       |
| **Global Analytics/Batch**   | Elastic bursts across providers; ephemeral scheduling matches queue spikes.  |
| **Smart Cities / Telco**     | Millions of devices with frequent churn; Machine Plane is throwaway.         |
| **Enterprise Databases**     | State lives with the workload’s own quorum; infra failures don’t corrupt it. |
| **Air-Gapped IT/OT**         | Full functionality inside isolated networks; optional selective sync.        |
| **Enterprise Workloads**     | Strong, consistent, reliable on-prem and multicloud behaviors by design.     |

---

## **11. Scalability Advantages**

1. **Two Independent DHTs**

   * Machine DHT absorbs rapid infrastructure churn.
   * Workload DHT remains stable and performant.

2. **No Global Consensus**

   * Eliminates `etcd` and cluster-wide Raft bottlenecks.

3. **Scale-out Fabric Foundation**

   * Built on `libp2p`, proven to scale to **tens of thousands of peers**.
   * **Scale is bounded primarily by network capacity (`libp2p`).**

---

## **12. Security Summary**

| Communication Type      | DHT Used     | Peer IDs          | Encryption                                         |
| ----------------------- | ------------ | ----------------- | -------------------------------------------------- |
| **Machine ↔ Machine**   | Machine DHT  | Machine Peer IDs  | Mutually authenticated `libp2p` stream (TLS/Noise) |
| **Workload ↔ Workload** | Workload DHT | Workload Peer IDs | Mutually authenticated `libp2p` stream (TLS/Noise) |

**Guarantees:**

* Machine- and Work-Planes operate as **separate trust domains**.
* Compromising one layer does **not** expose the other.

---

## **13. Getting Started (Conceptual)**

1. **Install** the single Beemesh daemon on any machines (on-prem VMs, bare metal, cloud instances).
2. **Join** the Machine DHT (via bootstrap peers or offline bundles for air-gapped).
3. **Submit** workloads (Kubernetes-style `Deployment`/`StatefulSet` manifests supported).
4. **Watch** the fabric schedule, run, and heal - across providers - **without a control plane**.

> Day-2 is basically **image rotation + key management**. That’s the point.

---

## **14. Summary**

Beemesh represents a **paradigm shift** in orchestration:

* Eliminates centralized control planes and scaling bottlenecks.
* Uses **ephemeral, decentralized scheduling** for effectively unparalleled scalability.
* Provides **strict isolation** between infrastructure and workloads via two DHTs and unique Peer IDs.
* Ensures **all communications are mutually authenticated and encrypted**.
* Scales to **tens of thousands of nodes**, ideal for edge, IoT, cloud, **multicloud**, and **air-gapped** environments.
* **Disposable, fully decoupled Machine Plane** → autonomous, low-toil operations.

> **Beemesh isn’t just another orchestrator - it's a secure, scale-out fabric for the future of global computing.**

---

## **Appendix: Comparison Matrix**

> *Note on “Mutually Authenticated Streams”: legacy systems may add this via optional plugins/sidecars, but it’s **not** default cluster behavior.*

| Feature                                 | Kubernetes        | Nomad             | **Beemesh**                 |
| --------------------------------------- | ----------------- | ----------------- | --------------------------- |
| Central Control Plane                   | Yes               | Yes               | **No**                      |
| Separate Machine & Workload DHTs        | No                | No                | **Yes**                     |
| Unique Peer IDs per Machine             | No                | No                | **Yes**                     |
| Unique Peer IDs per Workload            | No                | No                | **Yes**                     |
| Mutually Authenticated Machine Streams  | Optional (addons) | Optional (addons) | **Yes (default)**           |
| Mutually Authenticated Workload Streams | Optional (addons) | Optional (addons) | **Yes (default)**           |
| Global Consensus Required               | Yes               | Yes               | **No**                      |
| Scalability Ceiling                     | \~5,000 nodes     | \~10,000 nodes    | **Tens of thousands+**      |
| IoT Suitability                         | Medium            | Medium            | **Excellent**               |
| Edge Suitability                        | Medium            | Medium            | **Excellent**               |
| Enterprise Suitability                  | Excellent         | Excellent         | **Excellent (+air-gapped)** |

---

## **Community & Support**

- **GitHub**: [github.com/beemesh/beemesh](https://github.com/beemesh/beemesh)
- **Documentation**: [docs.beemesh.io](https://docs.beemesh.io)

---

**License**: Apache 2.0
