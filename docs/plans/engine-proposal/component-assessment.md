# Candidate component boundaries

**Unadopted.** Names below are candidates from the October design, not dependency approvals or a current supply-chain audit. Recheck the source revision, license, feature graph, native/build dependencies, maintenance status, and Rust toolchain before use.

| Area | Proposal direction | Boundary and unresolved check |
|---|---|---|
| JavaScript/TypeScript frontend | Evaluate the maintained Oxc parser family. | Prove immutable AST ownership, spans, early errors, lossless source handling, and thread bounds. A parser choice does not authorize emitting JS and reparsing it at runtime. |
| Existing JavaScript implementation | Keep an existing interpreter such as Boa as a semantic reference during development. | Do not make it a transparent production fallback or assume its garbage collector and embedding ownership fit a moving heap. Reuse of algorithms is separate from copying implementation source. |
| WebAssembly | Evaluate `wasmparser` for decoding and validation. | Validate the complete module before instantiation. The decoder does not supply the runtime, instance model, memory safety, traps, host bindings, or compiler. |
| Register allocation | Start with a small bounded allocator or existing backend interface; evaluate `regalloc2` only against that boundary. | Avoid duplicating allocator crates and account for compile cost and dependency closure. |
| Number and text operations | Evaluate Rust libraries for ECMAScript number conversion, UTF-8 validation, regular expressions, and locale data. | Keep language grammar, UTF-16 behavior, interruption, `lastIndex`, conversions, and locale semantics in the engine contract. An implementation library is not a substitute for the required semantics. |
| Scheduling and lifecycle | Prefer one reviewed Rust scheduler, table lifecycle, allocator, and native reclamation path. | Do not add a second executor, collector, or reclamation system. Verify the actual API and selected features instead of treating a dependency name as proof. |
| Compiler backend | Keep a Rust-owned typed IR and original Rust emitters as the proposed destination. | External compiler frameworks may serve as bounded comparators or research references, but the proposal did not adopt them as the final backend. Audit any future exception explicitly. |
| Rendering and browser host | Reuse the current browser's admitted Rust host interfaces where their event and ownership contracts fit. | A host library that polls internally, adds an unapproved native implementation, or owns a second executor does not satisfy the event-driven contract merely because it is written in Rust. |

The source designs explicitly rejected treating a large runtime, a foreign implementation, an alternate garbage collector, another asynchronous runtime, or an automatically admitted native library as a shortcut to completeness. The exact exclusions must be reconsidered against current source and product scope; they are not dependency findings for Chuzz today.

Reuse is governed by license as well as behavior. For restricted-license source, use the applicable specification and independently written requirements; never copy code. Preserve required notices for any code that is lawfully reused.
