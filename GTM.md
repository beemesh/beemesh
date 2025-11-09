# Beemesh - Commercial Planning

> **Purpose:** This doc aligns product, engineering, and GTM on how Beemesh becomes a sustainable business while preserving the open, distributed secure fabric architecture.

---

## 1) Positioning & One‑liner

**Beemesh** is an open, distributed secure fabric platform that turns any machine-cloud, on‑prem, edge-into a global, self‑healing compute pool with **NoOps** scheduling.

* **Beemesh (OSS)**: the foundation-decentralized scheduling, dual scoped service registries, secure identities, and self-healing workloads. Runs anywhere, including air-gapped. Optionally deploy the **Hub** app on Beemesh to discover, verify, and deploy add-ons.
* **Beemesh Enterprise (BSL/Commercial)**: the enterprise wrapper-policy, identity, governance, support, and fleet-scale ops packaged for regulated/large environments.

---

## 2) Packaging & Licensing

| Tier                   | License        | What’s inside                                                                                                                                                                                 | Notable Boundaries                                                                                                      |
| ---------------------- | -------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| **Beemesh**            | Apache‑2.0     | Machineplane + Rust Workplane, WDHT, sharded pub/sub, LeaseHints, basic controllers (ReplicaSet subset, Job subset, Headless Service), Podman adapter, REST apply/watch, metrics & dashboards | Always runnable offline. Protocols & SDKs open and stable. |
| **Beemesh Enterprise** | BSL/Commercial | Org CA & namespaces, policy & quotas, SSO/RBAC, audit log, read‑only Control Tower (inventory/rollouts/golden signals), LTS/FIPS, optional relay/bootstrap network                            | No mandatory SaaS. Control Tower is out‑of‑band and read‑only (no central control plane).                               |

---

## 3) Beemesh Hub

**Tagline:** Discover, verify, and deploy community or enterprise workloads and services.

* **Package format:** OCI images + `beemesh.yaml` (permissions/capabilities/planes) + SBOM + in‑toto provenance. Signed with cosign.
* **Channels:** `stable` / `beta` / `canary`; semantic versioning; rollback support.
* **Security:** signature verify, policy evaluation before install, tenant scoping, explicit capabilities.
* **Offline:** export/import **Hub Bundles** (`.bmz`) for disconnected sites.
* **Publisher tiers:** Community (signature verified), **Hub Verified** (security scan + e2e tests), **Hub Certified** (vendor support, SLOs, paid).
* **Economics:** publisher‑set pricing; 80/20 rev‑share (publisher/Beemesh).

---

## 4) Pricing (initial anchors)

* **Enterprise license:** per managed node/year. Tiers with volume discounts. Indicative: $150–$300/node/year (min $25k–$50k).
* **Hub paid packages:** price set by publisher; billed via Hub (online) or via license token (offline). Beemesh rev‑share 20%.
* **Relays/Bootstrap network (optional):** usage‑based (peer‑hours + egress GB).
* **Support for OSS users:** annual contracts with response SLOs.

> Pricing will evolve with design partners. Keep the open‑core boundary crisp and public.

---

## 5) GTM Plan (0–180 days)

### 0–30 days (Prototype → Show HN / Design Partners)

* Publish **Quickstart** + 2‑minute demo (deploy → kill → self‑heal → metrics).
* Open **commercial.md** (this doc) + pricing page placeholders.
* Identify 20 target logos across **edge/air‑gap/GPU burst**.
* Launch **Design‑Partner Program** (see §6): 3–6 month pilots with fixed price and success criteria.

### 31–90 days (Beta)

* Ship **Hub Beta**: install/list/verify, offline bundles, Verified tier.
* Enterprise: org CA integration, namespaces/tenants, audit log, read‑only Control Tower v1.
* Publish **three proof points** (partition demo, air‑gap bootstrap, spot burst economics).

### 91–180 days (Early GA motion)

* Hub Certified pipeline + payments/entitlements.
* Enterprise: SSO/SAML/RBAC, quotas/priority tiers, LTS channel.
* First‑party Hub packs: GPU scoring extension, storage adapter, SIEM exporter.
* Land 2–3 paid pilots → convert to annual.

---

## 6) Design‑Partner Program

**Who:** edge/retail/telco, air‑gapped/regulated (energy/defense), GPU/ML platforms.

**Structure (3–6 months):**

1. **Discovery** & success criteria (2–3 measurable outcomes).
2. **Architecture & threat model** sign‑off.
3. **Pilot sprints:**

   * Week 2: partition/duplicate‑bounding demo
   * Week 4: air‑gapped bootstrap + offline key rotation
   * Week 6: spot/preemptible burst economics
4. **Exec readout** & roll‑forward plan.

**Commercials:** fixed fee ($30–$60k) credited to first‑year Enterprise; joint PR/case study option.

---

## 7) Proof Points to Publish (trust builders)

1. **Partition‑first video:** 3 clouds + on‑prem; duplicates bounded; post‑heal drains to 1 in < 60s; fenced writes.
2. **Air‑gapped bootstrap:** USB join pack; full function offline; key rotation.
3. **Spot vs on‑prem economics:** $/job and utilization vs k8s autoscaler baseline.

Artifacts: reproducible repos, dashboards, and step‑by‑step scripts.

---

## 8) Security, Compliance, and Ops

* **Identity:** distinct machine/workload keyspaces; org CA admission.
* **Supply chain:** cosign + in‑toto, SBOM required for Hub Verified+.
* **Audit:** Enterprise audit log (tasks, installs, policy changes), tenant‑scoped.
* **Compliance track:** FIPS build, TPM/attestation (roadmap), SOC2 journey (policy docs, SDLC, incident playbooks).

---

## 9) Ecosystem Governance (Hub)

* **Submission:** publisher identity verification; automated scans; e2e smoke tests.
* **Quality gates:** resource caps, upgrade/rollback tests, explicit permissions, no cluster‑global secrets.
* **Revocation:** fast revoke & notification path; patched version guidance.
* **Telemetry:** opt‑in only; air‑gapped usage proofs supported.

---

## 10) Roadmap (business‑relevant)

* **Q1:** Hub Beta; Enterprise CA/namespaces/audit; Control Tower (read‑only).
* **Q2:** Payments/entitlements; Certified reviews; SSO/SAML/RBAC; LTS/FIPS track.
* **Q3:** Storage & secrets adapters; GPU scoring; quota/priority policies; partner co‑sell.

---

## 11) Success Metrics

* **Activation:** Quickstart completes (%), time‑to‑first‑deploy (TTFSD).
* **Reliability:** p95 schedule latency, duplicate‑boundedness, self‑heal MTTR.
* **Adoption:** Hub installs per tenant, Verified/Certified package share.
* **Revenue:** design‑partner conversions, nodes under Enterprise, Hub GMV & rev‑share.
* **Ecosystem:** # publishers, package quality score, time‑to‑patch vulnerabilities.

---

## 12) Risks & Mitigations

* **fabric skepticism:** publish sharding/relay metrics; show stability under load.
* **Data safety fears:** demo write fencing with a small Raft KV; clear explainer.
* **Air‑gap blockers:** first‑class offline bundles; no cloud hard‑deps.
* **Support load:** Certified tier obligations for publishers; diagnostics bundles.
* **Scope creep:** open‑core boundary documented; change via RFC only.

---

## 13) Appendices

### A) `beemesh.yaml` (Hub package manifest - sketch)

```yaml
name: org/app
version: 1.2.3
channel: stable
kind: hub-package
permissions:
  planes: ["workplane"]            # or ["machineplane","workplane"]
  capabilities: ["wdht:read","tasks:publish"]
signing:
  cosign: true
attestations:
  sbom: spdx.json
  provenance: intoto.jsonl
compatibility:
  core: ">=0.2.0"
  enterprise: ">=0.2.0"
```

### B) CLI snippets

```bash
# Hub online
bm hub list
bm hub info org/app
bm hub install org/app@v1.2.3 --tenant retail-eu --approve-perms

# Hub offline
bm hub bundle export org/app@v1.2.3 -o app.bmz
bm hub bundle import ./app.bmz

# Enterprise policy
bm tenant create retail-eu --namespace retail-eu
bm policy set retail-eu --quota cpu=200 --priority-tier=gold
```

### C) FAQ (short)

* **Does Enterprise require the Hub?** Enterprise ships the **Hub client**; you can run Hub offline with bundles.
* **Is there a control plane?** No. Control Tower is read‑only and out‑of‑band; it never orchestrates workloads.
* **Can we build our own packages?** Yes-use `beemesh.yaml`, sign with cosign, publish to Hub or side‑load via bundle.



