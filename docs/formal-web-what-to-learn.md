# formal-web: a third Blitz browser, and the one doing something we are not

Written 2026-08-11 from reading [gterzian/formal-web](https://github.com/gterzian/formal-web)
at the state pushed 2026-08-10. MIT, 17 stars, created 2026-03-19, actively developed,
macOS-focused, Rust pinned to 1.94.0. **Nothing here was built or run.**

## It is the same stack

The finding that makes the rest worth reading. `content/Cargo.toml` takes
`blitz-dom`, `blitz-paint`, `blitz-traits`, `stylo`, `html5ever` and `anyrender`, exactly
as we do. It is not an alternative engine to evaluate; it is a peer using our engine, and
so everything it does differently is a decision we could make.

That makes three Blitz browsers now, on three forks:

| | Chuzz | formal-web | [Aurora](small-things-to-learn-from-aurora.md) |
|---|---|---|---|
| Blitz | `pathscale/ps-blitz` | `gterzian/blitz` @ `954b41f` | upstream |
| Processes | one | main, content, graphics, net | one |
| JavaScript | Boa only | **V8 default**, Boa, JSC, behind one trait |  V8 |
| Verification | none | **TLA+ specs, checked against real traces** | none |
| Test corpus | none | **WPT vendored** | polyfills |
| Renderer | anyrender 0.12 / vello | anyrender 0.10, zero-copy IOSurface or CPU readback | anyrender |

Sizes, from the git tree: `content` 1.7 MB of Rust, `js_engine` 736 KB, `user_agent`
252 KB, `embedder` 164 KB, `graphics` 139 KB, `ipc` 75 KB, `media` 31 KB, `net` 7 KB.

## 1. It formalises the problem I hand-fixed today, and states it better

`verification/tla_specs/RenderingOpportunity.tla`, 11.5 KB, is a model of frame
production. Its variables are the frame tree (`live`, `parent`), the work in flight
(`pending`, `rendering_updated`, `composed`), batched opportunities (`op_count`), and the
two demand signals: `animating` and `frame_needed`. Its invariants:

- **`PendingLeadsRendering`**, a queued update is completed by content before graphics
  computes it.
- **`RenderingLeadsComposed`**, rendering precedes composition.
- **`DoubleBufferBound`**, the pipeline never holds more than `BufferCount` updates
  queued but not yet consumed by the embedder's paint.
- **`OpportunitiesServiced`**, batched opportunities drain when a frame is needed.

Compare what we shipped this afternoon ([animation-gap.md](animation-gap.md) item 1): a
fixed 33ms deadline, so animation-only frames run at 30fps instead of 120. It works, it is
measured, and it is a **time-driven** clamp standing in for a **demand-driven** one. Their
model says the real rule is that a render starts when the embedder needs a frame *and* an
opportunity exists, with a bound on how many can be in flight. Ours has no notion of how
many frames are outstanding at all.

Two things follow. Their formulation is the one to move toward when the paint side grows a
compositor, and it composes with the damage-region work rather than fighting it. And the
bug that cost me a measurement today, a cadence of 24fps where 30 was asked for because
the deadline ran from frame end rather than frame start, is the kind of thing a model with
an explicit clock does not let you write.

## 2. The JavaScript engine is a feature flag, not a rewrite

`content/Cargo.toml` declares `boa`, `jsc` and `v8` features, with `v8` the default, and a
build-time assertion that exactly one is enabled. The seam is the `js_engine` crate plus a
`js_engine_macros` proc-macro crate. By size: the shared surface is `src/engine.rs` at
53 KB and `src/gc.rs` at 31 KB; the backends are `v8/engine.rs` 223 KB, `jsc/engine.rs`
210 KB, `boa/engine.rs` 109 KB.

Our open research item says Boa has no rope strings, that Brimstone has cons strings but is
unpublished and self-describes as not production ready, and that "the bindings, not the
engine, are the project" because `blitz-script` is 6,659 lines written against Boa's API.
**That is the same conclusion they reached, and they paid the cost.** An abstraction turns
"should we leave Boa" from a rewrite into an experiment.

The caveat is not small: `AGENTS.md` says Chuzz is a pure Rust browser and must not add
WebKit or Chromium. V8 is Chromium's engine and JSC is WebKit's, so two of their three
backends are out of bounds here regardless of the seam's merit. What the seam buys us is
the ability to put Brimstone, or anything else, behind the same trait when it is ready,
and to keep Boa working while doing it.

## 3. Trace validation, which is cheaper than it sounds

The technique, from `verification/README.md`, is not "write a model and hope the code
matches". Rust code emits events with `verification::tla_log!(tracer, -> "{Name}",
"Event", args...)`; the main process runs a trace monitor and hands `TraceSender` clones to
the content and network processes; events land as NDJSON. `verify-specs.sh` starts the
embedder headless, drives it through a minimal WebDriver session against a fixture page,
collects the trace, and runs TLC over it. The spec is checked against **what the
implementation actually did**.

Two of their specs are verified this way today: Navigation and RenderingOpportunity. Three
more exist for `MessagePort` (`MessagePort.tla`, `MessagePortFG.tla` at 5 KB, and
`MessagePortExtraFG.tla` at 14 KB), which is the algorithm nobody gets right by reading the
spec prose.

**Their `Navigation.tla` does not fit our navigation, and it is worth saying why**, because
the obvious move is to grab it. Its variables are `navigables`, `navigations`,
`navigationStartQueue`; its actions are `CreateChildNavigable`, `RunBeforeUnload`,
`ContinueNavigation`; its one invariant is `FinalizedImpliesAllApproved`, that a finalized
navigation was approved by every navigable that ran `beforeunload`. That is the WHATWG
navigation lifecycle across a navigable tree. Chuzz has no `beforeunload`, no navigable
tree, and `apps/chuzz/src/history.rs` is a back and forward stack. The file models
behaviour we do not implement, so importing it would assert nothing.

## 4. They vendored WPT

`vendor/wpt/` accounts for most of the repository's 154 MB. Our own
[TODO.md](TODO.md) says the gap is "the corpus and the comparison, not the renderer", and
that for a program whose input is the entire web, unit tests answer a much narrower
question than "does this page still render". They solved the corpus half by taking the
obvious corpus.

We now have the comparison half: `--capture` renders headlessly, `CHUZZ_CAPTURE_TREE`
writes the laid-out tree with a census. What is missing is exactly what they vendored.

## What not to take

- **The multi-process split.** Real, and it is prior art that it can be done on Blitz,
  which is worth knowing given `agencyzero/docs/xpc-sidecar.md` designs the same thing and
  `GPUI-and-zng-what-we-should-learn.md` section 4 argues for it. But it is an
  architecture, not a patch, and nothing we are blocked on needs it.
- **Their Blitz fork.** A fourth divergence is the last thing this ecosystem needs. We
  already spent a morning reconciling one.
- **Their anyrender.** 0.10 against our 0.12; we are ahead.

## What to import, specifically

MIT licensed, so copying is permitted with attribution. Ordered by whether it is a file, an
adaptation, or only a lesson.

### Copy, roughly as-is

| # | What | From | Size | Lands in | Buys |
|---|---|---|---|---|---|
| 1 | The `tla_log!` macro and the tracer | `verification/src/lib.rs`, `tracer.rs` | 1.4 KB + 4.5 KB | a new `crates/chuzz-trace` | Emitting an ordered event trace from the running browser. No dependencies beyond `log`; `ipc_channel` appears only in its test module |
| 2 | The trace monitor, rewritten for one process | `verification/src/monitor.rs`, `types.rs` | 5.6 KB + 0.6 KB | same crate | Writing the trace as NDJSON. **`TraceSender` wraps an `IpcSender` because they are multi-process; ours is an `mpsc::Sender` or a file.** Rewrite, do not copy |
| 3 | The TLC driver | `verification/src/validate.rs` | 45 KB | same crate | Rendering trace data into TLA+ modules and running TLC over them. The one genuinely large piece of work already done, and the one nobody would write for fun |

Items 1 to 3 are worth nothing on their own. **They are only worth importing together with
a decision to write at least one spec**, and the specs they have do not fit us, see below.

### Adapt the invariants, not the files

| # | What | Why not the file |
|---|---|---|
| 4 | `DoubleBufferBound` and the `animating` versus `frame_needed` split, from `RenderingOpportunity.tla` | Their model names a graphics process and a compositor we do not have. What transfers is the shape: a bound on renders in flight, which we have no notion of at all, and the distinction between "the page wants a frame" and "the embedder needs one", which our fixed 33ms interval collapses |

### Take the decision, not the code

| # | What | Note |
|---|---|---|
| 5 | Vendor WPT | Nothing to import from them: the corpus is `web-platform-tests/wpt` upstream. What they demonstrate is that it is tractable on a Blitz engine, and the wiring is a runner |
| 6 | Drive the browser headlessly and assert on the result | They use WebDriver plus `session.rs`. **We already have the parts**: `crates/chuzz-control` speaks Inspect and Act over a socket, `--capture` renders headlessly, and `CHUZZ_CAPTURE_TREE` writes the laid-out tree with a census. What is missing is a harness that uses them, not a capability |

### Do not import

| What | Why |
|---|---|
| `Navigation.tla`, `NavigationTrace.tla` | Models `beforeunload` across a navigable tree. We have neither |
| The three `MessagePort` specs | We have no `MessagePort`. Revisit if we ever do; they are the best reason to |
| The `js_engine` trait | 53 KB of shared surface shaped around V8 and JSC lifetimes and GC, and `AGENTS.md` rules both out. Carrying an abstraction designed for engines we cannot use, to hold Boa and one hypothetical successor, is the wrong trade |
| The multi-process split | An architecture, not a patch |
| Their Blitz fork, their anyrender 0.10 | We are ahead on one and reconciling forks cost us a morning on the other |

## Honest limits

Five months old, 17 stars, one author, macOS only, toolchain pinned. Two specs verified,
not twenty. The TLA+ work depends on a locally installed TLA+ Toolbox jar at a hardcoded
path. None of this is a library we can depend on; it is a set of ideas with a working
demonstration attached, which is the most useful kind of prior art and the least
importable.

## Related

- [small-things-to-learn-from-aurora.md](small-things-to-learn-from-aurora.md), the other
  Blitz browser, reviewed the same way.
- [animation-gap.md](animation-gap.md), the frame pacing their RenderingOpportunity spec
  models.
- [TODO.md](TODO.md), whose fixture-corpus and JavaScript-engine items this bears on
  directly.
