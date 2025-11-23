# AGENTS.md

## Purpose
These are the instructions for AI coding agents working in this repository.  
Follow these rules before performing any task. For all project details, rely on the referenced documentation below.

---

## Required Reading

### Core Project Overview
- **Project README:** `README.md`  
  Contains high-level descriptions, setup instructions, development workflow, and testing commands.

### Component Specifications
- **Machineplane Specification:** `machineplane/machineplane-spec.md`  
  Defines architecture, data structures, interfaces, responsibilities, and constraints for the machineplane subsystem.

- **Workplane Specification:** `workplane/workplane-spec.md`  
  Defines architecture, workflows, data flows, interfaces, and constraints for the workplane subsystem.

Agents must refer to these specification files before making changes to their corresponding subsystems.

---

## Subsystem-Specific Guidance

### Machineplane
- Follow the rules and structures defined in `machineplane-spec.md`.
- Maintain compatibility with the defined interfaces and data models.

### Workplane
- Follow the workflows and logic defined in `workplane-spec.md`.
- Avoid creating new abstractions without referring to the spec.

---

## When Unsure
If the documentation is ambiguous or missing details, the agent should:

1. Prefer minimal changes.

---

End of AGENTS.md
