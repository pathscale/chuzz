# JavaScript, TypeScript, and WebAssembly engine proposal

**Status: unadopted design proposal.** This is a cleaned requirements record from the engine plans dated October 2026. It does not describe shipped Chuzz behavior, prove that any dependency or architecture has been admitted, or claim that correctness, security, or performance gates have passed.

The source proposal described a separate engine subsystem hosted by Chuzz, while the engine checkout and ownership were not established. This folder records the useful architecture and delivery sequence for review. It does not decide whether the work belongs in Chuzz, a separate repository, or an existing runtime. The tentative name used in private working notes is omitted from this public document.

The current Chuzz house rules remain authoritative. In particular, follow the repository's required formatting, lint, and workspace test checks; the old proposal's blanket exclusion of unit-test suites is not adopted. Recheck dependency versions, feature closures, toolchain requirements, and platform support against current sources before any implementation decision.

The design's benchmark and corpus specification is intentionally not reproduced here. This record contains no benchmark cases, corpora, results, or performance claims. Its staged plan is an architectural dependency order, not evidence that any stage is ready or complete.

## Contents

- [Architecture and invariants](architecture.md): boundaries, immutable artifacts, runtime, compiler, memory, scheduling, policy, and host integration.
- [Runtime and compiler contracts](implementation-contracts.md): representation, heap, bytecode, IR facts, deoptimization, Wasm, code publication, and source-reuse constraints.
- [Staged implementation proposal](stages.md): dependency order, deliverables, and stop conditions without the removed benchmark matrix.

## Source disposition

The October 2026 revision marked itself as replacing earlier engine-plan revisions. The CORE, FORK, and PERF documents were companion designs. Their lasting system boundaries and exclusions are summarized in the two files here; earlier conflicting choices and historical measurements are not carried forward. The source plans' benchmark/corpus section was excluded. No source implementation was changed as part of this documentation cleanup.

Before code work begins, the owner must choose the project boundary and repository, assign the engine and host responsibilities, and approve a current dependency and platform baseline. Until then, every decision in these files remains a proposal.
