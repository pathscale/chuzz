# Handover: make Chuzz load and render 24x.ai correctly

Written 2026-08-11 as a handover to a fresh session. Everything below is a verified read of
the working tree on that date, except where marked. **Nothing here was measured or run.**

## The task

Chuzz already opens 24x.ai as its home page (see `git log`: "Point home at 24x.ai and
report resolved image sources", "Open 24x.ai in new tabs as well as home"). It does not
render correctly. Make it render correctly.

A previous attempt stalled. The failure mode reported was that the session concluded the
work was impossible and kept yielding rather than making progress. **Read the
"Anti-yield contract" section before starting.**

## Isolation contract: do not disturb the AgencyZero work

Another session is working in `~/code/agencyzero` concurrently. The good news, verified on
2026-08-11:

- **AgencyZero currently has no local Blitz path overrides.** The
  `[patch."https://github.com/pathscale/ps-blitz.git"]` block that used to redirect to
  `~/code/ps-blitz-render` has been removed from `agencyzero/Cargo.toml`, and
  `agencyzero/apps/gui/Cargo.toml` has no path deps. It builds ps-blitz from its pinned git
  rev. **Verify this is still true before you start**, because it changes the whole
  isolation picture if the patch comes back.
- **Chuzz path-depends on `~/code/blitz-rust`**, seven crates, `chuzz/Cargo.toml:16-33`,
  plus `~/code/endpoint-libs`.

So the two projects currently share no local checkout for their main builds.

### Rules

1. **Work on a branch in `~/code/chuzz`.** Currently on `master` with `docs/` untracked.
   AGENTS.md forbids committing to `master`.
2. **Engine changes go in `~/code/blitz-rust`, on a branch.** That is Chuzz's engine tree.
3. **Never touch `~/code/ps-blitz-render`, `~/code/ps-anyrender`, or
   `~/code/tauri-runtime-blitz`.** Those belong to AgencyZero even when it is not currently
   path-depending on them.
4. **One overlap to know about:** `agencyzero/apps/blitz-preview/Cargo.toml:12` path-depends
   on `~/code/blitz-rust`. It declares its own `[workspace]`, so AgencyZero's main build
   does not compile it, but an engine change you make can break that app. If you change
   `blitz-rust`, say so in the PR body.
5. **Do not create git worktrees or fresh clones.** See disk, below.

## Disk: there is no room for a second tree

Measured 2026-08-11 with `du -sh`:

| Path | Size |
|---|---|
| `agencyzero/target` | **209 G** |
| `blitz-rust/target` | 68 G |
| `ps-blitz-render/target` | 41 G |
| `chuzz/target` | 26 G |

That is roughly 344 G already committed to build artifacts. Do not add a fifth target
directory by cloning or worktree-ing anything.

**Do not set a shared `CARGO_TARGET_DIR` across projects.** Cargo takes a lock per target
directory, so a shared one would serialize your builds against the AgencyZero session's,
and differing feature sets would thrash the cache.

If you genuinely need space, `ps-blitz-render/target` (41 G) is the best candidate since
AgencyZero no longer path-depends on that tree, but **ask the owner first**. AGENTS.md
requires asking before anything destructive.

## What is already known, so you do not rediscover it

These are verified reads, with line numbers, from `blitz-rust/packages/blitz-dom/src/`
unless stated. Full detail in [blink-for-a-browser.md](blink-for-a-browser.md).

### The likely wall: shadow DOM is not implemented

- `stylo.rs:224` and `:231`: `todo!("Shadow roots not implemented")`
- `stylo.rs:300`: `as_shadow_root` returns `None`, with `// TODO: implement shadow DOM`
- `blitz-script` exposes no `attachShadow`

**These are `todo!()`, so they panic rather than degrade.** If 24x.ai uses web components,
this is the first thing to establish, and it changes the shape of the whole task. Aurora,
another Blitz-based browser, carries `src/dom/shadow.rs` (20 KB) plus a 138 KB
custom-elements polyfill for exactly this reason.

### The JavaScript surface is small

- `blitz-script`'s DOM surface is **33 methods** total. No `MutationObserver`, no
  `customElements`, no `attachShadow`.
- The web API shim is hand-written and partial:
  `apps/chuzz/src/document_loader.rs:58` `WEB_API_SHIM` covers `localStorage`,
  `sessionStorage` and a `URL` whose own comment says it is "not a WHATWG-conformant
  implementation".
- The engine is Boa, not V8. Boa has no rope strings, so heavy string building in page
  scripts is quadratic. See `agencyzero/docs/js-engine-big-problem.md`.

### Style invalidation is over-broad but correct

Not a rendering-correctness issue, so do not chase it here. It is documented in
`agencyzero/docs/style-invalidation-we-already-ship.md` and
[TODO-dom-related-work.md](TODO-dom-related-work.md) if you trip over it.

## How to diagnose without taking over the desktop

AGENTS.md is explicit: never take over the desktop, never drive the real app for visuals,
and never touch AgencyZero's running instance or its data directory.

Use these instead:

1. **Headless capture.** `apps/chuzz/src/main.rs:55` handles `--capture <file>` behind the
   `capture` feature: it renders the page with the CPU rasteriser and exits.
   `apps/chuzz/src/capture.rs:8` notes it deliberately shares the loader with the browser,
   so what it captures is what the browser would show. This is the primary tool.
2. **The control surface.** `crates/chuzz-control` speaks the same MCP framing as
   AgencyZero's Blitz interface (`blitz.agent.control`, `blitz.diagnostics`), with
   `Inspect` and `Snapshot` requests (`crates/chuzz-control/src/lib.rs:228`, `:286`).
3. **Console and network.** The loader resolves and reports image sources already; extend
   that reporting rather than guessing.

## Anti-yield contract

The previous attempt stalled by declaring the task impossible. It may genuinely be blocked
on a missing web platform feature. That is a finding, not a conclusion, and the difference
matters:

- **Never report "impossible" without naming the specific missing capability**, the source
  line that proves it is missing, and what the page does that needs it. "Shadow DOM is
  `todo!()` at `stylo.rs:224` and the page's `<x-header>` element attaches one" is a
  finding. "This cannot work" is not.
- **Partial progress counts and should ship.** If the page renders with three of five
  sections correct, land that and write down the two that do not.
- **Work the list, not the goal.** Enumerate every concrete failure first (missing API,
  panic, layout difference, missing asset), then attack them one at a time. A single
  ordered list of failures is what the previous attempt appears to have lacked.
- **When blocked on one failure, move to the next** and record the blockage here rather
  than stopping.
- **If the honest answer is that a feature is too large for this task**, write the size
  estimate and the alternatives (polyfill, partial implementation, upstream) and let the
  owner decide. Do not decide by yielding.

## The failures, measured 2026-08-11

Everything below was measured, not inferred. Method, because it decides what each
number is worth:

- **Layout** comes from `CHUZZ_CAPTURE_TREE=<file>` (added this session), which writes
  every node's absolute box and computed `display`/`position` from the same settled
  document the pixels come from. Compared against `getBoundingClientRect` and
  `getComputedStyle` taken from a real browser at the same 1440x960 viewport.
- **Paint** comes from two places: `--capture` PNGs probed pixel by pixel, and the
  windowed browser, which the owner looked at because this session has no screen
  recording.
- **The live tree** comes from the control socket's `inspect`, pulled from the running
  window, so layout claims are not capture-only.

**Set `CHUZZ_CAPTURE_SCALE=2` for anything you intend to compare against the window.**
The window renders the page as a sub-document at scale 2 on this display; at scale 1 the
capture disagrees with the screen, and failure 3 is invisible.

### What is not wrong, so nobody re-derives it

The page very nearly lays out correctly. Against the reference, at the same viewport,
these agree to the pixel:

| | reference | chuzz |
|---|---|---|
| `.page-container` | 1180 wide, centred | 1180 wide, centred |
| hero grid `lg:grid-cols-[1.2fr_0.8fr]` | 694 and 462, gap 24 | 694 and 462, gap 24 |
| `h1` | 172,174 596x220 | 596x220, same origin |
| the three feature cards | 377 / 377 / 377 | 377 / 378 / 377 |
| form, labels, inputs, button | 412 wide, 40 tall | 412 wide, 40 tall |

The live tree and the headless tree are node for node identical. Grid, flex, the
`page-container` centring, mask-image icons, the raster background image, inline SVG
sizing, the SolidJS bundle, and Brotli responses all work. **Layout is not the problem;
paint and the platform surface are.**

### 1. `backdrop-filter` is discarded by both renderers RENDERER

The single largest visual difference, and it is not in Blitz.

- `blitz-paint` computes it (`packages/blitz-paint/src/render.rs:379`) and passes it down
  (`packages/blitz-paint/src/layers.rs:67-79`).
- `ps-anyrender-vello-0.13.0/src/scene.rs:74-76` takes `_filter` and `_backdrop_filter`
  and calls `push_layer` without them. **The GPU path the window uses drops every CSS
  filter and every backdrop filter.**
- `ps-anyrender-vello-cpu-0.15.0/src/scene.rs:69` drops `_backdrop_filter` too, and
  honours `filter` only under a `filters` feature that chuzz does not enable
  (`:71-80`).

24x.ai puts `backdrop-filter: blur(4px) saturate(1.1)` on `.glass-nav` and
`.hero-copy-glass`, and `blur(6px) saturate(1.15)` on `.hero-form-pop`. Every panel that
should read as frosted glass shows the city photograph through it at full sharpness. That
also accounts for the page reading brighter and busier than the reference.

Not fixable in this repository or in `blitz-rust`: the backends are crates.io releases.

### 2. `position: fixed` is laid out against the document, not the viewport ENGINE

- Reference: `.pointer-events-none.fixed.inset-0` is 1425x**960**, the viewport.
- Chuzz, live: the same node is 1410x**1027**, the document height. Headless at 1440:
  1440x1027.
- `Position::Fixed` appears exactly once in the engine, at
  `blitz-rust/packages/blitz-dom/src/node/node.rs:1001`, and that is for stacking
  contexts. Layout hands fixed to Taffy, whose only out-of-flow mode is absolute, so a
  fixed box resolves against the root box.

On 24x.ai the background image is stretched about 7 percent vertically, and it will
scroll away with the page instead of staying put. On any site with a fixed header it is
worse.

### 3. The textarea placeholder is drawn at half size off scale 1 ENGINE

Visible in the window; reproduced headlessly with `CHUZZ_CAPTURE_SCALE=2`; **absent at
scale 1**, which is why the first capture looked right.

At scale 2 the placeholder is painted twice: once at the correct size, and once at half
size over the top. Single-line `input` placeholders in the same form are correct at both
scales, so this is the multiline path. `TextInputData::sync_multiline_width`
(`packages/blitz-dom/src/node/text.rs`, uncommitted in the working tree) is the only code
that re-lays out a placeholder editor after layout, and it takes
`layout.content_box_width()`, which is CSS pixels, on a document whose paint is scaled.
That is the first place to look, not a conclusion.

A minimal page with a plain 400px textarea does **not** reproduce, so something about the
24x textarea (`w-full min-w-0` in a flex column, `resize: vertical`, an inset box shadow)
is part of it.

### 4. The JavaScript platform surface is missing pieces sites depend on PLATFORM

From the run log of the live browser, with the sites that hit them:

| Missing | Symptom |
|---|---|
| `Text` | pathscale.com's bundle dies on load: `ReferenceError: Text is not defined` |
| `import.meta` | Boa parse error, so the module never runs |
| `CustomEvent` | thrown twice on ebay.com |
| `screen` | thrown on ebay.com |
| `IntersectionObserver` | thrown from a `DOMContentLoaded` handler |

`MutationObserver` is already shimmed as a no-op recorder
(`apps/chuzz/src/document_loader.rs:166`); these belong beside it, except `Text`, which is
a DOM constructor and belongs in `blitz-script`, and `import.meta`, which is Boa's.

24x.ai itself does not need any of them.

### 5. The CPU capture drops whole tile columns inside a text input TOOLING

Not a page defect. It is a defect in the tool I was diagnosing with, and it cost real time,
so it is written down.

- At 1440 wide, the pixels at x 1024..1279 are fully transparent wherever an input or the
  textarea is painted. At 1472 the same column. At 1408, nothing.
- At scale 2 it is x 1792..2559: three columns, all 256-aligned, and exactly the wide
  tiles that fall **entirely inside** an input's box. The partly covered tiles at the
  edges paint.
- The window does not show it, so it is `vello_cpu`, not Blitz.
- A minimal page with an input and a textarea at the same coordinates does not reproduce.

Consequence: a capture is trustworthy for layout and mostly trustworthy for paint, but
alpha-zero regions in one are the tool, not the page.

### 6. Intrinsic text is about 13 percent narrower than the reference TEXT

The only place the page lets an intrinsic text width show is a chip, and there:
`.chip--flat.chip--sm` measures 150 and 163 in the reference, 134 and 143 live. Subtracting
padding, border, icon and gap, the 11px text measures 104 against 120.

The page asks for `ui-sans-serif, system-ui, sans-serif` and ships no webfont, so this is
font selection or small-size metrics, not layout. Nothing else on the page is visibly
affected, because every other box takes its width from its container.

### 7. Closing a tab killed the browser APP, fixed

Hit while the owner was driving the live window: four tabs open, close one, and the
process died with `index out of bounds: the len is 3 but the index is 3` inside
`dioxus-stores`. A `Store<Tab>` from `tabs.iter()` is a positional lens and the animation
clock held one across a 16ms await. Fixed in `Resolve the animated tab by id rather than
by position`; `tab.rs` now re-resolves by id every tick and stops when the tab is gone.

### 8. Stale control descriptors accumulate MINOR

`ControlServer::drop` removes the socket and its descriptor, but a killed process leaves
both behind: `$TMPDIR` holds a dozen `chuzz-*.json` files from yesterday. A client that
picks the newest descriptor can connect to a dead pid. Sweep on startup, or check the pid
before trusting one.

## Where each failure lives

| # | Layer | Fix goes in |
|---|---|---|
| 1 backdrop-filter | renderer | `ps-anyrender-vello`, upstream, not this machine |
| 2 fixed positioning | engine | `blitz-rust`, layout |
| 3 textarea placeholder | engine | `blitz-rust`, `blitz-dom` text input |
| 4 web APIs | app and engine | `document_loader.rs` shim, plus `blitz-script` for `Text` |
| 5 tile columns | tooling | `vello_cpu`, upstream |
| 6 text metrics | engine | font selection in `blitz-dom` |
| 7 tab panic | app | done |
| 8 stale descriptors | app | `chuzz-control` |

Nothing here makes 24x.ai impossible. Failure 1 is the only one that changes how the page
reads, and it is a missing renderer feature with a named source line, not a wall.

## Also changed this session, at the owner's request

- New tabs open `about:blank` again rather than the home page.
- Cmd+L and Cmd+D focus the address bar. Both chords already resolved; the Toolbar was
  never given the signal, so nothing read it.

## Constraints from AGENTS.md

Branch, PR, no commits to `master`. No em dashes anywhere, including commit messages. No AI
attribution. Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets
--all-features -- -D warnings` and `cargo test --workspace --all-features` before delivery.
Ask before installing anything.

## Related

- [blink-for-a-browser.md](blink-for-a-browser.md), the engine gaps that matter for a
  browser, with line numbers.
- [TODO-dom-related-work.md](TODO-dom-related-work.md), the DOM work plan, including the
  ENGINE-in-two-trees problem.
- [animation-gap.md](animation-gap.md), why an arbitrary page's animations pin the process.
- [small-things-to-learn-from-aurora.md](small-things-to-learn-from-aurora.md), the other
  Blitz-based browser and how it solved the same gaps.
