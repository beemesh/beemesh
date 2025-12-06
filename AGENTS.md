# AGENTS.md

This file defines **mandatory behavior** for all AI agents (Copilot, ChatGPT, etc.) working in this repository.
Deviations from these rules are considered **incorrect behavior**.

The primary goals are:

* **Security-first**
* **Correctness-first**
* **No “vibe coding”**

---

## Required Reading

### Core Project Overview
- **Project README:** `README.md`

### Specifications
- **Machineplane Specification:** `machineplane/machineplane-spec.md`
- **Machineplane Test Specification:** `machineplane/tests/test-spec.md`
- **Workplane Specification:** `workplane/workplane-spec.md`
- **Workplane Test Specification:** `workplane/tests/test-spec.md`
- **Technical Specification:** `technical-spec.md`

Agents must refer to these specification files before making changes to their corresponding subsystems.

---

## Global Mode (All Coding & Editing)

Whenever you **generate, modify, or refactor code** in this repository:

1. **Act as your own security & correctness reviewer.**
   Assume that any code you emit will immediately be run in production unless you clearly mark it as experimental.

2. **No vibe coding.**
   Do not produce rushed, speculative, or hand-wavy code. Avoid:

   * “TODO” or “in production you’d…” comments in non-test code.
   * Half-implemented error handling.
   * Placeholder validation that doesn’t actually validate.

3. **Defensive by default.**
   For any code dealing with:

   * Network I/O
   * External input (RPC, HTTP, disk, env, user input)
   * Concurrency / async / threads / tasks

   Always:

   * Validate lengths, bounds, and ranges.
   * Enforce **timeouts**, **limits**, and **cancellation**.
   * Prefer **explicit error handling** over silent failures.
   * Avoid magic numbers: introduce **named constants** for sizes, timeouts, and limits.

4. **Tests and behavior coverage.**
   When adding or changing non-trivial logic, either:

   * Add tests, or
   * Clearly describe which tests should exist (inputs, edge cases, failure paths, concurrency scenarios).

   Focus especially on:

   * Race conditions and concurrent access patterns.
   * Network partitions, timeouts, and retries.
   * Large or malformed inputs (fuzzing candidates).

5. **Consistency over cleverness.**

   * Prefer simple, clear patterns that match the rest of the codebase.
   * If you introduce a new pattern (e.g. for retries, validation, logging), make it reusable and documented.

---

## Review Mode (Security & Quality Review)

When explicitly asked to **review** code (or when running in review mode), perform a **security-first, correctness-first review**, *not* a high-level or marketing-style summary.

### Scope of the review

Look for concrete technical issues, including but not limited to:

* **Resource management**

  * Leaked sockets, tasks, goroutines/threads, memory, file descriptors, channels, timers.
* **Denial-of-service vectors**

  * Unbounded queues or maps, missing limits, unbounded retries, OOM risks, pathological worst-case behavior.
* **Concurrency and memory safety**

  * Data races, lock misuse, read-modify-write hazards, non-atomic access to shared state, unsafe async patterns.
* **Cryptography, authentication, identity**

  * Certificate verification, signature schemes, key handling, identity checks, missing revocation/pinning, weak randomness.
* **Networking behavior**

  * Fragile or incorrect logic in hole punching, relay, timeouts, backoff, ICE/ICE-lite behavior, retry loops, error handling.
* **Input and data validation**

  * Unvalidated lengths, missing bounds checks, unchecked parsing, unchecked external input (network, disk, RPC, env).

### For every issue found

For each distinct issue, output a structured entry with:

* **Severity:** `Critical`, `High`, `Medium`, or `Low`
* **Title:** Short, specific name
* **Location:** File and approximate line(s) (e.g. `net.rs:1018`)
* **Snippet:** Minimal relevant code excerpt
* **Problem:** Clear, technical description of what is wrong
* **Impact:** What can realistically go wrong (security, reliability, performance, correctness)
* **Fix / Recommendation:** Concrete, actionable guidance. If an obvious fix exists, state it explicitly.

Avoid vague advice. Be precise about **what to change** and **why**.

---

## Detect and Call Out “Vibe Coding”

Actively look for signs of rushed or AI-style code, especially in critical paths. Treat them as issues **when they have realistic risk**:

* `TODO` / “in production you’d…” comments in non-test or hot paths.
* Inconsistent validation, checks, or error handling between similar code paths.
* Magic numbers or unexplained constants, especially for timeouts, sizes, and limits.
* Suppressed warnings or lints on important code
  (e.g. `#[allow(dead_code)]`, `#[allow(unused)]`, `// nolint`, `// ignore`).
* Copy–paste variants where one version is safe and another is not
  (e.g. one validates size/auth, the other doesn’t).

If these indicate potential security, correctness, or maintainability problems, log them with severity and a concrete recommendation.

---

## Areas Requiring Extra Scrutiny

Always pay **special attention** to:

* **Path probing / UDP usage**

  * Socket and buffer lifetimes, resource leaks, unbounded retries, missing timeouts.
* **RPC handling**

  * Response size validation on all code paths, per-request and global limits, behavior on partial reads, timeouts, cancellation.
* **Hole punching / rendezvous logic**

  * Clock and time assumptions, start times, race windows, stale entries in registries/maps, cleanup of failed attempts.
* **PubSub**

  * Queue growth, backpressure, rate limiting, DHT-based subscription announcements, cache eviction and poisoning.
* **Relay / ICE-lite behavior**

  * Priority computations, tie-breaking rules, session lifecycle and cleanup, metrics correctness.
* **Identity / endpoint records**

  * How signatures are constructed and verified, ambiguity or confusion risks, identity spoofing and replay.
* **Connection caches / long-lived maps**

  * Unbounded growth, eviction policies, idle/failed connection cleanup, memory pressure and DoS vectors.

Fragile-but-currently-safe patterns should still be called out as **Medium or Low** with clear hardening suggestions.

---

## Required Review Output Structure

When producing a **formal review**, structure the output as follows:

1. **Title and Executive Summary**

   * 1–2 paragraphs summarizing overall risk posture and key findings.

2. **Severity Breakdown**

   * Counts of issues by severity, e.g.
     `Critical: N`
     `High: N`
     `Medium: N`
     `Low: N`

3. **Issues by Severity**

   * Sections: `Critical`, `High`, `Medium`, `Low`.
   * Under each, list issues using the structured format:

     * Severity
     * Title
     * Location (file + approximate lines)
     * Snippet
     * Problem
     * Impact
     * Fix / Recommendation

4. **Signs of “Vibe Coding”**

   * A short, explicit section summarizing:

     * Any vibe-coding indicators found.
     * Why they are risky or unacceptable in this codebase.

5. **Recommendations**

   * **Immediate (before production):**
     Must-fix items that block safe deployment.
   * **Short-term (1–2 weeks):**
     Targeted refactors, tests, and hardening steps.
   * **Long-term:**
     Architectural improvements, systemic testing strategies, tooling, observability.

---

## Make Findings Specific and Testable

* Always refer to **actual functions, structs/types, modules, files, and line ranges** when possible.
* When suggesting tests, be explicit about:

  * Which **behaviors** are currently untested.
  * Which **edge cases** and failure modes to cover
    (race conditions, network partitions, retry exhaustion, large/malformed inputs, clock skew, etc.).

Call out **subtle but realistic** attack and failure scenarios, such as:

* OOM via large length prefixes, unbounded queues, or unbounded maps.
* Sybil or recon attacks via weak or disabled certificate/identity verification.
* Clock skew or timing issues causing hole punching, rendezvous, or session logic to fail or become exploitable.

---

**Bottom line:**
All generated or modified code in this repository must be **secure, robust, and maintainable**.
No vibe coding, no silent footguns, no unbounded or unvalidated behavior.
