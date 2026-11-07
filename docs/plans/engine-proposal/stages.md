# Staged implementation proposal

**Unadopted dependency order.** The stages below condense the October 2026 proposal. They are not active Chuzz tasks, approved release gates, or evidence that code exists. The original benchmark/corpus matrix is deliberately omitted.

## Before implementation

The owner first decides whether this proposal is still wanted, where the engine lives, who owns the host/runtime/code-generator boundaries, and which platforms and language features are mandatory. Recheck the current source trees, licenses, published interfaces, feature closures, and toolchain support. Establish one semantic acceptance contract and local validation commands before any runtime work. Keep engine activation opt-in until its owner approves the applicable gates.

## Proposed order

| Stage | Focus | Required evidence before moving forward |
|---|---|---|
| A | Scope and dependency feasibility | Current ownership map, dependency and feature audit, supported platform list, and a buildable Rust baseline. |
| B | Minimum runtime slice | Owned artifacts, isolate and roots, basic heap and interpreter, shared IR boundary, one original machine-code path, and safe code publication/reclamation. |
| C | Frontend ownership | Stable source identity, owned syntax/HIR, early errors, lossless source positions, and no parser recovery execution. |
| D | JavaScript execution | Calls, closures, object and array behavior, module operations, abrupt completion, and explicit root coverage. |
| E | Policy and inspection | Pre-effect checks, deny and warn behavior, bounded event delivery, debugger snapshots, and no hidden effectful fallback. |
| F | Browser host integration | Module and host calls, timers/messages, rendering completion, permissions, and explicit host-to-engine event delivery. |
| G | Fast compilation | Profitability-based admission, direct calls, guarded property access, source-generation checks, transactional OSR, and effect-safe deoptimization. |
| H | TypeScript execution | Type provenance through lowering, checking before checked builds/AOT, and JavaScript-correct runtime behavior despite annotations. |
| I | WebAssembly | Validation, typed execution, trap separation, and integration with the shared backend. |
| J | Optional optimization | Bounded inlining, scalar replacement, and specialization only where each pass has independent evidence and can be disabled. |
| K | Mature heap evolution | Selective evacuation and survival-guided policy only after root, lifetime, and memory accounting contracts are established. |
| L | Second machine target | ABI, unwind, root-map, executable publication, and reclamation evidence on the additional target. |
| M | Product rollout | Browser routes and host behavior pass the owner-approved acceptance contract. Keep the existing runtime as default until the owner closes all required correctness and performance gates. |

Stages with independent ownership may proceed in parallel after their stated prerequisites, but no stage inherits another's pass. A failed invariant closes only dependent work; it is not waived by aggregate performance or by an implementation being merged.

## Stop conditions

Stop the affected stage on any of the following:

- A parser, type annotation, or compiler path changes language-visible behavior.
- A guest root is missed, a movable pointer crosses an unsafe boundary, or an object is reused while an execution frame can still reference it.
- A stale compilation can publish after source, policy, or dependency generation changed.
- An effect can commit before policy approval or after a denial.
- Work waits through polling, spinning, or repeated no-progress reclamation instead of a real event.
- A platform writes or executes code outside its approved publication protocol.
- A dependency introduces an unapproved executor, runtime, collector, native library, feature, or license obligation.

Record the exact source identity, failure, owner, and next safe action. Do not convert an unverified proposal into a passing claim or silently broaden the supported feature set.

## Open owner decisions

1. Is a dedicated engine still desired, and where is its canonical repository?
2. Which JavaScript, TypeScript, WebAssembly, and browser-host behavior is mandatory for the first useful release?
3. Which frontend and Rust toolchain are supportable in the current publishing environment?
4. Which compiler backends and operating systems are in scope, and what is the safe fallback when executable-memory support is absent?
5. Which event, policy, and host APIs can the current browser stack provide without polling?
6. What correctness and performance acceptance contract is current? The historical measurement and corpus specification is not preserved here and must be re-established by the owner before implementation.
