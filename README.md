# **Beemesh: Global Mesh Computing**

> **Beemesh** is a decentralized, lock-free orchestration system that turns any device — cloud, on-prem, edge, or IoT — into an interchangeable compute resource.
> It scales infinitely by eliminating the centralized control plane, enabling **secure, self-healing workloads** across highly dynamic environments.

---

## **1. Introduction**

Modern orchestration systems like Kubernetes and Nomad are powerful but **inherently limited by centralization**.
As clusters grow:

* Control planes become **scaling bottlenecks**.
* Consensus overhead increases.
* Infrastructure failures ripple through workloads.

Beemesh rethinks orchestration from the ground up:

* **No central control plane** — fully peer-to-peer coordination.
* **Workload-scoped consistency** — each application carries its own state management.
* **Separate security and discovery planes** — machines and workloads each operate on their own DHT with independent trust domains.
* **Infinite scale** — limited only by network capacity (`libp2p`).

---

## **2. Why Beemesh**

| Problem in Legacy Systems                                                | Beemesh Solution                                                                  |
| ------------------------------------------------------------------------ | --------------------------------------------------------------------------------- |
| Scaling beyond 5,000–10,000 nodes is hard due to control plane overhead. | **Fully decentralized coordination** using `libp2p` DHT + Pub/Sub.                |
| Machine failures destabilize cluster state.                              | Machines are **fungible resources**; state lives only inside workloads.           |
| High operational complexity (etcd, API servers, Raft quorums).           | **Single lightweight daemon** (\~50–80MB RAM).                                    |
| Weak identity and trust at scale.                                        | **Separate Peer IDs for machines and workloads**, mutually authenticated streams. |
| Vendor lock-in to specific clouds or infra.                              | **Infrastructure-agnostic**, runs on anything.                                    |

---

## **3. Core Concepts**

### **3.1 Two-Plane Architecture**

Beemesh divides orchestration into **two planes**:

| Plane              | Purpose                                                               | CAP Trade-off                                |
| ------------------ | --------------------------------------------------------------------- | -------------------------------------------- |
| **Machine Plane**  | Node-level coordination, scheduling, and resource management.         | **A/P** – Availability & Partition Tolerance |
| **Workload Plane** | Application-level self-healing, service discovery, secure networking. | **C/P** – Consistency & Partition Tolerance  |

This separation eliminates cross-cutting concerns and allows **each plane to optimize independently**.

---

### **3.2 Separate DHTs**

Beemesh uses **two distinct Distributed Hash Tables (DHTs):**

| DHT              | Purpose                                                                                                                                                |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **Machine DHT**  | - Node discovery and coordination.<br>- Task scheduling and negotiation.<br>- Stores only minimal, transient machine metadata.                         |
| **Workload DHT** | - Service discovery between workloads.<br>- Maintains workload-level peer state and connectivity info.<br>- No machine-level data is ever stored here. |

> **Key benefit:**
> Machine failures do **not** pollute the workload discovery layer.
> Each DHT is optimized for its purpose, ensuring scalability and reliability.

---

### **3.3 Workload-Scoped Consistency**

Traditional systems bind **consistency to infrastructure**:

* Kubernetes uses `etcd` for cluster state.
* Nomad and Consul use Raft quorums for global consensus.

**Beemesh flips this model:**

* **Infrastructure is stateless** and ephemeral.
* Each **stateful workload carries its own consensus cluster** (e.g., Raft group for a database).

> This ensures that infrastructure failures never corrupt workload state and enables scaling to **tens of thousands of nodes**.

---

## **4. Security and Identity Model**

### **4.1 Unique Peer IDs**

Beemesh assigns **separate cryptographic identities** to machines and workloads:

| Entity       | Identity Details                                                                                                                                                                                  |
| ------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Machine**  | - Each machine has a **unique `libp2p` Peer ID** generated at first startup.<br>- Backed by a public/private key pair.<br>- Used for node discovery, scheduling, and machine-plane communication. |
| **Workload** | - Each workload Pod also has a **unique `libp2p` Peer ID**.<br>- Backed by a separate public/private key pair.<br>- Used only for service discovery and secure workload-to-workload traffic.      |

This separation ensures **complete isolation of trust boundaries** between the infrastructure layer and the workloads running on top of it.

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

| System     | Control Plane                   | Scaling Limit                | State Model                         | Notes                                            |
| ---------- | ------------------------------- | ---------------------------- | ----------------------------------- | ------------------------------------------------ |
| Kubernetes | Centralized (etcd + API server) | \~5,000 nodes / 150,000 pods | Strong consistency cluster-wide     | Rich ecosystem but control-plane limited         |
| Nomad      | Centralized Raft quorum         | Thousands of nodes           | Strong consistency global scheduler | Lighter than K8s but still infra-bound           |
| Swarm      | Raft manager nodes              | \~1,000 nodes                | Strong consistency cluster-wide     | Simple but infra-coupled                         |
| Beemesh    | None – peer-to-peer             | Tens of thousands of nodes+  | **Consistency scoped to workload**  | Infinite scale, only stateful workloads run Raft |

---

## **6. Architecture**

### **6.1 High-Level Diagram**

```
                      +---------------------------------------+
                      |             Workload Plane            |
                      |---------------------------------------|
                      | Secure Workload Communication (libp2p)|
                      | Workload DHT for Service Discovery    |
                      | Stateful Raft Clusters for App State  |
                      +---------------------------------------+
                                  ^            ^
                                  |            |
            Workload ↔ Workload Secure Streams | (TLS/Noise)
                                  |
                          Pub/Sub Messaging
                                  |
                      +---------------------------------------+
                      |              Machine Plane            |
                      |---------------------------------------|
                      | Machine DHT for Node Discovery        |
                      | Ephemeral Scheduler & Resource Bidding|
                      | Machine ↔ Machine Secure Streams      |
                      +---------------------------------------+
```

---

### **6.2 Machine Plane**

The **Machine Plane** manages infrastructure resources and scheduling, with **no persistent state**.

#### **Responsibilities**

* Node discovery via **Machine DHT**.
* Decentralized workload scheduling using ephemeral task negotiations.
* Peer-to-peer coordination through secure, mutually authenticated streams.

---

### **6.3 Ephemeral Scheduling Process**

Traditional schedulers rely on a **centralized control plane**, maintaining global cluster state and enforcing consistency through consensus systems like Raft.
This creates bottlenecks and limits scalability.

Beemesh introduces **ephemeral scheduling**, where **tasks are never persisted** and scheduling happens dynamically across the mesh:

#### **Step-by-Step Flow**

1. **Task Publication**

   * A new workload task is published on the `scheduler-tasks` Pub/Sub topic.
   * Tasks contain:

     * Resource requirements (CPU, memory, etc.).
     * Priority and QoS hints.
     * Workload manifest reference.

2. **Local Resource Evaluation**

   * Each node listens to the task topic and evaluates:

     * Its current resource availability.
     * Local policies (e.g., affinity rules, hardware constraints).

3. **Bidding Phase**

   * Nodes **submit bids** to the `scheduler-proposals` topic.
   * Bids include:

     * Node identity (Machine Peer ID).
     * Scoring metrics like resource fit and network locality.

4. **Selection Window**

   * A short window (e.g., 100–500ms) allows multiple bids to arrive.
   * Nodes independently select the **best bid** based on scoring rules.

5. **Workload Deployment**

   * The winning node deploys the workload using Podman.
   * Deployment is confirmed via a follow-up Pub/Sub event.

---

#### **Advantages of Ephemeral Scheduling**

| Advantage                      | Why It Matters                                                  |
| ------------------------------ | --------------------------------------------------------------- |
| **No single point of failure** | No central scheduler to crash or partition.                     |
| **Low coordination overhead**  | Tasks are transient and do not require consensus.               |
| **Partition tolerance**        | Nodes continue to schedule independently during network splits. |
| **High throughput**            | Scales naturally with node count.                               |

---

### **6.4 Machine-to-Machine Security**

* Each machine verifies the authenticity of scheduling messages using **Machine Peer IDs**.
* All communications are encrypted with `libp2p.Security` (TLS or Noise).
* Rogue nodes cannot influence scheduling without valid cryptographic identity.

---

### **6.5 Workload Plane**

The **Workload Plane** operates inside every Pod.

| Function              | Description                                                                             |
| --------------------- | --------------------------------------------------------------------------------------- |
| **Self-Healing**      | Monitors replica counts and autonomously spawns replacements using local manifest data. |
| **Service Discovery** | Registers workloads in the **Workload DHT** for peer lookup and routing.                |
| **Secure Networking** | Pod-to-pod traffic flows through mutually authenticated encrypted libp2p streams.       |
| **Manifest Storage**  | Stores manifests locally for offline recovery during network partitions.                |

**Workload-to-Workload Security:**

* Each pod uses a **unique Workload Peer ID**.
* Secure communication occurs independently of machine identities.
* Enforces strict isolation and prevents cross-plane privilege escalation.

---

## **7. Key Features**

* **Infinite Scalability:**
  Two independent DHTs prevent machine churn from disrupting workloads.
* **Security by Design:**

  * Unique Peer IDs for machines and workloads.
  * Mutually authenticated encrypted streams for both planes.
* **Self-Healing Workloads:**
  Pods replicate themselves without relying on external control planes.
* **Lightweight Footprint:**

  * Machine Plane Daemon: \~50–80MB RAM.
  * Workload Pods: \~20–40MB including Beemesh container.
* **Kubernetes-Compatible:**
  Accepts Kubernetes `Deployment` and `StatefulSet` manifests.

---

## **8. Use Cases**

| Scenario                    | Why Beemesh Works                                                       |
| --------------------------- | ----------------------------------------------------------------------- |
| **Edge IoT Networks**       | Operates in unreliable, partitioned networks with minimal resources.    |
| **Multi-Cloud Deployments** | Runs seamlessly across heterogeneous cloud providers.                   |
| **Smart Cities**            | Self-healing workloads for millions of sensors and devices.             |
| **Large-Scale Analytics**   | Stateless and stateful workloads scale independently of infrastructure. |

---

## **9. Scalability Advantages**

1. **Two Independent DHTs**

   * Machine DHT handles rapid infrastructure churn.
   * Workload DHT remains stable and performant.

2. **No Global Consensus**

   * Eliminates `etcd` and cluster-wide Raft bottlenecks.

3. **Peer-to-Peer Foundation**

   * Built on `libp2p`, proven to scale to **tens of thousands of peers**.

---

## **10. Setup**

### **10.1 Prerequisites**

* Go 1.20+
* Podman container runtime
* Dependencies listed in `go.mod`

### **10.2 Quick Start**

```bash
# Build Machine Plane binary
CGO_ENABLED=0 GOOS=linux go build -o beemesh ./cmd/machine/main.go

# Build Workload Plane image
docker build -t beemesh-infra -f Dockerfile.workload .

# Run Beemesh daemon
./beemesh
```

---

## **11. CLI: beectl**

| Command                     | Purpose           |
| --------------------------- | ----------------- |
| `beectl create -f app.yaml` | Deploy workloads  |
| `beectl get pods`           | List running pods |
| `beectl delete pod <name>`  | Remove a pod      |

---

## **12. Security Summary**

| Communication Type      | DHT Used     | Peer IDs          | Encryption                                         |
| ----------------------- | ------------ | ----------------- | -------------------------------------------------- |
| **Machine ↔ Machine**   | Machine DHT  | Machine Peer IDs  | Mutually authenticated `libp2p` stream (TLS/Noise) |
| **Workload ↔ Workload** | Workload DHT | Workload Peer IDs | Mutually authenticated `libp2p` stream (TLS/Noise) |

**Guarantees:**

* Machine-plane and workload-plane operate as **separate trust domains**.
* Compromising one layer does **not** expose the other.

---

## **13. Roadmap**

| Phase           | Goals                                                                    |
| --------------- | ------------------------------------------------------------------------ |
| **Alpha**       | Core networking, ephemeral scheduling, CLI                               |
| **Beta**        | Kubernetes API compatibility, edge performance tuning                    |
| **1.0 Release** | Production-ready, security audits, decentralized marketplace integration |

---

## **14. Summary**

Beemesh represents a **paradigm shift** in orchestration:

* Eliminates centralized control planes and scaling bottlenecks.
* Uses **ephemeral, decentralized scheduling** for infinite scalability.
* Provides **strict isolation** between infrastructure and workloads via two DHTs and unique Peer IDs.
* Ensures **all communications are mutually authenticated and encrypted**.
* Scales to **tens of thousands of nodes**, making it ideal for edge, IoT, and cloud environments.

> **Beemesh is not just an orchestrator — it's a secure, decentralized mesh for the future of global computing.**

---

## **Appendix: Comparison Table**

| Feature                                 | Kubernetes    | Nomad          | Beemesh                |
| --------------------------------------- | ------------- | -------------- | ---------------------- |
| Central Control Plane                   | Yes           | Yes            | **No**                 |
| Separate Machine & Workload DHTs        | No            | No             | **Yes**                |
| Unique Peer IDs per Machine             | No            | No             | **Yes**                |
| Unique Peer IDs per Workload            | No            | No             | **Yes**                |
| Mutually Authenticated Machine Streams  | No            | No             | **Yes**                |
| Mutually Authenticated Workload Streams | No            | No             | **Yes**                |
| Global Consensus Required               | Yes           | Yes            | **No**                 |
| Scalability Ceiling                     | \~5,000 nodes | \~10,000 nodes | **Tens of thousands+** |
| Edge/IoT Suitability                    | Poor          | Medium         | **Excellent**          |

---
