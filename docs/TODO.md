# TODO: what to work on, and what is still research

Written 2026-08-11, revised the same day after the first round of measurement. A
high-level index: every line points at the document carrying the evidence and the line
numbers. **Read the linked doc before acting**, because several items carry ordering
constraints or caveats that do not survive a one-line summary.

Items marked ENGINE land in `~/code/ps-blitz`, which is now a plain checkout of
`ps-blitz` master. That was not true this morning, and everything measured before the
reconciliation carries an asterisk because of it.

## Done today

- **The engine fork is closed.** The checkout was 58 commits behind `ps-blitz` master with
  eleven uncommitted changes on top; nine were hand-applied copies of work master already
  carried. The two that were not, plus a placeholder scale fix, are on master now.
- **taffy 0.13 came with it**, which is the layout cache change this list wanted A/B
  tested. It is in use and unmeasured, which is not the same as tested.
- **Animation-only frames are paced, measured 120.0fps to 29.8fps** on a CSS spinner. A
  page that animates anything no longer repaints the whole window at the display's rate
  for as long as it is open. [animation-gap.md](animation-gap.md)
- **`Text`, `CustomEvent`, `IntersectionObserver` and `screen`** exist now. pathscale.com
  got two errors further before hitting `customElements`, which is the shadow DOM wall.
- **Closing a tab no longer kills the browser.** A positional `Store<Tab>` lens held across
  an await.
- Cmd+L and Cmd+D focus the address bar; new tabs and startup open `about:blank`.
- **Packaging**: `Chuzz.app`, the release pipeline, and a Homebrew cask.

## Also done today, on the engine

Eight merges onto `pathscale/ps-blitz` master, which is now **317 tests passing** (325 with
`--features blitz-dom/shadow-dom`), up from 297.

- **mega-blitz became master.** The fork is rebuilt on `DioxusLabs/blitz` main instead of
  carrying 59 commits against a base 74 behind it. This **replaced** history rather than
  merging it; the old tip is preserved as `pathscale/pre-mega-master`, and `ps-blitz-render`
  is based on it, so leave that branch alone until the other session has rebased.
- **Shadow DOM and custom elements**, ported from upstream's unmerged
  `devin/1782520416-shadow-dom-custom-elements`, behind `feature = "shadow-dom"`, off by
  default. 19 conflicts across 8 files.
- **`customElements.define/get/getName`** on top, upgrading on define and on insertion.
  The constructor body still does not run — Boa cannot construct into an existing object.
- **Seven commits ported from `ps-blitz-render`**: `element.style.x = y` actually writing
  through, `ParentNode` insertion, `navigator.clipboard`, `document.getSelection`, unzoomed
  box metrics, focus and pressed-node cleanup on removal, and an anonymous-box leak guard.
- **`isolation: isolate` establishes a stacking context**, and a hoisted fixed layer now
  paints in the context its box tree gives it rather than the root's.
- **The SVG sprite call sites are reconnected** — three functions had kept their definitions
  and lost their only caller when `construct.rs` was reverted during the merge.

## The world moved under this document

The other session has since pushed **17 more commits** to master, and two of them change
conclusions written above:

- **`chore: take the renderer crates from ps-anyrender`.** Master now takes `anyrender`,
  `anyrender_vello`, `_cpu` and `_hybrid` from `github.com/pathscale/ps-anyrender` at rev
  `0e4122b`. **This was the blocker on pointing chuzz at the mega tree** — the two used
  different renderer families and `PaintScene` would have been two unrelated traits.
- **`chore: namespace the three crates downstream declares by name`.** `ps-blitz-dom`,
  `ps-blitz-traits` and `ps-blitz-script` are namespaced again — but `blitz-html` and
  `blitz-paint` are **not**, and chuzz's `Cargo.toml:17,33` still asks for `ps-blitz-html`
  and `ps-blitz-paint`.

**So the chuzz port is now viable and small**: repoint the six paths from `../ps-blitz` to
the mega tree, and drop the `package =` rename on `blitz-html` and `blitz-paint` only. It was
correctly deferred this afternoon; it is not blocked any more.

They also fixed two things recorded as open elsewhere in these documents:
`BLITZ_INCREMENTAL` decides again, and the `debug-control` feature declarations the replay
dropped are restored.

## 24x.ai: parked at roughly 80 percent

Good enough, and the rest is low value. Recorded here rather than dropped, since these are
real defects that any other site will hit.
[HANDOVER-24x-rendering.md](HANDOVER-24x-rendering.md) has all eight with measurements.

| # | Item | Why it is parked |
|---|---|---|
| 1 | **`backdrop-filter` is discarded by the vello renderers.** `blitz-paint/src/filters.rs` computes both filters and `render.rs:402-432` passes them to `push_layer`; `anyrender_vello{,_cpu,_hybrid}/src/scene.rs` take `_backdrop_filter` and drop it. Every frosted panel renders sharp. | ~~Not fixable in this repo~~ **This was wrong, and it stopped us looking.** `~/code/ps-anyrender` is ours, and `anyrender_skia/src/scene.rs:450-453` already implements it. Do `anyrender_vello_cpu` first so headless capture can prove it. Still **the biggest single visual gap on any modern site**, and now the cheapest. [TODO-fastrender-what-we-can-learn.md](TODO-fastrender-what-we-can-learn.md) §0.1 |
| 2 | **`position: fixed` sizes against the document, not the viewport** (1027 tall against a 960 viewport). Still true after the engine move. | Real, and worse on sites with fixed headers than on 24x.ai. The next engine item to pick up if a site needs it. |
| 3 | Intrinsic text measures a few percent narrower than a reference browser. Chips: 134 and 143 against 150 and 163. | Cosmetic. The engine move changed it slightly in both directions, so it is font selection rather than a single bug. |
| 4 | The CPU capture drops 256-aligned tile columns inside a text input. `vello_cpu`, not Blitz; the window is unaffected. | A tooling defect. Known and characterised, so it costs nothing as long as nobody reads a capture's transparent regions as a page defect. |

**Use `CHUZZ_CAPTURE_SCALE=2` for any capture meant to be compared against the window.**
The window renders the page as a sub-document at scale 2, and at scale 1 the capture
disagrees with the screen. That one flag is what separated two rasteriser artifacts from
real page defects.

## Do next

| # | Item | Detail |
|---|---|---|
| 1 | `import.meta`, the last of the five missing JavaScript globals and the only one that is Boa's rather than ours. A module using it is a parse error, so the whole file never runs. | [HANDOVER-24x-rendering.md](HANDOVER-24x-rendering.md) failure 4 |
| 2 | ~~Do not shim `customElements`~~ **Done, and in the right order.** Shadow DOM and custom elements were ported from upstream's unmerged branch behind `feature = "shadow-dom"`, then `customElements.define/get/getName` landed on top with upgrade on define and on insertion. The trap this item described — a bundle getting past the `ReferenceError` into a `todo!()` — was avoided by doing shadow DOM first. **The constructor body still does not run**, because Boa cannot construct into an existing object. | `ps-blitz` PR #6 |
| 3 | **Point the engine at `ps-taffy`.** The layout cache work is shared with AgencyZero and lands in the fork, `~/code/ps-taffy`, which AgencyZero already consumes as a path dependency through `ps-blitz-render`. `ps-blitz/Cargo.toml:99` still takes stock `taffy` from crates.io, so none of that work reaches this browser. One line, and it is the prerequisite for inheriting the rest. | see below |
| 4 | Two counters: DOM node count against layout node count, and distinct versus total attribute values on a real page. Both size per-page costs that scale with everything loaded. | [blink-for-a-browser.md](blink-for-a-browser.md) section 3 |
| 5 | Make Stylo snapshots updatable. A correctness fix, and it unblocks the invalidation work. ENGINE | [TODO-dom-related-work.md](TODO-dom-related-work.md) item 1a |

## Security: a live arbitrary file read, and the sandbox plan

**[TODO-sandbox-plan.md](TODO-sandbox-plan.md)**. One item cannot wait:

`blitz-net/src/lib.rs:147-149` does `std::fs::read(request.url.path())` for any `file:` URL,
**with no origin check anywhere above it**. A remote page embedding
`file:///Users/you/.ssh/id_rsa` reads it, through a stylesheet `url()`, an `<img>`, an
iframe, or directly once `fetch` exists. **The fix is about five lines** and should land
before anything else on any list here.

The plan deliberately does not copy Chrome or adopt CSP. Chrome's and Firefox's sandboxes
exist to contain memory-safety exploits in C++, and our hostile-byte surface — html5ever,
Stylo, `png`, `zune-jpeg`, `gif`, `image-webp`, `usvg`, `skrifa`, Boa — is **already
memory-safe Rust**. That is the list Firefox needed RLBox for. Our remaining C is mostly
removable: `native-tls` pulls in OpenSSL where `rustls-tls` is a feature flag away.

So our sandbox is capability restriction, not memory containment, and it goes at the choke
point we already own: `fetch_inner` sees every subresource and already switches on scheme.
Deny `file:` to web origins, deny private and loopback addresses to public ones (a page port
-scanning your dev servers is the attack CSP does nothing about), scope cookies, cap request
counts. **CSP asks the page what it wants; we decide what a page gets.**

## The plan

**[TODO-plan-what-exists-in-open-source.md](TODO-plan-what-exists-in-open-source.md)** is
the phased plan, built on one observation: fastrender wrote 57MB from scratch and **we
compose**. Most of the audit's gaps are not builds.

- **The HTTP disk cache already exists and is switched off.** `blitz-net` has a `cache`
  feature wiring `http-cache-reqwest` + `cacache` + `directories`; chuzz enables
  `http2, cookies, compression` and not `cache`. One word, not a medium build.
- **`fetch` is mostly done too.** `reqwest` 0.13 and `http` 1.4 are already dependencies and
  `blitz-net` already does TLS, redirects, compression and cookies. What is missing is the
  *binding*, not the stack.
- `url` 2.5.8 and `encoding_rs` 0.8.35 are already in the tree, so `URL`/`URLSearchParams`
  and `TextEncoder`/`TextDecoder` are bindings over parsers we ship.
- **Phase 0 is five items that need no new dependency at all**, two of which are the biggest
  visual defects we know of.

Sandboxing is the exception that stays ours: `birdcage` is the only cross-platform option
and is two years stale, `extrasafe` and `ErickJ3/sandbox-rs` are Linux-only (the latter also
archived), and we ship macOS. A Seatbelt profile plus a probe that fails loudly.

## The full audit: us against fastrender against Chrome

**[TODO-audit-us-vs-fastrender-vs-chrome.md](TODO-audit-us-vs-fastrender-vs-chrome.md)**
is the section-by-section audit — JS API surface, CSS properties consumed, paint,
concurrency, HTML platform, layout, networking, testing, security — measured by grepping
our own source, with Chrome as the bar. Its ranked list is the authoritative one; the
headlines:

- **We provide 12 JS globals and `fetch` is not one of them.** A page has no way to make a
  network request from JS at all. `getComputedStyle` and `ResizeObserver` are also absent.
- **Nine of chuzz's JS APIs are shims injected by the app**, not bindings in the engine, so
  `blitz-script`'s tests never see them and no other embedder gets them.
- **Today's `isolation` bug is a family of six.** `filter`, `clip-path` and `mask` are
  already painted and still do not create a stacking context.
- **`parallel-construct` is dead code** — the feature is not in `default`.
- **`StyleThreading::Parallel` is the default and documented as unsafe** for multiple
  concurrent documents, which is what a tabbed browser is.
- **0 benches, 0 fuzz targets**, no perf or accuracy job in CI.

## From the fastrender review

Ranked, from [TODO-fastrender-what-we-can-learn.md](TODO-fastrender-what-we-can-learn.md).
**Nothing there may be copied** — the repo carries no licence, so it is all-rights-reserved
and every item below is a design to reimplement from spec or from our own tree.

Read the size difference carefully before acting on it: their `src/` is 57MB against our
1.8MB, and most of that is style, text, flex and grid written from scratch where we
delegate to Stylo, Parley and Taffy. Those are not gaps.

| # | Item | Why |
|---|---|---|
| 0 | **`backdrop-filter` in `anyrender_vello_cpu`, then `anyrender_vello`.** Not from fastrender — found by checking the claim in item 1 of the 24x.ai table above, which was false. We own `~/code/ps-anyrender`; `anyrender_skia` already implements it. | Smallest and highest-value item on any list here. Do the CPU backend first so the scoreboard can prove it. Backdrop-root semantics are the part to get right. **Attempted 2026-08-12 and the obvious approach is ruled out:** having the backend rasterise its own in-progress command list to snapshot the backdrop cannot work, because `vello_cpu`'s `render_to_buffer` asserts `!wide.has_layers()` and `blitz_paint` calls `push_layer` from several `maybe_with_layer` frames deep, so the layer stack is never empty when a backdrop is wanted. It panics on every real page. The backdrop has to be rendered as its own scene, and what counts as the backdrop is `blitz_paint`'s decision, so start in `blitz-paint/src/layers.rs` rather than in the backend. Separately: the `filters` feature on `anyrender_vello_cpu` was enabled by no consumer, so element `filter:` was being dropped too; chuzz now enables it, which changes nothing on 24x.ai because that page uses `backdrop-filter` only |
| 1 | **The pageset scoreboard.** `fetch_pages` → `fixtures/html/`, `render_pages` reusing `apps/chuzz/src/capture.rs`, a Chrome baseline **from the same cached HTML** with JS disabled, `diff_renders` → HTML report, and `fixtures/progress/<stem>.json` carrying `status`/`stages_ms`/`hotspot`/`notes`. Baseline against a live load and you diff ad rotation, not your renderer. | Closes the corpus gap [blitz-fork-sweep.md](blitz-fork-sweep.md) names, subsumes the site-inventory item below, and would have caught today's stacking-context bug on its own. Watch the `vello_cpu` tile-column artefact in item 4 above, or every page reads as broken |
| 2 | **A disk cache in `blitz-net`.** 15KB today with no cache at all, so every navigation refetches everything — including every pageset run. Keyed by URL, honouring `Cache-Control`/`ETag`. | Makes item 1 fast and the browser usable offline. Self-contained, no engine risk |
| 3 | **A macOS Seatbelt slice.** We ship a signed `Chuzz.app` that loads arbitrary sites with no sandbox. A profile plus `sandbox_init`. | The only security gap on the list, and the cask is getting wider use |
| 4 | **Decide about generated WebIDL bindings**, do not drift into it. We hand-write every binding in `blitz-script/src/dom/`, and today found `HTMLElement` was a `Symbol.hasInstance` shim whose prototype had no relation to the real DOM prototype — exactly what generation removes. The stack is settled if we go: **`@webref/idl` → [`weedle`](https://crates.io/crates/weedle) 0.13.1 to parse → a typed IR of ours → templates → Boa prototypes and `NativeFunction` shims.** | **Use `weedle`, not `weedle2`.** Same MIT licence, but weedle2 "forked to extend the functionality beyond WebIDL needs" for UniFFI's UDL dialect and is two years staler; `weedle` is what `wasm-bindgen/crates/webidl` ships against, so it is exercised over every interface in `web-sys`. Read that crate as the worked example — `first_pass.rs` builds the IR, `generator.rs` emits — it is real WebIDL against a JS host under MIT/Apache. Build rename/exclude/override tables in from the start or the generator dies on the first interface that does not fit. Not a port, a decision: `blitz-script` has an upstream home in PR #491 and generating forks us from it. [TODO-fastrender-what-we-can-learn.md](TODO-fastrender-what-we-can-learn.md) §4 |
| 5 | **A display list.** `blitz-paint` walks the DOM straight into a scene with no intermediate representation — `grep display_list packages/*/src` is empty. Theirs is 2MB across builder, renderer and an optimise pass. | The structural prerequisite for the damage-regions item further down this file: you cannot repaint a damaged region without a record of what was painted where. Also what makes paint parallelisable and occlusion-cullable. Ours should be a flat `Vec` of commands with bounds, not 2MB |
| 6 | **`srcset` / `sizes` / `<picture>`.** `grep srcset packages/*/src` is empty; we take the `src` fallback or nothing. | Ubiquitous on content sites, and it is parsing plus selection rather than layout. The cheapest real-site win after item 0 |
| 7 | **A perf regression bench in CI.** We have **0 benches and 0 fuzz targets**; they have 16 and 3, plus `perf.yml` and a workflow running the accuracy diff on every PR. | Today's frame pacing went 120.0 to 29.8fps, the taffy cache was retuned over five commits, and more perf work is landing from the other session — **nothing guards any of it.** 307 tests assert behaviour, none assert cost. Fuzzing matters too for a reason specific to us: a browser parses hostile input by definition |
| 8 | **An ownership table for parallel sessions**, in the style of their `AGENTS.md`: every workstream lists what it owns *and what it does not*. Plus their priority order — P0 panics, P1 timeouts, P2 accuracy, P3 hotspots, P4 polish, P5 spec expansion, never skip to P5 while P0–P2 exist — and a `hotspot` field on every scoreboard page (`fetch`/`css`/`cascade`/`box_tree`/`layout`/`paint`/`decode`) so a red page routes to a workstream. | We collided today: `layout/damage.rs` and `resolve.rs` were edited here while the other session had damage-flush and scroll-clamping work in flight on the same files. Their 90/10 rule — 90% accuracy and capability, 10% perf and infra — is also a fair criticism of where today's effort went |
| 9 | **The stacking-context and containing-block property cluster.** `is_stacking_context_root` checks opacity, position, transform and isolation; its own TODOs name `mix-blend-mode`, `filter`, `clip-path`, `mask`, `contain` — **all of which create a stacking context per spec, and three of which we already paint.** `establishes_containing_block` checks transform/translate/rotate/scale; its own TODO names `filter`, `backdrop-filter`, `will-change`, `contain`, `perspective`. | **Today's `isolation` bug is a family, not a one-off.** We paint a blur or a clip and then order it as an ordinary box, so content is drawn and painted over — the exact 24x.ai symptom. Two functions, one afternoon, and the test pattern already exists in `isolation_stacking_context.rs`. Highest conformance per hour on any list here, and it needs nothing from fastrender: both TODOs are already in our source. [TODO-fastrender-what-we-can-learn.md](TODO-fastrender-what-we-can-learn.md) §5b |
| 10 | **Noted, not scheduled**: fragmentation and pagination (none), a real hit-test module, a static accessibility tree, colour fonts (COLR/sbix — emoji render as tofu; check Parley first), CSP, `meta refresh`, CSS transitions (Stylo parses them, nothing drives them), MathML. | Real gaps, none blocking a browser that renders sites today. Listed so they are not rediscovered |

Habits worth taking whatever else happens: a CI grep for conflict markers (we shipped one
into a test file today), rendering under an address-space cap so a runaway page fails fast
instead of swapping the machine, and per-page `notes` in the scoreboard so a red cell is
triaged rather than rediscovered.

One thing they confirm rather than teach: their paint keeps stacking context and fixed
containing block strictly separate and never reparents a fixed node. That is the same
separation as `ps-blitz` PR #7, arrived at independently — but **their shape is cleaner**,
because our second offset pass exists only to undo our own hoist.

## The sites that have to work

Eight live sites in `~/code`, all Pathscale's, all SolidJS, and seven of the eight built
on `@pathscale/ui`. That makes them one target rather than eight: a defect in the shared
component library or its Tailwind output shows up on every one of them, and a fix does
too. They are the corpus this browser should be measured against, ahead of the open web.

| Site | Stack | Standing |
|---|---|---|
| [24x.ai](../../24x.ai) | `@pathscale/ui`, wss-adapter | **Renders, roughly 80 percent.** The measured reference, see the table above |
| [pathscale.com](../../pathscale.com) | `@pathscale/ui`, wss-adapter | **Blocked on `customElements`**, which needs shadow DOM. Two errors further than this morning |
| [nofilter.io](../../nofilter.io) | Cloudflare speedtest, ui-css-purge | Untested, and expected to be the heaviest: a speed test is timers, workers and network in a loop |
| [support.cafe](../../support.cafe) | felte, zod | Untested. Forms, so it exercises input, validation and submit |
| [honey.id](../../honey.id) | modular-forms, `@pathscale/ui` | Untested. Auth, so it exercises storage and navigation |
| [promptsyntax.org](../../promptsyntax.org) | `@pathscale/ui`, solid router | Untested. Client-side routing |
| [worktables.dev](../../worktables.dev) | `@pathscale/ui`, solid router | Untested. Client-side routing |
| [js.software](../../js.software) | the `@pathscale/ui` kitchen sink | Untested, and the most valuable: it is the component showcase, so one page covers the library |

**Do this as one pass**, not eight: capture each with `CHUZZ_CAPTURE_SCALE=2` and
`CHUZZ_CAPTURE_TREE`, collect the console errors, and sort the failures by how many sites
share them. `js.software` first, because a showcase of every component in the library is
the cheapest way to find which components do not render at all.

The census in each tree dump makes this cheap to compare: node counts, elements without a
layout box, and distinct-versus-total attribute values, per site.

### What is shared with AgencyZero, and what only looks shared

The layout cache is the one workstream both repositories carry. It is shared as research
and as an item, and **not shared in the build**:

- The evidence is one document, `agencyzero/docs/layout-caching-prior-art.md`: taffy's
  0.12.0 changelog documents its own cache regression as a deliberate 10 to 60 percent
  trade for correctness, and Yoga, Chromium and Gecko each solve it better.
- AgencyZero's `docs/TODO.md` item 7 is the same item, and its session reports it half
  done: bumped to 0.13.0 with a green suite, A/B measurement not taken, four call sites
  changed including an upstream correctness fix to `grid_template_areas`.
- `~/code/ps-taffy` is the fork, at 0.13.0, published as `ps-taffy` and imported as
  `taffy`. **AgencyZero resolves it; we do not.** Its lockfile carries `ps-taffy` with no
  `source` field, meaning a path dependency, reached through the `[patch]` in its
  `.cargo/config.toml` that redirects ps-blitz to `~/code/ps-blitz-render`. Our
  `Cargo.lock` has zero references to it.

So a source change in the fork reaches AgencyZero on their next build and reaches this
browser never, until item 3 above is done. A measurement they take, by contrast, is
knowledge and transfers for free.

## Then

| # | Item | Gated on | Detail |
|---|---|---|---|
| 6 | Gate animation ticks on visibility, so offscreen animations stop driving full-window redraws | none | [animation-gap.md](animation-gap.md) item 2 |
| 7 | Honest snapshot flags, then narrow the restyle hints | item 5, plus the taffy measurement | [TODO-dom-related-work.md](TODO-dom-related-work.md) items 1b, 1c |
| 8 | Narrow `ALL_DAMAGE` on the mutation paths | the taffy measurement | [TODO-dom-related-work.md](TODO-dom-related-work.md) item 3 |
| 9 | Shared attribute data and rare-data storage | item 4 first | [TODO-dom-related-work.md](TODO-dom-related-work.md) item 5 |
| 10 | Pending invalidations: batch instead of invalidating at the mutation site | none | [TODO-dom-related-work.md](TODO-dom-related-work.md) item 2 |

The full DOM plan with verification steps and sizes is
[TODO-dom-related-work.md](TODO-dom-related-work.md).

## Web platform gaps, recorded

Not scheduled, but they bound what "works" means for a browser:

- **Shadow DOM is `todo!()`** at `blitz-dom/src/stylo.rs:224` and `:231`, and `:300`
  returns `None`. Any page using web components renders with the wrong tree, and it panics
  rather than degrades. 24x.ai does not use them, which is the only reason this is not
  higher. [blink-for-a-browser.md](blink-for-a-browser.md) section 1
- **No `MutationObserver`, no `customElements`.** `blitz-script`'s DOM surface is 33
  methods. [small-things-to-learn-from-aurora.md](small-things-to-learn-from-aurora.md)
- **Hand-written, partial web API shims.** `apps/chuzz/src/document_loader.rs:58`, whose
  `URL` implementation says in its own comment that it is not WHATWG conformant.
- **No fixture corpus.** `capture.rs` renders headlessly and `dump.rs` writes the laid-out
  tree; the gap is the corpus and the comparison, not the renderer. For a program whose
  input is the entire web, unit tests answer a much narrower question than "does this page
  still render".
- **No in-app updater**, so an installed copy moves forward with
  `brew reinstall --cask chuzz`. [distribution.md](distribution.md)

## Open research

- **JavaScript engine.** Boa has no rope strings. Brimstone has cons strings, is
  unpublished on crates.io and self-describes as not production ready. The bindings, not
  the engine, are the project: `blitz-script` is 6,659 lines against Boa's API. Revisit
  when Brimstone publishes. `agencyzero/docs/js-engine-big-problem.md`
- **Damage regions.** The paint-side plan, which applies here unchanged.
  `agencyzero/docs/partial-paint.md`
- **Per-document health.** Aurora carries `healthy` and `consecutive_panics` beside its
  document so a failing page degrades instead of retrying forever. Natural companion to the
  Stylo panic guard.
  [small-things-to-learn-from-aurora.md](small-things-to-learn-from-aurora.md)

## Reference: what each document covers

| Document | Covers |
|---|---|
| [HANDOVER-24x-rendering.md](HANDOVER-24x-rendering.md) | the eight measured failures, isolation, how to diagnose |
| [distribution.md](distribution.md) | the bundle, the release pipeline, the cask, signing |
| [blink-for-a-browser.md](blink-for-a-browser.md) | shadow DOM, invalidation, DOM storage, whitespace |
| [TODO-dom-related-work.md](TODO-dom-related-work.md) | the DOM plan in full, theirs versus ours |
| [animation-gap.md](animation-gap.md) | why a page's animations pin the process |
| [small-things-to-learn-from-aurora.md](small-things-to-learn-from-aurora.md) | the other Blitz-based browser |
| [TODO-fastrender-what-we-can-learn.md](TODO-fastrender-what-we-can-learn.md) | fastrender file by file, the licence gate, what to build |
| [TODO-audit-us-vs-fastrender-vs-chrome.md](TODO-audit-us-vs-fastrender-vs-chrome.md) | **the full audit**: every section measured against Chrome, with the ranked list |
| [TODO-plan-what-exists-in-open-source.md](TODO-plan-what-exists-in-open-source.md) | **the plan**: which crate closes each gap, and the four phases |
| [TODO-sandbox-plan.md](TODO-sandbox-plan.md) | **security**: the live `file:` read, what Chrome and Firefox do, and why we do neither |

AgencyZero has a parallel set at `agencyzero/docs/`, including its own `TODO.md`, the
measured `performance.md`, and the renderer research this repository inherits.
