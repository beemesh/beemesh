# Beemesh - Commercial Planning (v0.2)

> **Purpose:** Align product, engineering, and GTM on building a sustainable business while preserving the open, distributed secure fabric architecture.

---

## 1) Positioning & One-liner

**Beemesh** is an open, distributed secure fabric that turns any machine into a global, self-healing compute pool with **NoOps** scheduling - enabling **true multicloud** without a centralized control plane to run or replicate.

* **Beemesh (Apache-2.0)**: Production-ready choreography with decentralized scheduling, dual DHTs, secure identities, and self-healing workloads. Runs anywhere including air-gapped. **No feature gating.**
* **Beemesh Commercial (BSL/Commercial)**: Fleet governance, policy controls, SSO/RBAC, audit logging, and commercial support packaged for business environments.

**Business value (exec-ready)**

* **Lower cost:** Use the cheapest capacity across clouds + on-prem (spot/preemptible + owned) with policy control.
* **Higher resilience:** No control plane to babysit; self-healing by default; works when links are flaky or offline.
* **Faster delivery:** Deploy new regions/providers in hours, not months; same packaging everywhere.
* **Stronger posture:** Mutual TLS with workload identities; default-deny policies; tenant/space segmentation.

---

## 2) Packaging & Licensing

| Tier                   | License        | What is inside                                                                                                                                                                                                                                           | Primary customer value                                                                        |
| ---------------------- | -------------- | --------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------- |
| **Beemesh**            | Apache-2.0     | **Full choreography**: Machineplane + Workplane, dual DHTs, secure streams, ReplicaSet/Job controllers, Podman adapter, metrics, REST API.           | **True multicloud runtime** that runs anywhere (incl. air-gapped) with one operational model. |
| **Beemesh Commercial** | BSL/Commercial | Adds: Org namespaces, SSO/SAML, policy engine (OPA), audit log, Control Tower UI, LTS/FIPS builds, priority support.                                   | **Fleet governance** and compliance without re-introducing a central control plane.     |

---

## 3) Hub (SaaS for Package Management)

**Tagline:** The secure supply chain for Beemesh fabrics.

**How it works**

* **Package format:** OCI image + `beemesh.yaml` (capabilities, planes) + SBOM + in-toto provenance. Signed with cosign.
* **On-prem client:** Each fabric runs a lightweight Hub client daemon (not a full Hub). It syncs packages, verifies signatures, enforces policies.
* **Offline mode:** Export/import **Hub Bundles** (`.bmz`) via USB/air-gap courier. No cloud dependency.
* **Publisher tiers:** 
  * Community (signature verified)
  * **Hub Verified** (security scan + e2e tests)
  * **Hub Certified** (vendor support, SLOs, paid)
* **Economics:** Publisher-set pricing; 85/15 rev-share (publisher/Beemesh). We handle billing, entitlements, and license token generation for offline use.

---

## 4) Pricing (Commercial Anchors)

* **Beemesh Commercial per-node:** $150-300/node/year (volume discounts from 50 nodes). Minimum $25k/year. Includes Hub SaaS access.
* **Hub SaaS standalone:** $500/month base + $0.10/GB package delivery. For teams not ready for Commercial.
* **Support for OSS users:**
  * Community: GitHub + Discord (best effort)
  * Standard: $15k/year, 8x5, 24h response SLA
  * Premium: $50k/year, 24x7, phone support
* **Professional services:**
  * Implementation packages: $35k (standard) to $150k (complex air-gap)
  * Training: $6k/day on-site

> Priced 30-50% below managed K8s control plane costs while delivering multicloud capabilities.

---

## 5) GTM Plan (0-180 days)

### 0-30 days (Proof of Value)

* Ship **Quickstart** + 2-minute demo: deploy → kill → self-heal → metrics.
* Publish **architectural proof points** (see §7) with reproducible repos.
* Launch **Design-Partner Program** (see §6): target 5 pilots, $75k fixed fee.

### 31-90 days (Beta)

* Hub Beta: install/list/verify, offline bundles, Verified tier.
* Commercial: org CA, SSO/SAML, OPA policies, audit log, Control Tower v1.
* Publish **three technical proof points** with dashboards.

### 91-180 days (Early GA)

* Hub Certified pipeline + payments/entitlements.
* Commercial: quotas, priority tiers, LTS/FIPS builds, TPM attestation preview.
* Land 3 paid pilots → convert to $200k+ annual contracts.
* Announce at KubeCon.

---

## 6) Design-Partner Program

**Who:** Edge retail, air-gapped energy/defense, GPU/ML platforms, sovereign providers.

**Structure (6 months, $75k fixed fee):**

1. **Wk 1-2:** Discovery & threat model sign-off. Define 3 measurable success criteria.
2. **Wk 3-8:** Pilot sprints
   * Sprint 1: Partition/duplicate-bounding demo (target: <60s convergence)
   * Sprint 2: Air-gapped bootstrap + key rotation (target: zero external deps)
   * Sprint 3: Spot burst economics vs K8s baseline (target: 40%+ cost reduction)
3. **Wk 9-10:** Joint GTM deliverables (case study, blog, conference abstract).
4. **Wk 11-12:** Exec readout & roll-forward plan. $75k credited to first-year Commercial.

**Deliverables:** Production config, team training, documented ROI.

---

## 7) Proof Points to Publish

1. **Partition-first video:** 3 clouds + on-prem; kill 50% of nodes; show bounded duplicates (<3 per workload); self-heal in <60s.
2. **Air-gapped bootstrap:** USB join pack; operate offline 7 days; rotate keys; restart workloads without cloud.
3. **Spot burst economics:** 1k GPU jobs across AWS/GCP; compare $/job vs K8s; show p99 schedule latency (<500ms).

**Artifacts:** GitHub repos, Grafana dashboards, cost analysis spreadsheets.

---

## 8) Security, Compliance, and Ops

* **Identity:** Distinct machine/workload keyspaces; org CA admission.
* **Supply chain:** Cosign + in-toto mandatory for Hub Verified+; SBOMs published.
* **Audit:** Commercial audit log captures tasks, installs, policy changes (tenant-scoped).
* **Compliance:** FIPS-140-2 build roadmap; SOC2 Type I by Q4.
* **Control Tower:** Read-only by design. Shows inventory, rollout status, golden signals. Write actions are audit-logged but not choreography-critical.

---

## 9) Ecosystem Governance (Hub)

* **Submission:** Publisher identity verification; automated CVE scan; e2e smoke test.
* **Quality gates:** Resource caps, upgrade/rollback tests, explicit capabilities, no fabric-global secrets.
* **Revocation:** <1h from CVE notification to bundle revocation; client checks every 5min.
* **Telemetry:** Opt-in only; air-gapped fabrics can send usage proofs via bundle export.

---

## 10) Roadmap

| Quarter | Hub | Commercial | Community |
|---------|-----|------------|-----------|
| **Q1** | Beta, Verified tier, offline bundles | Orgs, SSO, OPA, audit, Control Tower v1 | Stable v0.2 |
| **Q2** | Payments, Certified reviews, entitlements | Quotas, priority tiers, LTS/FIPS | Windows support |
| **Q3** | Partner co-sell, private catalogs | TPM attestation, HSM integration | Workplane SDK |
| **Q4** | Marketplace API | SOC2 Type I | GPU scheduling |

---

## 11) Success Metrics

| Category | Metric | Target (6mo) |
|----------|--------|--------------|
| **Activation** | Quickstart completion rate | >60% |
| **Reliability** | p95 schedule latency | <500ms |
| **Resilience** | Duplicate-boundedness (max replicas/task) | ≤3 |
| **Adoption** | Hub installs per tenant | >5 |
| **Revenue** | Design-partner conversion | 3 → $200k+ ARR |

---

## 12) Risks & Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| **Fabric skepticism** | High | Medium | Publish 10k-node simulation; show sharding metrics |
| **Data-safety FUD** | High | High | Demo write-fencing with Raft KV; publish safety analysis |
| **Pricing pushback** | Medium | High | ROI calculator; pilot metrics vs K8s baseline |
| **Support scaling** | Medium | High | Certified tier requires publisher support |
| **Scope creep** | Low | High | RFC process for open-core changes |

---

## 13) Appendices

### A) `beemesh.yaml` (Hub package manifest)

```yaml
name: org/postgres-ha
version: 14.7.1
channel: stable
kind: hub-package
permissions:
  planes: ["workplane"]
  capabilities: ["wdht:read","tasks:publish"]
signing:
  cosign: ghcr.io/org/postgres-ha:sha256-abc.sig
attestations:
  sbom: spdx.json
  provenance: intoto.jsonl
compatibility:
  core: ">=0.2.0"
  commercial: ">=0.2.0"
```

### B) ROI Calculator

```
Baseline cost (K8s):
  Control plane: $150/node/year × 1000 = $150k
  Ops overhead: $85k
  Underutilization: $180k
  Total: $415k

Beemesh cost:
  Commercial license: $250/node/year × 1000 = $250k
  Spot savings: -$240k
  Ops overhead: $17k
  Total: $27k

Net savings: $388k (93% reduction)
```

### C) Competitive Positioning

| vs. | Beemesh Advantage |
|-----|-------------------|
| **AWS EKS** | No $72/node CP cost; true multicloud; air-gap ready |
| **HashiCorp Nomad** | True scale-out fabric; stronger security; no Raft quorum to manage |
| **K3s** | Not just lightweight - decentralized; no control plane at all |
