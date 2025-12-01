# AGENTS.md

## Purpose
These are the instructions for AI coding agents working in this repository.

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

## Implementation & Documentation Requirements

For **every implementation change**, or when adding or modifying tests, the agent **must** ensure that the inline documentation and relevant specs stay consistent. 

---

## When Unsure

If the documentation is ambiguous or missing details, the agent should:

1. Prefer minimal, conservative changes that do not introduce new assumptions.
2. Re-read the relevant specs and README to infer intent whenever possible.
3. If intent is still unclear:
   - Document the ambiguity briefly in comments and/or spec (e.g., `TODO: clarify behavior of X in scenario Y`).
   - Avoid introducing behavior that conflicts with existing documented or implied design.