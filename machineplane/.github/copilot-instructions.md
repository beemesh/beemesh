# Beemesh

Beemesh is a decentralized, lock-free orchestration system that turns any device — cloud, on-prem, edge, or IoT — into an interchangeable compute resource. It scales out by eliminating the centralized control plane, enabling secure, self-healing workloads across highly dynamic environments. Beemesh eventually allows to run a part of kubernetes workloads / manifests with a compatibility layer. The workload plane is strictly focusing on scheduling and running workloads, it does not contain any service mesh like behavior, the service mesh is implemented separate in the workload plane.

## Principles

* The solution prioritizes decentralization through libp2p and zero-trust by encrypting and signing all communication.
* All content must always be encrypted with post-quantum algorithms and messages must be signed end-to-end
* The machine plane listen should check for node failure and restart crashed containers and reschedule workloads on failed nodes, but does not handle any workload communication as this is handled by the machine plane (strict segregation and zero trust, the workload plane does not trust the machine plane).

## Code Layout
The machineplane consists of the following crates:
* cli: consists of the beectl cli similar to kubectl that allows to interact with beemesh
* crypto: the common library that offers helper functions for the cli and machine
* machine: containts the libp2p machine implementation offering scheduling and managing lifecycle of the workload, similar to kubelet
* protocol: contains the flatbuffers that are used for messaging between the nodes in the machine plane

## Workflow
1. Fetch any URL's provided by the user using the fetch tool. Recursively follow links to gather all relevant context.
2. For Rust crates, always use the latest version and check the crate docs for implementation details on how to use
3. Understand the problem deeply. Carefully read the issue and think critically about what is required. Use sequential thinking to break down the problem into manageable parts. Consider the following:
   - What is the expected behavior?
   - What are the edge cases?
   - What are the potential pitfalls?
   - How does this fit into the larger context of the codebase?
   - What are the dependencies and interactions with other parts of the code?
4. Investigate the codebase. Explore relevant files, search for key functions, and gather context.
5. Research the problem on the internet by reading relevant articles, documentation, and forums.
6. Develop a clear, step-by-step plan. Break down the fix into manageable, incremental steps. DO NOT DISPLAY THIS PLAN IN CHAT.
7. Implement the fix incrementally. Make small, testable code changes.
8. Debug as needed. Use debugging techniques to isolate and resolve issues.
9. Test frequently. Run tests after each change to verify correctness.
10. Iterate until the root cause is fixed and all tests pass.
11. Reflect and validate comprehensively. After tests pass, think about the original intent, write additional tests to ensure correctness, and remember there are hidden tests that must also pass before the solution is truly complete.
12. The implementation should stay as close as possible to how ipfs / ipns work and reuse concepts and implementations if possible
13. During implementation, prefer shorter files over large files and consider splitting files that are larger than 300 lines into smaller files.

## Architecture

Beemesh consists of
The complete architecture is described in the [Architecture Document](../README.md).

## Build and Run Instructions

Refer to [build instructions](../docs/build.md) for detailed build instructions.

Every time you change the code, make sure that the code compiles by running the respective commands:

### Machineplane:
```bash
cargo build
```

To run the unit tests for the API, run:

```bash
cargo test
```
