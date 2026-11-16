# Architecture and invariants

**Proposal only.** These are requirements to assess, not claims about current Chuzz or runtime code.

## Product boundary

The proposal is for a Rust implementation that can execute JavaScript, execute runtime-bearing TypeScript without emitting JavaScript text and reparsing it, and validate and execute WebAssembly through a shared compiler backend. A browser host remains responsible for navigation, permissions, persistence, downloads, document integration, rendering, and user-facing policy. The engine owns parsing adapters, checking inputs, runtime state, compilation, guest memory, host-call contracts, and execution policy hooks.

Keep the host and engine independently reviewable. The host selects and configures the runtime; page content cannot select an unvalidated fallback engine. Unsupported language features produce explicit errors. Existing interpreters can serve as semantic references while the new implementation is compared, but they are not a silent production fallback.

## Component boundaries

The proposed components are:

- **Frontend adapter:** a maintained Rust JavaScript/TypeScript parser family, compiler-mode early errors, source spans, immutable syntax ownership, and lowering into engine-owned HIR. Do not retain an arena pointer without its owner, publish borrowed parser nodes across threads without a proven lifetime contract, or use a stock transform that emits JavaScript as the production execution path.
- **Runtime:** an engine-owned interpreter, bytecode format, module and scope handling, object model, isolate, and guest heap.
- **Compiler:** an engine-owned typed CFG/SSA intermediate form, followed by machine IR, original Rust emitters, object sinks, and immutable compilation metadata. Rust MIR and WebAssembly IR enter through explicit semantic adapters rather than being serialized through guest memory.
- **Host integration:** typed calls between the engine and browser services. Mutable guest state stays inside its isolate; host callbacks retain generation-checked handles instead of movable object pointers.
- **Policy and observability:** sensitive effects pass through a policy check before commitment. Bounded events, debugger snapshots, and audit records refer to stable identities and never expose raw heap addresses or invoke guest getters.

Specific crate versions and features from the old proposal are not approved by this record. Any reused parser, allocator, scheduler, table, reclamation, or Wasm library needs a current source and feature-closure review before admission.

## Immutable artifact ownership

The proposed pipeline carries owned immutable artifacts through source, tokens, syntax, HIR, bytecode, compiler IR, machine code, and debug/deoptimization metadata. Each artifact records source generation, schema identity, and its storage owner. Shared ownership may clone an `Arc`; it must not deep-copy a payload just to hand it to another stage. A representation change may allocate the new representation while keeping the original owner alive as needed.

Builders stay uniquely owned until publication. Published buffers expose no mutation. Source spans reference the source owner; syntax and compiler records retain the owners needed by later phases. Compiled code, root maps, relocation data, deoptimization maps, and unwind metadata publish as one immutable artifact.

Count only copies with a defined semantic or platform reason, such as decoding, requested string conversion, structured cloning, moving collection, IPC, persistence, or executable-memory transfer. Queueing, snapshots, and stage handoffs are not reasons to serialize or duplicate the entire payload.

## Isolates, values, and objects

One isolate owns its realms, heap, roots, activations, feedback, modules, and microtasks. One mutator executes an isolate at a time. Guest pointers do not cross isolates. Immutable syntax and compiler artifacts can cross a worker boundary only when their Rust types establish the required thread-safety bounds. Isolate-local shapes and constants cross through isolate-local identities and traced roots.

The proposed value representation uses tagged values with explicit distinctions for numbers, booleans, nullish values, objects, strings, symbols, and big integers. Object layouts use immutable shapes and authoritative slots; fast property access checks object kind and shape before reading the slot. Result representation is guarded separately. Array holes remain distinct from `undefined`.

The proposal treats address-envelope verification and value representation as an initialization decision. Verify the heap representation before guest allocation or effects begin; fail closed on a violation rather than restarting or changing representation after guest execution has started. Guest handles retained by host code are generation checked, and persistent guest handles are not thread-safe by default.

Synchronous no-GC borrows cannot allocate, relocate, detach backing, perform conflicting mutation, or re-enter guest code. Suspended work retains rooted state and backing owners instead of carrying a movable pointer across suspension.

## Language and compiler contracts

JavaScript and TypeScript share JavaScript runtime semantics. TypeScript annotations provide checker facts and provenance, not permission to erase runtime guards. For example, a parameter annotated `number` does not make every call's argument an unboxed integer, and `readonly` does not freeze an object. Native scalar widths, layouts, immutability, and exclusive ownership require separate checked contracts.

Bytecode is the interpreter and deoptimization representation. Rust keeps its source-level semantics until an explicit adapter lowers the supported subset. WebAssembly keeps typed module semantics until validation has succeeded and an explicit adapter produces the shared compiler representation. Traps, JavaScript throws, speculative deoptimization, Rust failures, and policy denials remain distinct completion paths.

The compiler proposal has an interpreter-only policy and optional fast and peak tiers. Admission depends on runtime evidence, expected remaining work, compile and queue cost, and resource limits. Compilation requests are deduplicated and event driven. No background scan searches for hot functions. Every compiled entry is tied to source and dependency generations; stale results are discarded before publication.

Published code is immutable. Recompilation creates a new artifact and retargets an entry cell; it does not patch code that may be executing. Deoptimization reconstructs interpreter frames from retained metadata without repeating committed effects. OSR imports an interpreter frame transactionally, leaving the original frame intact if import fails.

JIT support is platform gated. The proposal excludes guest JIT on iOS and selects interpretation or Rust AOT when JIT support is absent. Executable-memory transitions, cache invalidation, relocation, metadata publication, and protection restoration need platform-specific checks before code can be enabled. AOT artifacts are checked against format, schema, target, source, dependency, and policy identities before use.

## Memory management and reclamation

The initial guest collector proposal is a moving nursery with precise roots and a non-moving mature space. A collection request is raised by allocation exhaustion, allocation debt, explicit request, or host pressure, then handled by the isolate owner at a safe point. Every root class, compiled spill, suspended job, module, and constant that can reference a guest object must participate in tracing. Derived interior pointers are recomputed after relocation.

A later mature-space design may add selective evacuation and survival-based pretenuring. That is a separate step and must not introduce a second runtime pool. Concurrent marking is outside the proposal. Finalization and weak-reference semantics require explicit language-level handling whenever enabled.

Guest tracing and native reclamation protect different lifetimes. A native code or metadata entry becomes unreachable to new lookups first, then waits for active readers and retained frames to release it, and only then becomes reusable. Retirement must make progress from publication and release events; repeated busy calls to advance reclamation are not an acceptable wait strategy. Retained code remains charged to memory limits until it is actually reclaimed.

## Scheduling and waits

The proposal assigns one explicitly configured runtime ownership of compute workers. A task carries a stable identity, generation, and reason. Immutable payloads remain in owned artifacts; mutable queues, cancellation state, counters, and publication cells remain separate control state.

Producers publish state before signaling. A consumer registers its wake, checks protected state, and parks if no work is ready. It may drain a bounded batch after a real wake and schedule a continuation when real work remains. It must not repeatedly yield, sleep, or scan the same condition to simulate progress. Timers are for actual guest timers, deadlines, or one-shot debounce, not heartbeats used to discover work.

The same event rule applies to compilation, code retirement, garbage collection requests, policy decisions, host fetches, module loads, messages, timers, rendering, and event-stream capacity. Observability reads on demand or follows producer events instead of periodically copying tables.

## Policy, host effects, and observability

Every sensitive operation must reach a policy check before the effect commits. The proposal includes both warning and blocking modes, with warning as the initial mode. Checks bind the principal, capability, site, argument identity, and policy generation. A changed argument or generation invalidates the decision. Denial resumes through a denial completion; it cannot deopt into a path that performs the blocked effect.

Policy events are bounded and sequence numbered. Overflow is visible. In blocking mode, an unresolved decision waits for a decision or a deadline; timeout denies. The watcher consumes redacted immutable evidence and does not execute page text as instructions.

Debugger and audit interfaces use rooted snapshots and retained artifact IDs. They do not invoke page getters or proxies and do not expose raw pointers. Incident records are generated outside the mutator path and may omit source bytes while retaining content identities.

Host completion is delivered through explicit events. A worker thread alone does not make a polling API event driven; the host contract must provide a real completion notification or an implementation that can wait without spinning.

## Dependency and platform exclusions to reassess

The source proposal preferred Rust implementations, one scheduler, one collector, one frontend, and a shared managed heap. It rejected a second executor or reclamation model, cross-heap fallback, parser caches, active handwritten assembly, foreign runtime implementations, and unreviewed native-library dependencies. It proposed macOS and Linux first, with Windows and guest JIT on iOS excluded.

These are design constraints to review, not current dependency findings. Recheck each exclusion against current Chuzz requirements and the selected crates before adopting the proposal. Existing Chuzz house rules take precedence when a historical proposal conflicts with them.
