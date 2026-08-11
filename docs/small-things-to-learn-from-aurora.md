# Small things worth taking from Aurora

Written 2026-08-11. Aurora references are pinned to commit
[`f44a05a`](https://github.com/JohannaWeb/Aurora/tree/f44a05a56ec25a9ad9c7f4e6255fa9455ea7f40e),
read that day, so the line numbers are stable. Chuzz references are local reads.
**Nothing here was measured or built.**

[Aurora](https://github.com/JohannaWeb/Aurora) is another Rust browser engine, and despite
the "Not Servo" tagline it runs the same stack Chuzz does: blitz-dom, blitz-html,
blitz-paint, Stylo, Taffy, Vello on wgpu. That is what makes it worth reading. The
differences are not architecture-shaped noise; they are decisions made against the same
constraints.

This document is deliberately narrow: four small, self-contained things. It is not a
general comparison of the two projects.

## 1. An event-loop-aware JS runtime trait

**Theirs.** [`src/js_engine.rs#L66-L124`](https://github.com/JohannaWeb/Aurora/blob/f44a05a56ec25a9ad9c7f4e6255fa9455ea7f40e/src/js_engine.rs#L66)
defines a `JsRuntime` trait that the whole engine talks to, with an `EngineKind` factory
above it ([#L38](https://github.com/JohannaWeb/Aurora/blob/f44a05a56ec25a9ad9c7f4e6255fa9455ea7f40e/src/js_engine.rs#L38))
so no call site names the concrete runtime.

The shape of the trait is the interesting part. It is written for an event loop that wants
to know what to do next:

```rust
fn next_deadline(&self) -> Option<Instant>;
fn has_ready_work(&self, now: Instant) -> bool;
fn has_animation_frame_callbacks(&self) -> bool;
fn drain_animation_frame_callbacks(&mut self, now: Instant) -> bool;
fn deliver_mutation_records(&mut self) -> bool;
fn take_needs_reflow(&mut self) -> bool;
fn clear_dirty_bits(&mut self);
fn has_dirty_bits(&self) -> bool;
fn tick(&mut self, now: Instant) -> bool;
```

Two properties worth copying:

- **Every pumping method returns whether work actually happened**, so the loop knows when
  to stop rather than polling a fixed number of times.
- **The runtime can say when it next needs to be woken** (`next_deadline`), so a timer
  does not require spinning.

**Ours.** Chuzz drives `blitz_script::ScriptDocument` directly
([document_loader.rs:374](../apps/chuzz/src/document_loader.rs), and the boxed variant at
:544). The engine's own pump is `ScriptDocument::poll`, which does not expose a deadline or
a per-source "did anything happen" answer, and the surrounding code has no seam that would
let a different runtime be dropped in.

**What to do.** Nothing urgent, and this is not an argument for swapping engines. But if a
JS seam is ever introduced here, this is the shape to introduce, because it is the one that
lets the event loop idle correctly. Adding it later, once call sites name a concrete
runtime, is the expensive order.

## 2. Capability-gated fetch

**Theirs.** [`src/fetch/capability.rs`](https://github.com/JohannaWeb/Aurora/blob/f44a05a56ec25a9ad9c7f4e6255fa9455ea7f40e/src/fetch/capability.rs),
31 lines in total. Every fetch asks an `Identity` whether it holds the relevant capability
before touching the network or the filesystem:

```rust
pub(super) fn require_network_access(identity: &Identity) -> Result<(), FetchError> {
    if identity.default_capabilities.contains(&Capability::NetworkAccess) {
        Ok(())
    } else {
        Err(FetchError::InvalidUrl(format!(
            "Identity {} lacks network.access capability", identity.did)))
    }
}
```

The gate lives at the fetch layer rather than at call sites, so there is one place to audit
and no path that forgets to check.

**Ours.** [`AGENTS.md`](../AGENTS.md) already claims this ground: "The browser app owns
tabs, navigation, history, **permissions**, persistence, and downloads." Today there is no
permission concept in the loader; [document_loader.rs](../apps/chuzz/src/document_loader.rs)
fetches what it is asked to fetch.

**What to do.** When permissions arrive, put the check at the fetch boundary the way Aurora
does, not at the call sites. Their file is worth reading first purely because it is short
enough to read in one sitting and shows the seam clearly.

## 3. Per-document health, not just a panic guard

**Theirs.** Their Blitz document carries `healthy: bool` and `consecutive_panics: u32`
alongside the document itself
([`src/blitz_document.rs#L222-L230`](https://github.com/JohannaWeb/Aurora/blob/f44a05a56ec25a9ad9c7f4e6255fa9455ea7f40e/src/blitz_document.rs#L222)),
so a document that keeps failing is known to be failing rather than being retried forever.

**Ours.** A tab owns its document ([tab.rs](../apps/chuzz/src/tab.rs)) and has no health
concept.

**What to do.** This is the natural companion to the Stylo panic guard, which is being
fixed separately. A guard that catches a panic answers "did this frame fail". A counter
beside the document answers "is this page hopeless", which is what decides whether to show
an error page instead of repainting a broken document forever. Worth adding in the same
change as the guard, while the failure paths are already open.

## 4. Fixtures and a test-suite harness

**Theirs.** The repository root carries `fixtures/`, `tests/`, `test_suite_analysis/`, a
`Makefile` and a `Dockerfile`, plus a `.devcontainer`. Whatever the quality of the contents,
the shape says a browser is being tested against a corpus of real pages rather than against
its own unit tests.

**Ours.** [`AGENTS.md`](../AGENTS.md) mandates `cargo fmt`, `cargo clippy` and
`cargo test --workspace --all-features` before delivery, which is good discipline, but there
is no fixture corpus in the tree. For a program whose input is the entire web, unit tests
answer a much narrower question than "does this page still render".

**What to do.** A small committed corpus of saved pages, rendered headlessly and compared,
would use machinery that already exists: `capture.rs` renders to a buffer with the CPU
rasteriser precisely so a result can be inspected without a window. The gap is the corpus
and the comparison, not the renderer.

## Not carried over

Aurora runs V8 with hand-written DOM bindings, and it keeps its own DOM mirrored into a
Blitz document rather than using blitz-dom as the source of truth. Both were reviewed and
neither is proposed here. Recorded so the analysis is not repeated.
