# TODO: what to learn from fastrender

Written 2026-08-11. A file-level review of
[wilsonzlin/fastrender](https://github.com/wilsonzlin/fastrender) against our tree, and
what to do about it. Read via the GitHub API; **nothing was cloned** — the repo is ~830MB.

## The licensing gate, first

No `LICENSE` file, no `license` field in `Cargo.toml`, `publish = false`. Absent a licence
grant the work is all-rights-reserved by default.

**We can read it. We cannot copy any of it** — not a file, not a function, not a
distinctive comment. Everything below is a design to reimplement or a gap to notice, and
every item must be written from the spec or from our own tree. Where a spec section is
what actually matters, this document cites the spec rather than their file.

If that ever changes, revisit: several pieces would be worth vendoring outright.

## Do not read the size difference as a gap

| | fastrender `src/` | ours `packages/` |
|---|---|---|
| Rust files | 2,016 | 156 |
| Bytes | 56,983,168 | 1,840,068 |

31×, and **most of it is not a deficiency**. They wrote from scratch what we delegate:

| Area | Their code | Ours |
|---|---|---|
| Cascade, computed values | `src/style/` 5.0MB, `cascade.rs` alone 1.3MB | Stylo |
| Text shaping, bidi, line breaking | `src/text/` 1.7MB | Parley |
| Flex, grid | `layout/contexts/flex.rs` 833KB, `grid.rs` 957KB | Taffy |
| Rasterisation | `paint/painter.rs` 932KB, `rasterize.rs` 84KB | vello / vello_cpu |
| JS engine | `vendor/ecma-rs` | Boa, via upstream PR #491 |

Their bet is a self-contained engine; ours is composition. Neither is parity with the
other, and rewriting Stylo to match them would be the single worst use of our time. **The
list below is only the areas where they have something we genuinely lack.**

**Paint is the exception, and the first version of this document got it wrong.** We do not
delegate paint: `blitz-paint` is ours, and it is **224KB against their 7.1MB**. Their
`paint/` is where most of the findings turned out to be, and it is the one place where
"they wrote it from scratch" describes something we also have to own. Section 0 below is
the result of going back through it.

One caveat on quality: this is a Cursor research artifact built by parallel AI agents.
`src/js/vmjs/window_realm.rs` is a **2.6MB single file**; `style/properties.rs` is 1.25MB.
`docs/conformance.md` mixes "intended conformance targets" with actual status. Read the
support matrix, not the prose, and treat file size as a warning as often as a signal.

## 0. Paint: the section the first pass missed

`blitz-paint` is 224KB of ours against 7.1MB of theirs, and it is not a crate we delegate.
Everything here is a real gap in code we own.

### 0.1 `backdrop-filter` is not blocked. [TODO.md](TODO.md) item 1 is wrong

That item calls `backdrop-filter` "the biggest single visual gap on any modern site" and
says it is **"not fixable in this repo or in `ps-blitz`. Needs a change in the anyrender
backends, which are crates.io releases."**

Both halves are wrong, and checking took two minutes:

- **We own the fork.** `~/code/ps-anyrender` is a local checkout with ten crates in it,
  and `ps-blitz` master is being pointed back at it precisely because there is work there.
- **One backend already implements it.**
  `ps-anyrender/crates/anyrender_skia/src/scene.rs:450-453` converts the filter and applies
  it with `save_layer_rec.backdrop(...)`. It works today.
- Only the vello backends drop it, and they say so in the parameter name:
  `anyrender_vello/src/scene.rs:76`, `anyrender_vello_cpu/src/scene.rs:69`,
  `anyrender_vello_hybrid/src/scene.rs:182` all take `_backdrop_filter: Option<Arc<Filter>>`
  and discard it.

Our side of the pipeline is already complete: `blitz-paint/src/filters.rs` (40 lines)
converts Stylo filters to `FilterEffect`, and `render.rs:402-432` passes both `filter` and
`backdrop_filter` into `push_layer`. Everything is plumbed up to the backend, where it is
thrown away.

fastrender is the proof it is tractable rather than the source of a fix: they implement it
themselves (`paint/svg_filter.rs` 358KB, `paint/blur.rs` 137KB) with proper backdrop-root
semantics, tested in `paint/tests/backdrop/backdrop_root_triggers_test.rs` and
`backdrop_root_will_change_test.rs`. Backdrop root is the part that is easy to get wrong:
the filter samples a defined ancestor's rasterised output, not "whatever is behind".

**Three routes, in order of appeal:** implement it in `anyrender_vello_cpu` first, since
it is CPU-side and is what headless capture uses, so the pageset scoreboard can prove it;
then `anyrender_vello`; or blur in `blitz-paint` before the backend, which duplicates work
the backends should do. Not a fastrender port at all — a correction to a note that stopped
us looking.

### 0.2 We have no display list

Theirs: `paint/display_list.rs` 109KB, `display_list_builder.rs` 835KB,
`display_list_renderer.rs` 1.1MB, `optimize.rs` 79KB with `optimize_test.rs`, and
`parallel_paint_test.rs`.

Ours: `blitz-paint` walks the DOM and pushes straight into a scene. **No intermediate
representation at all** — `grep display_list packages/*/src` returns nothing.

This is the structural gap behind an item already on our list. [TODO.md](TODO.md) carries
"Damage regions. The paint-side plan" pointing at `agencyzero/docs/partial-paint.md`, and
you cannot repaint a damaged region without a list of what was painted where. A display
list is also what makes paint parallelisable, and what lets an optimisation pass drop
occluded work — the pass that already exists on our side in spirit, as
`fbaf77345 perf: stop clipping background layers that draw nothing` in `ps-blitz-render`.

The right size for us is nothing like 835KB. It is a flat `Vec` of paint commands with
bounds, built once per frame, which is the shape `agencyzero/docs/allocations.md` wants for
per-frame scratch anyway.

### 0.3 Colour fonts

`src/text/color_fonts/` — COLR, sbix, SVG-in-OT. We have none: our only `COLR` hit is
`stylo_to_cursor_icon.rs`, which is a different thing entirely.

This is emoji. Any page with an emoji in it renders a tofu box or a monochrome outline.
Parley may cover part of it; **check before building anything**, because this is the kind
of gap that is either free or expensive with nothing in between.

## The real gaps, file by file

Verdict column: **TAKE** = build our own version, ranked below. **NOTE** = worth knowing,
not now. **SKIP** = not our architecture.

### 1. Accuracy tooling — the one that matters

Our own [blitz-fork-sweep.md](blitz-fork-sweep.md) says the gap is "the corpus and the
comparison, not the renderer". This is that, built.

| Theirs | What it does | Ours | Verdict |
|---|---|---|---|
| `src/bin/_real/fetch_pages.rs` 47KB | fetches a pageset to `fetches/html/<stem>.html` | NONE | **TAKE** |
| `scripts/chrome_baseline.sh` 30KB | Chrome render **of the same cached HTML**, JS disabled, to `fetches/chrome_renders/` | NONE | **TAKE** |
| `src/bin/render_pages.rs` 79KB | renders the pageset at a fixed viewport/DPR | `apps/chuzz/src/capture.rs` (one page, manual) | **TAKE** |
| `src/bin/diff_renders.rs` 37KB | before/after image diff → HTML report | NONE | **TAKE** |
| `src/bin/pageset_progress.rs` 454KB | scoreboard over `progress/pages/*.json`: `status`, `stages_ms`, `hotspot`, `notes` | NONE | **TAKE** |
| `scripts/chrome_vs_fastrender.sh` 14KB | one-command wrapper for the whole loop | NONE | **TAKE** |
| `src/bin/compare_diff_reports.rs` 65KB | diffs two diff reports, i.e. did this commit help | NONE | NOTE |
| `src/bin/bundle_page.rs` 228KB, `prefetch_assets.rs` 218KB | freeze a page and its subresources for offline replay | NONE | NOTE |
| `src/bin/import_wpt.rs` 92KB, `import_wpt_dom.rs` 38KB | pull WPT subsets in | upstream `wpt/runner/` (we are behind on it) | NOTE |
| `src/bin/css_coverage.rs` 23KB | which CSS properties a corpus actually uses | NONE | NOTE |

**The detail that makes it work**: the Chrome baseline is generated **from the same cached
HTML**, with JS disabled, at a pinned viewport. Baseline against a live load and you are
diffing network variance and ad rotation, not your renderer. They also keep
`scripts/verify_chrome_baseline_viewport.sh` to assert the baseline's viewport really
matches — worth copying as a discipline, not as code.

### 2. Capabilities we do not have at all

| Theirs | Ours | Verdict |
|---|---|---|
| `src/layout/fragmentation.rs` 311KB, `pagination.rs` 171KB, `layout/tests/paged_media.rs` 253KB | **NONE** — no fragmentation, no pagination, no multicol | NOTE |
| `src/interaction/hit_test.rs` 62KB, `engine.rs` 486KB, `state.rs` 63KB, `form_submit.rs` 55KB | `blitz-dom` `hit()`; no `hit_test` module, no form submission | NOTE |
| `src/sandbox/` 360KB — `linux_seccomp.rs`, `macos.rs` 58KB, `windows.rs` 88KB | **NONE** | **TAKE** (small slice) |
| `src/resource/disk_cache.rs` 368KB | **NONE** — every load is cold | **TAKE** (small slice) |
| `src/accessibility.rs` 156KB — static AOM export, ARIA roles/names/states | `accesskit_xplat` 17KB | NOTE |
| `src/ipc/` 350KB + `crates/fastrender-{ipc,shmem}` | single process | SKIP |
| `src/media/` 988KB + `crates/fastrender-{yuv,libvpx-sys-bundled}` | no video | SKIP |
| `src/webidl/generated/` 605KB, `js/webidl/bindings/generated/` 486KB — bindings generated from WHATWG IDL in `specs/` | hand-written bindings in `blitz-script/src/dom/` | NOTE, see below |
| `src/dom2/` 1.1MB — a live DOM separate from the static parse DOM | one tree serves both | NOTE |

### 2b. HTML platform features we simply do not have

Cheap to check, and all of them bite on ordinary sites rather than exotic ones.

| Theirs | Ours | Why it matters |
|---|---|---|
| `src/html/image_attrs.rs` 52KB — `srcset`/`sizes`/`<picture>` | **NONE** (`grep srcset` is empty) | Every responsive image on every modern site. We take the `src` fallback, so we fetch and paint the wrong resolution, or nothing when there is no `src` |
| `src/html/content_security_policy.rs` 46KB | **NONE** | A browser fetching subresources with no CSP enforcement at all |
| `src/html/meta_refresh.rs` 44KB | **NONE** | `<meta http-equiv="refresh">`. Old, still on landing and redirect pages |
| `src/html/image_prefetch.rs` 52KB | **NONE** | Starts image loads at parse time rather than at layout. Pairs with `pending_image_count`, which we just ported |
| `src/animation/transitions.rs` 60KB | Stylo parses `transition`; nothing drives it | CSS transitions do not animate. Adjacent to the frame pacing we just landed |
| `src/math/mod.rs` 218KB — MathML | Stylo knows the namespace, no layout | Real, and rare enough to stay unscheduled |

`srcset` is the one to take seriously. It is ubiquitous, it is parsing plus selection
rather than layout, and getting it wrong is visible on any content site.

### 3. Where they independently confirm work we just did

`src/paint/stacking.rs` (105KB) keeps `StackingContextReason::FixedPositioning` **entirely
separate** from `establishes_fixed_containing_block()`, and threads a `has_fixed_cb_ancestor`
flag down the paint walk. They never reparent a fixed node.

That is the same separation as `pathscale/ps-blitz` PR #7, arrived at independently, which
is good evidence the diagnosis was right. **Theirs is the cleaner shape**: we reparent onto
the root and then correct the offset in a second pass after layout, and that second pass
exists *only* because the hoist moved the node. If `hoist_fixed_position_nodes` is ever
revisited, thread a flag instead. They also carry
`src/paint/tests/paint/abspos_containing_block_rebase_test.rs`, same problem area.

### 4. Small things worth stealing as habits

- **UA defaults as real CSS.** `src/user_agent.css` holds element defaults as UA-origin
  rules; their Rust-side defaults are restricted to CSS initial values and genuine engine
  defaults. Ours are spread through Rust. Easier to audit against spec.
- **`scripts/check_no_conflict_markers.sh`** and `ci_check_no_merge_conflicts.sh` as CI
  gates. We shipped a conflict-marker bug into a test file today; a grep in CI would have
  caught it.
- **`scripts/run_limited.sh --as 64G`** — render under an address-space cap so a runaway
  page fails fast instead of swapping the machine. Relevant given ~344G of build artifacts
  and a browser that loads arbitrary sites.
- **Per-page `notes` in the scoreboard JSON.** Known-bad pages carry their reason inline,
  so a red cell is triaged, not rediscovered.

## What to actually do, ranked

Effort is rough and assumes no code is copied.

### 0. `backdrop-filter` in `anyrender_vello_cpu` — smallest and highest value

Not from fastrender at all; found by checking a claim in our own TODO that turned out to be
false. We own `~/code/ps-anyrender`, `anyrender_skia` already implements it at
`crates/anyrender_skia/src/scene.rs:450-453`, and the vello backends drop the argument.
`blitz-paint` already computes and passes the filter.

Do `anyrender_vello_cpu` first: it is CPU-side and is what headless capture uses, so the
scoreboard in item 1 can prove the fix. Then `anyrender_vello` for the window. Take the
backdrop-root semantics seriously — the filter samples a defined ancestor's rasterised
output, not "whatever is behind" — which is the part fastrender has two whole test files
for.

### 1. The pageset scoreboard — do this first

Everything else is easier to justify once a number moves. It would have caught today's
stacking-context bug automatically.

- `apps/chuzz/src/bin/fetch_pages.rs` — read a page list, write `fixtures/html/<stem>.html`
  plus subresources. Seed the list from the sites in `~/code/`: pathscale.com, nofilter.io,
  24x.ai, js.software, support.cafe. This is the inventory item already in
  [TODO.md](TODO.md), with a number attached.
- `apps/chuzz/src/bin/render_pages.rs` — reuse `apps/chuzz/src/capture.rs`, pinned viewport
  and `CHUZZ_CAPTURE_SCALE`, output `fixtures/renders/<stem>.png`.
- `scripts/chrome_baseline.sh` — headless Chrome over `fixtures/html/`, JS disabled, same
  viewport, output `fixtures/chrome/<stem>.png`. Assert the viewport rather than trusting
  it.
- `apps/chuzz/src/bin/diff_renders.rs` — per-pixel diff plus an HTML report. `image` and
  `png` are already workspace dependencies.
- `fixtures/progress/<stem>.json` — `status`, `stages_ms`, `hotspot`, `notes`. Feed
  `stages_ms` from `debug_timer`, which `blitz-dom` already uses for the resolve line.
- `apps/chuzz/src/bin/pageset_report.rs` — worst-N table.

**Caveat to respect**: capture goes through `vello_cpu`, and
[TODO.md](TODO.md) item 4 records that it drops 256-aligned tile columns inside text
inputs. Baseline that artefact or exclude those regions, or every page reads as broken.

### 2. Disk cache for resources

`packages/blitz-net/` is 15KB and has no cache at all, so every navigation refetches
everything, including the pageset runs above. A keyed-by-URL on-disk cache honouring
`Cache-Control`/`ETag` makes the accuracy loop fast and the browser usable offline.
Small, self-contained, no engine risk.

### 3. macOS sandbox slice

We ship a signed `Chuzz.app` that loads arbitrary sites with no sandbox at all. Their
`src/sandbox/macos.rs` is a Seatbelt profile; the concept is a few dozen lines of profile
text plus `sandbox_init`. Worth doing before the cask gets wider use, and it is the one
security gap on this list.

### 4. Generated WebIDL bindings — decide, do not drift

They generate bindings from WHATWG IDL checked into `specs/`. We hand-write every binding
in `blitz-script/src/dom/`, and today alone added `customElements` by hand and found that
`HTMLElement` is a `Symbol.hasInstance` shim with a prototype unrelated to the real DOM
prototype — exactly the class of bug generation removes.

**There is existing work, and none of it is a drop-in.** What exists, and what it is for:

| Project | What it is | Licence | Use to us |
|---|---|---|---|
| [`weedle`](https://crates.io/crates/weedle) 0.13.1, `rustwasm/weedle`, updated 2026-02 | WebIDL **parser** in Rust | MIT | **The piece to take.** Parsing IDL is the boring half and it is already solved. `wasm-bindgen/crates/webidl` depends on exactly this (`weedle = "0.13.1"`) |
| [`weedle2`](https://crates.io/crates/weedle2) 5.0.0, last updated 2024-01 | The same parser, originally by the Rust and WebAssembly Working Group, **now maintained inside [`mozilla/uniffi-rs`](https://github.com/mozilla/uniffi-rs) at `weedle2/`** | MIT, via its own `LICENSE.md` despite the MPL-2.0 parent repo | **Not for us**, and the reason is in its own README: "forked to extend the functionality beyond WebIDL needs". It serves UniFFI's UDL, which is WebIDL-*derived* rather than WebIDL. We want the real thing, and `weedle` is two years fresher |
| `wasm-bindgen`'s [`crates/webidl`](https://github.com/rustwasm/wasm-bindgen/tree/main/crates/webidl) | Generates `web-sys` from WebIDL. `first_pass.rs` 61KB builds an IR, `generator.rs` 53KB emits, `wbg_type.rs` 62KB maps types, `traverse.rs` visits | MIT/Apache-2.0 | **The architecture reference.** Real WebIDL, a JS host, a permissive licence, and it is what `weedle` is exercised against. Read for the awkward parts — overloads, partials, mixins, nullable unions, dictionaries, extended attributes |
| [`mozilla/uniffi-rs`](https://github.com/mozilla/uniffi-rs) | Mozilla's binding generator: UDL in, Kotlin/Swift/Python out, across an FFI boundary | MPL-2.0 | **A footnote.** See below — it is where `weedle2` lives, and little else applies |
| Servo `script_bindings` codegen | Generates SpiderMonkey bindings from `.webidl` | MPL-2.0 | **Read, do not use.** Python, SpiderMonkey-shaped, and file-level copyleft we do not want inside `blitz-script` |
| [`w3c/webref`](https://github.com/w3c/webref) / `@webref/idl` | Machine-readable IDL **extracted from every published spec**, continuously updated | W3C permissive | The input. Do not hand-curate `.webidl` files the way fastrender's `specs/` does |

### How much is uniffi-rs actually worth to us? Not much

An earlier draft of this document called it "the architecture reference, and the best one
here". That was wrong, and worth recording as wrong, because the reasoning that produced it
— *the parser's home is there, so the project must be the reference* — is exactly the kind
of inference that looks sound and is not.

The problem shapes barely overlap:

| | UniFFI | Us |
|---|---|---|
| Boundary | Rust → Kotlin/Swift/Python across an **FFI/ABI** | WebIDL → Boa, **same process, same memory** |
| Dialect | UDL, WebIDL-*derived* | actual WebIDL |
| Hard parts | marshalling, ownership transfer, C ABI, async across the boundary, errors mapped to foreign exceptions | overloads, partials, mixins, extended attributes, prototype chains, unwrapping `this` to a `NodeId` |

None of its hard parts are ours and none of ours are its. And the pattern it was being
praised for — IDL → typed IR → templates — is not its own: `wasm-bindgen/crates/webidl`
has the same shape (`first_pass.rs` builds the IR, `generator.rs` emits, `traverse.rs`
visits), in our dialect, against a JS host, under a permissive licence.

What survives as genuinely useful:

- It settled which `weedle` is which, which is a one-time answer we now have.
- **The escape hatches are a real checklist**, wherever they came from:
  `uniffi_bindgen/src/interface/` carries `rename.rs`, `exclude.rs` and `custom_type.rs`,
  and a generator without that trio dies on the first interface that does not fit. For us
  that is `HTMLElement` needing to stay a `Symbol.hasInstance` shim, and hand-written
  overrides surviving regeneration.
- [`askama`](https://crates.io/crates/askama) for rendering rather than `format!` chains,
  which is a library choice, not an insight.

Read `wasm-bindgen/crates/webidl` instead. Same pattern, right dialect, right licence.

So the shape, if we do it:

```
@webref/idl  →  weedle (parse)  →  a typed IR of ours  →  templates  →  Boa bindings
```

with rename/exclude/override tables from the start. The IR and the escape hatches are the
design; the emitter is the small part, because our binding surface is regular — nearly
every one ends in `dom_ctx`, `this_node_id` and a document call.

Everything named here is read-and-reimplement: `wasm-bindgen` is MIT/Apache but targets a
runtime we do not have, and UniFFI and Servo are MPL-2.0.

**The argument against is real.** `blitz-script` is Nico Burns's from upstream PR #491 and
we carry ~50 commits on it; a generated binding layer forks us from that branch
permanently. Decide deliberately, and decide before the hand-written surface doubles again.

### 5. Fragmentation, hit testing, accessibility — noted, not scheduled

Real gaps, none of them blocking a browser that renders sites today. Fragmentation matters
for print; a proper hit-test module matters when interaction gets serious; the
accessibility tree matters when someone asks for it. Listed so they are not rediscovered.

## 5b. CSS conformance: the gap is not Stylo, it is what we read back out of it

This is the section that came from reading code rather than listings, and it is the most
actionable thing in the document.

**Stylo is not the weak point.** It is Gecko's production style engine — rule tree, bloom
filters, style sharing cache, parallel traversal — and it computes hundreds of properties
correctly. Our conformance gap is downstream of it: `blitz-dom` and `blitz-paint` together
read **35 `clone_*` accessors and 16 struct fields**. Everything Stylo computes and we never
read is a silent conformance failure, and it fails silently precisely because the cascade
worked.

Today's `isolation` bug was exactly this shape: Stylo computed `Isolate`, `blitz-dom`
never looked, and a full-bleed background disappeared. **It is not one bug. It is a family,
and the code says so in its own TODOs.**

### `is_stacking_context_root` (`blitz-dom/src/node/node.rs`)

Checks opacity, position, transform and — as of today — isolation. Its own trailing
comments list what is missing: `mix-blend-mode`, `filter`, `clip-path`, `mask`, `contain`.

Per CSS, **every one of those creates a stacking context**, and three of them we already
compute and paint:

| Property | Painted? | Creates stacking context? | Status |
|---|---|---|---|
| `filter` | yes, `blitz-paint/src/render.rs` | **yes** | **not checked — bug** |
| `clip-path` | yes, `render/clip_path.rs` | **yes** | **not checked — bug** |
| `mask-image` | yes, `render/background.rs` | **yes** | **not checked — bug** |
| `mix-blend-mode` | **never read** | yes | bug, and the property is unimplemented |
| `contain` | **never read** | yes (`paint`/`layout`/`strict`) | bug, and unimplemented |
| `will-change` | **never read** | yes, for the listed properties | bug, and unimplemented |

So we paint a blur or a clip and then order it as though it were an ordinary box. The
symptom is the same as 24x.ai's: content that is drawn and then painted over, or that
escapes to the wrong ancestor.

### `establishes_containing_block` (`blitz-dom/src/resolve.rs`)

Checks `transform`, `translate`, `rotate`, `scale`. Its own comment names the rest:
*"TODO: `filter`, `backdrop-filter`, `will-change`, `contain` and `perspective` also do
this."*

That one matters more than it looks, because it decides which `position: fixed` descendants
get hoisted. A fixed element inside a `filter`ed ancestor must resolve against that
ancestor, not the viewport — we hoist it to the root and it lands in the wrong place. This
is the same function today's stacking-context fix was built around, so the shape is
already familiar.

### What to do

Two functions, one afternoon, and the tests are cheap because the pattern already exists in
`tests/blitz-tests/tests/isolation_stacking_context.rs`: build the markup, assert which
stacking context holds the child, assert the painted position.

Take them in this order, because it is the order of how often they appear on real pages:
`filter` and `clip-path` and `mask` first (already computed, three lines each in
`is_stacking_context_root`), then `will-change` and `contain` in both functions, then
`perspective` in the containing-block one, then `mix-blend-mode` as a paint feature in its
own right.

**This is the highest-conformance-per-hour work available**, and none of it needs
fastrender: it needs the two TODO comments already sitting in our own source, which is a
lesson about reading our code as carefully as we read theirs.

### Properties Stylo computes that we never read at all

Beyond the stacking-context set: `text-shadow`, `perspective`, `transform-style`,
`backface-visibility`, `background-blend-mode`, `border-image-source`, `accent-color`,
`appearance`, `text-orientation`, `shape-outside`, `column-count`, `column-gap`.

`text-shadow` is the notable one — ubiquitous, purely a paint feature, and we already have
`box_shadow.rs` to model it on.

## 5a. Their concurrency model, properly

An earlier draft of this document compared *what* is parallel on each side and called that
the concurrency section. It is not — a stage table is not a model. Read out of
`docs/ipc.md`, `docs/ipc_frame_transport.md` and `docs/live_rendering_loop.md`, theirs has
four layers, and the first one is the point.

### Layer 1: the process split is the trust boundary, not a performance decision

| Process | Trust | Owns |
|---|---|---|
| **Browser** | trusted | UI, window management, persistent user state (profile, cookies, history, bookmarks), spawning and supervising children. **The only process allowed to make privileged decisions** |
| **Renderer** | **untrusted** | parses and executes untrusted HTML/CSS/JS, produces pixels. Sandboxed, and *"assume it may send arbitrary malformed messages"* |
| **Network** | less-trusted | network I/O on behalf of the other two, sandboxed separately, *"must be treated as malicious for IPC purposes"* |

Three distinct IPC links: browser↔renderer for navigation, input and frame submission;
browser↔network for fetch, DNS and cookie mediation; renderer↔network for fetch and
WebSocket proxying. And the governing requirement: **the browser must be able to kill and
restart a renderer or network process** without risking its own memory or unbounded
resource use.

**This is the thing to take away.** Their concurrency and their security are one design.
The renderer is a separate process *because* it runs web content, which makes it
sandboxable and killable; the parallelism is a consequence. An earlier draft of our
[sandbox plan](TODO-sandbox-plan.md) filed process separation under performance
architecture — that was wrong, and it is why we currently have neither.

### Layer 2: frames over shared memory, with hard caps everywhere

Rendered frames cross browser↔renderer as FD-backed shared buffers — `memfd` with seals on
Linux, tempfile-backed as fallback — in a fixed premultiplied RGBA8 format. Around that:

- `MAX_IPC_MESSAGE_BYTES` 8 MiB, `MAX_FRAME_BUFFERS` 8, bounded decode that must consume
  the whole frame and reject trailing bytes
- **FD arity declared per message** (`expected_fds()`) and enforced at every receive site,
  with payload and FDs sent atomically in a single `sendmsg`/`recvmsg` — their term for the
  failure this prevents is "FD confusion"
- FD type, size and seals validated **before** `mmap`, to avoid SIGBUS and OOM
- ack-on-drop for frame buffers, which is flow control
- a versioned protocol (`RENDERER_PROTOCOL_VERSION = 2`)

The transferable discipline is not the FD passing, which we do not need in one process. It
is **"no unbounded allocations" as a normative rule with the caps in one file**
(`src/ipc/limits.rs`). Our equivalent is the per-document request and byte caps in the
[sandbox plan](TODO-sandbox-plan.md), and they belong in one place for the same reason.

### Layer 3: intra-process parallelism

Parallel display-list build and parallel raster tiling over rayon, a render worker off the
UI thread (`src/ui/render_worker.rs` 555KB), and `benches/layout_parallel.rs`. This is the
layer our stage table was measuring, and it is the *least* interesting of the four.

### Layer 4: the event loop is deliberately single-threaded, and split three ways

`BrowserTab` couples a live document, an HTML-shaped event loop (tasks, microtasks, timers,
`requestAnimationFrame`, `requestIdleCallback`) and a script scheduler — and exposes
**three separate drivers**:

- `run_event_loop_until_idle` runs tasks, microtasks, due timers and idle callbacks, and
  **explicitly does not render and does not run rAF**
- a step-wise driver
- a converge-to-stable driver

**That decomposition is worth copying directly.** We paced animation frames today, and the
reason that work was fiddly is that "run the event loop", "run animation callbacks" and
"paint a frame" are not separable in our shell the way they are here. Three named drivers
also make each one testable on its own, which is why their event-loop behaviour is
regression-tested and ours is measured by watching an fps counter.

## 5c. Concurrency, stage by stage: they are ahead in paint, we are ahead in style

Measured on both sides rather than assumed.

| Stage | fastrender | ours |
|---|---|---|
| Style / cascade | their own, threading unclear | **Stylo's parallel rayon traversal** — `blitz-dom/src/stylo.rs:147-150`, `StyleThreading::Parallel` is the **default** |
| Box construction | — | `parallel-construct` feature over rayon in `resolve.rs`… **not in the default feature list, so off** |
| Layout | `benches/layout_parallel.rs` 25KB | Taffy, single-threaded |
| Display list | **parallel build** | we have no display list at all (§0.2) |
| Rasterisation | **parallel tiling** | single-threaded: `grep -E "par_iter\|rayon\|thread" packages/blitz-paint/src/*.rs` returns **nothing** |
| Rendering thread | `src/ui/render_worker.rs` 555KB, off the UI thread | on the event loop |
| Process model | multi-process — `crates/fastrender-ipc`, `crates/fastrender-shmem`, `src/ipc/` 350KB | single process |

`docs/instrumentation.md` is where the paint claim comes from, in their own words: diagnostics
are aggregated across rayon workers for "parallel display-list build + parallel raster
tiling". They also bench contention directly — `benches/disk_cache_contention.rs`.

**So the honest scoreline**: we are not behind on style, and Stylo's parallel traversal is
about as good as that stage gets — it is Gecko's. We are behind everywhere after it. And
the two paint items are one item, because **parallel paint requires a display list**: you
cannot hand disjoint tiles to workers while the painter is walking the DOM. §0.2 and this
section are the same piece of work.

### Two findings that are ours, not theirs

**`parallel-construct` is dead code.** The feature exists, `resolve.rs` has eight `cfg`
sites for it and imports `rayon::prelude`, and it is **not in `blitz-dom`'s `default`
list**. So every build we ship constructs boxes sequentially. Either put it in `default`
and measure it, or delete it — carrying an unbuilt parallel path is worse than either,
because it rots without anyone noticing.

**Our one piece of parallelism is documented as unsafe for exactly our use case.**
`blitz-dom/src/config.rs:13-18` warns that two `Document`s resolving on
`StyleThreading::Parallel` concurrently share Stylo's global pool and can panic with
`already mutably borrowed` ([upstream #430](https://github.com/DioxusLabs/blitz/issues/430)),
and says to use `Sequential` for documents that may resolve while another parallel resolve
is in flight.

`Parallel` is the default. **chuzz never sets it** — `grep style_threading apps/chuzz/src`
is empty — so every tab takes it, and `blitz-dom/src/iframe.rs:71` propagates the parent's
setting to every sub-document. chuzz mounts each tab as a sub-document.

Whether it is reachable today depends on whether two resolves ever land on different
threads, which is not traced here and should be. But the shape is a latent panic in a
multi-tab browser, and by fastrender's own triage table a panic is P0. Worth ten minutes
to either prove it unreachable or set `Sequential` and move on.

## 6. Testing infrastructure: we have none of it

Counted, not estimated.

| | fastrender | ours |
|---|---|---|
| Criterion benches | **16** | **0** |
| Fuzz targets | **3** | **0** |
| CI workflows | perf, chrome fixture diff, ui perf smoke, test262, wpt | `ci.yml`, `wpt.yml` (upstream's, and we are behind on the runner) |

Their benches are not decorative — they name exactly the things we have been changing:
`layout_hotspots.rs` 59KB, `paint_benches.rs` 42KB, **`perf_regressions.rs` 35KB**,
`selector_bloom_bench.rs` 34KB, `cascade_bench.rs`, `layout_parallel.rs`,
`css_parse_pageset.rs`, `html_parse_bench.rs`, `float_bench.rs`,
`disk_cache_contention.rs`, `dom_clone_bench.rs`, `scroll_blit_bench.rs`,
`fragment_clone.rs`.

**This is urgent rather than aspirational.** Today alone: animation frames went 120.0 to
29.8fps, the taffy measure cache was retuned across five commits, layer instrumentation and
a background-clip skip are landing from the other session, and **nothing guards any of it**.
The next person to touch `resolve.rs` can undo all of it and the suite stays green, because
307 of our tests assert behaviour and none assert cost.

`.github/workflows/perf.yml` plus a `perf_regressions` bench is the answer, and it pairs
with the pageset scoreboard: `chrome_fixture_diff.yml` runs their accuracy loop **in CI**,
so a pull request shows what it broke rather than waiting for someone to look.

Fuzzing matters here for a reason that does not apply to most projects: a browser parses
hostile input by definition. Their targets are `text_shaping`, `svg_filters` and
`animation_properties` — all of them paths where a malformed page reaches a parser that
was written assuming a well-formed one.

## 7. How they run parallel agents, which is how we work

`AGENTS.md` (13KB) plus `docs/philosophy.md` and `docs/triage.md`. This is the part with the
most immediate application to us, because we are two sessions in `~/code` that collided
today: I edited `layout/damage.rs` and `resolve.rs` while the other session had damage-flush
and scroll-clamping work in flight on the same files.

**Their answer is an explicit ownership table.** Every workstream lists what it *owns* and
what it *does NOT own* — `capability_buildout` owns CSS and layout algorithms and does not
own page-specific fixes or browser UI; `browser_responsiveness` owns frame rate and input
latency and does not own chrome functionality. Two agents cannot both believe they own
`damage.rs`.

**Their priority order**, which is a sharper instrument than our TODO ordering:

| | | |
|---|---|---|
| P0 | Panics / crashes | any panic in production code |
| P1 | Timeouts / loops | a renderer that does not finish is wrong |
| P2 | Accuracy failures | missing content, wrong layout or paint |
| P3 | Big-stage hotspots | only when they block renders or iteration |
| P4 | Fidelity polish | |
| P5 | Spec expansion | only when it moves accuracy |

with the rule: **do not skip to P5 while P0–P2 exist**.

**The 90/10 rule** — 90% of effort on accuracy and capability, 10% on performance and
infrastructure — is worth sitting with, because it is a fair criticism of where today went.
Frame pacing, cache tuning, counters and eviction instrumentation are all P3 and below.
The `customElements` binding and the `isolation` fix were P2. The ratio was not 90/10.

**"No vanity work"**: changes that do not improve accuracy, remove a crash or timeout, or
reduce uncertainty for an imminent fix are not acceptable, and *instrumentation that never
leads to a fix is waste*. That is a direct test to apply to the counter and eviction-count
work, and the honest reading is that a counter earns its place only when it is followed by
the fix it pointed at.

**Rules of theirs that match decisions we reached independently today**, which is mild
evidence they are the right rules:

- *"Incomplete but correct beats complete but wrong."* Why the fixed-descendant test stayed
  as a documented failure rather than being deleted, and why
  `mark_containing_svg_for_reconstruction` was dropped rather than kept unproven.
- *"No panics in production code."* Exactly the class of the focus-and-pressed-node fix.
- *"No page-specific hacks, no hostname checks, no magic numbers for one site."* Worth
  writing down before someone fixes 24x.ai specifically.
- *"No post-layout pixel nudging; keep the pipeline staged."* Note our own
  `correct_hoisted_fixed_positions` is a post-layout pass — justified, because it corrects
  an offset the hoist introduced rather than nudging pixels, but it is the shape they warn
  about and it deserves the scrutiny.

**A failure taxonomy worth copying wholesale**: every pageset page carries a `hotspot`
field — `fetch`, `css`, `cascade`, `box_tree`, `layout`, `paint`, `decode`, `unknown` —
so a red page routes to a workstream instead of to whoever looks first. That is the field
that makes the scoreboard in item 1 actionable rather than merely informative.

## 8. Their JS engine, looked at properly

An earlier draft dismissed this in two lines — "swapping engines dwarfs everything, and it
is the part most entangled with the unlicensed code". The second half was wrong.

**`vendor/ecma-rs` declares `license = "Apache-2.0"`** in its workspace `Cargo.toml`. It is
also a standalone project, [`wilsonzlin/ecma-rs`](https://github.com/wilsonzlin/ecma-rs),
described as a "TypeScript parser, type checker, minifier, and compiler", pushed 2026-01,
11 stars.

**Provenance caveat, and it matters**: the standalone repo reports **no licence** through
the GitHub API — no `LICENSE` file — while the vendored copy's workspace metadata says
Apache-2.0. Those disagree. Treat it as unresolved until someone reads the actual file, and
do not copy from it on the strength of a `Cargo.toml` field.

### It is far more than a JS engine

Forty-eight crates. The parts worth knowing:

| Crates | What |
|---|---|
| `parse-js`, `hir-js`, `semantic-js`, `optimize-js`, `emit-js` | a full compiler pipeline, not just a parser |
| `vm-js` | the bytecode VM that actually runs in fastrender |
| **`native-js`, `runtime-native`, `runtime-native-abi`, `llvm-stackmaps`, `stackmap`, `stackmap-context`** | **an LLVM-backed native compiler with GC stackmaps.** That is a real AOT/JIT path, and it is why the workspace excludes them from `default-members` — they need LLVM |
| `typecheck-ts`, `types-ts-interned`, `ts-erase` | TypeScript type checking |
| `minify-js` | the author's known minifier |
| `test262`, `test262-semantic`, `megatest`, `conformance-harness` | conformance infrastructure |
| **`webidl`, `webidl-vm-js`, `webidl-runtime`** | **a Rust WebIDL→engine-binding generator** |

### What this changes, and what it does not

**Does not change**: we should not switch engines. Boa is a maintained project with a
community and published test262 results; `vm-js` is one person's, at 11 stars. Our JS
problem is not the engine — it is that we expose 12 globals and no `fetch`. A faster VM
renders exactly as many pages as a slower one when neither can make a network request.

**Does change**: two things I got wrong.

1. **The WebIDL question has a fourth answer, and it is a design rather than a dependency.**
   [Section 4](#4-generated-webidl-bindings--decide-do-not-drift) concluded that
   `wasm-bindgen/crates/webidl` was the only worked example targeting a JS host. `webidl` +
   `webidl-vm-js` is a second, it targets a *same-process DOM* rather than an FFI boundary,
   and it is **far smaller**. See §8b — the licence question is moot because what is worth
   taking is the shape, not the source.
2. **Boa has no JIT and that is fine.** Worth stating explicitly so nobody reopens it.

   The weak reason is that we are not JS-CPU-bound — we are JS-API-bound, and a faster VM
   renders exactly as many pages as a slower one when neither can `fetch`.

   **The strong reason is LLVM itself.** Taking `native-js` means taking LLVM as a build
   dependency, and that is a decision with years of ongoing cost: version drift against the
   toolchain, build times, distribution weight, and debugging through a code generator you
   do not own. Weighed against what it buys a browser that cannot yet make a network
   request, it is not close. This is settled by direct experience on the team rather than
   by inference, and it should not be relitigated on the strength of a benchmark.

   fastrender's own workspace says the same thing structurally: `native-js` and
   `runtime-native` are **excluded from `default-members`** precisely because they "require
   heavy system deps (notably LLVM)". They quarantined it too.

   It also points the wrong way for us. We are trying to *shrink* the native surface —
   `native-tls` to `rustls-tls` to drop OpenSSL, and auditing `dav1d-sys` out. Adding LLVM
   would be the largest step in the opposite direction available.

## 8b. The one design worth pulling: their WebIDL layer is 65KB

`vendor/ecma-rs` is a **git submodule**, so none of it is in fastrender's tree — it has to be
read from [`wilsonzlin/ecma-rs`](https://github.com/wilsonzlin/ecma-rs) directly. Doing that
is what makes this worth a section.

**The whole WebIDL binding layer is about 65KB across 16 files.** wasm-bindgen's equivalent
is roughly 440KB. That difference is the finding: a compact design exists, and here is its
shape.

| File | Size | Concern |
|---|---|---|
| `webidl/src/types.rs` | **3.5KB** | the type model — the entire IR |
| `webidl/src/convert.rs` | 12.8KB | ES ↔ IDL value conversion (WebIDL §3.2), the tedious part |
| `webidl/src/overload.rs` | **7.3KB** | overload resolution |
| `webidl/src/to_js.rs` | 7.9KB | emission |
| `webidl/src/lib.rs` | 10.2KB | assembly |
| `webidl-vm-js/src/lib.rs` | **30KB** | **everything engine-specific, quarantined in its own crate** |
| `webidl/tests/toy/mod.rs` | 15.8KB | a toy IDL fixture |

### Four things to take, none of them code

1. **The engine-specific part is a separate crate.** `webidl` knows nothing about the VM;
   `webidl-vm-js` is the only thing that does, and it is 30KB of the 65KB. For us that means
   a `webidl` crate that has never heard of Boa, plus a thin `webidl-boa`. Two consequences:
   the hard parts get tested without an engine, and swapping or upgrading Boa touches one
   crate.

2. **The IR is 3.5KB.** [Section 4](#4-generated-webidl-bindings--decide-do-not-drift) said
   "a typed IR of our own" without saying how big — and the honest reading of UniFFI's
   `interface/` directory would have suggested something ten times larger. It does not need
   to be. Do not build a `ComponentInterface`.

3. **Conversion and overload resolution are separate concerns, and both are small.**
   Overload resolution is the part of WebIDL everyone dreads — dispatching on argument types
   through the spec's distinguishing-argument algorithm — and it is **7.3KB**. That is a
   useful calibration: it is tractable, and it does not belong tangled into emission.

4. **A toy IDL fixture, not the real corpus, for tests.** `tests/toy/mod.rs` is 15.8KB of
   invented IDL exercising the generator. Testing against `@webref/idl` would make every
   test slow and couple the suite to spec churn. Build the toy first.

### So what we would actually do

Unchanged from §4 in outline, sharpened in proportion:

```
@webref/idl  →  weedle (parse)  →  ~4KB IR  →  convert + overload + emit  →  webidl-boa
```

with the last box the only one that knows what Boa is. The estimate that matters: **their
whole engine-agnostic half is 35KB.** Ours will not be smaller, and it should not be much
bigger.

## 9. The rendering layer: our rasteriser is 0.0.9

The paint *architecture* is §0. This is the layer underneath, and it is the one place their
more conservative choice looks better than ours.

| | Them | Us |
|---|---|---|
| Rasteriser | `tiny-skia` — a Rust port of Skia's raster backend, mature, what `resvg` uses | `vello` **0.9.0** on GPU, `vello_cpu` **0.0.9** on CPU |
| Painter | their own, `painter.rs` 932KB | `blitz-paint` 224KB over anyrender |

**`vello_cpu` is at 0.0.9**, and it is what our headless capture runs on. That single fact
explains a run of things we have recorded separately as unrelated:

- [TODO.md](TODO.md) item 4: the capture drops 256-aligned tile columns inside text inputs.
  Filed as "a tooling defect", which it is — a pre-1.0 rasteriser's.
- `backdrop-filter` discarded (§0.1). The vello backends take `_backdrop_filter` and drop
  it; `anyrender_skia` implements it.
- Colour fonts unknown (§0.3), where the constraint was "check what `vello_cpu` can draw".

**And `tiny-skia` is already in our lock file**, pulled in transitively by `usvg`. We are
carrying a mature CPU rasteriser and rendering our baselines with a 0.0.9 one.

**The concrete suggestion is narrow**: for the *capture and pageset baseline path
specifically*, evaluate `anyrender_skia` — which is in `~/code/ps-anyrender`, already
implements `backdrop-filter`, and rasterises through Skia rather than vello_cpu. The window
can stay on GPU vello. The reason to care is that
[the pageset scoreboard](#1-accuracy-tooling--the-one-that-matters) is only worth building
if the capture path is trustworthy, and today we know it is not.

Keep vello for the window. It is the right long-term bet and the GPU path is where the
performance is. This is about which rasteriser we measure with.

## What not to chase

- **Their JS engine** (`vendor/ecma-rs`) — see §8, which corrects the licence claim an
  earlier draft made here. Do not switch: Boa is maintained with a community and published
  conformance results, and our JS problem is the 12 globals we expose rather than the VM
  under them. **Especially do not take `native-js`**, which means taking LLVM.
- **An LLVM-backed JIT**, for anything. Years of build-time, version-drift and
  debugging-someone-else's-codegen cost, bought with a speedup in the one layer that is not
  our bottleneck.
- **Style, text and flex/grid.** Stylo, Parley and Taffy are the composition bet. Their
  10MB of layout is not a gap.
- **Multi-process and media.** Real browser features, wrong decade for us.

## Related

- [blitz-fork-sweep.md](blitz-fork-sweep.md) — the corpus gap this answers
- [formal-web-what-to-learn.md](formal-web-what-to-learn.md) — the other outside engine reviewed
- [TODO.md](TODO.md) — the site inventory item item 1 above subsumes
