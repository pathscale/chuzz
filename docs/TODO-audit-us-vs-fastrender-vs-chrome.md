# Audit: us against fastrender against Chrome

Written 2026-08-11. A section-by-section audit of our tree, measured by reading and
grepping our own source, with fastrender as a second data point and Chrome as the
reference. Chrome's column is "what a page may assume exists"; it is the bar, not a
suggestion.

Companion to [TODO-fastrender-what-we-can-learn.md](TODO-fastrender-what-we-can-learn.md),
which covers what fastrender teaches. **This document is about what we are missing**,
regardless of source. Most of it needs nothing from anyone else.

Nothing in fastrender may be copied — it carries no licence. Everything here is a design
to build from spec.

Legend: **HAVE** · **PARTIAL** · **MISSING** · **N/A** (not our architecture).

---

## 1. JavaScript API surface — the largest gap in the tree

Measured by grepping `packages/blitz-script/src` and `chuzz/apps/chuzz/src/document_loader.rs`.

**We provide 12 globals.** fastrender has 30+ files in `src/js/vmjs/` alone. Chrome has
several hundred interfaces.

### What we have

| Where | What |
|---|---|
| `blitz-script` | `document`, `window`, `self`, `location`, `history`, `navigator` (+`clipboard`), `performance`, `customElements`, `crypto`, `ipc`, `setTimeout`/`clearTimeout`/`setInterval`/`clearInterval`, `requestAnimationFrame`/`cancelAnimationFrame` |
| `blitz-script` DOM protos | `node`, `element`, `document`, `event`, `style`, `custom_elements` |
| **chuzz shims** (`document_loader.rs`) | `IntersectionObserver`, `MutationObserver`, `URL`, `URLSearchParams`, `localStorage`, `sessionStorage`, `matchMedia`, `requestIdleCallback`/`cancelIdleCallback`, `screen` |
| Boa built-ins, free | `Promise`, `JSON`, `Math`, `Map`, `Set`, `Proxy`, `Symbol`, `Reflect`, typed arrays |

### What is missing

| API | Us | fastrender | Chrome | Why it matters |
|---|---|---|---|---|
| **`fetch` / `Request` / `Response` / `Headers`** | **MISSING** | `window_fetch.rs` 598KB | HAVE | **The single biggest hole.** Every SPA loads data with it. `grep fetch runtime.rs` is empty — a page has *no way to make a network request from JS* |
| **`getComputedStyle`** | **MISSING** | HAVE | HAVE | Ubiquitous. Any library that measures before animating calls it |
| **`ResizeObserver`** | **MISSING** | `window_resize_observer.rs` 33KB | HAVE | Modern component libraries depend on it; `@pathscale/ui` is a component library |
| `XMLHttpRequest` | MISSING | `window_xhr.rs` 139KB | HAVE | Older bundles and many polyfills still use it |
| `TextEncoder` / `TextDecoder` | MISSING | `window_text_encoding.rs` 121KB | HAVE | Common inside bundles |
| `AbortController` / `AbortSignal` | MISSING | `window_abort.rs` 41KB | HAVE | Pairs with `fetch`; cancelling is not optional |
| `structuredClone` | MISSING | `window_structured_clone.rs` 114KB | HAVE | |
| `queueMicrotask` | MISSING | HAVE | HAVE | Trivial to add on Boa's job queue |
| `Blob` / `File` / `FileReader` / `FormData` | MISSING | 4 files, ~148KB | HAVE | Any upload or download path |
| `WebSocket` | MISSING | `window_websocket.rs` 217KB | HAVE | |
| `Worker` | MISSING | `window_worker.rs` 51KB | HAVE | |
| `MessageChannel` / `BroadcastChannel` | MISSING | 2 files, 81KB | HAVE | |
| `DOMRect` | MISSING | `window_dom_rect.rs` 32KB | HAVE | `getBoundingClientRect` should return one |
| `Image` constructor | MISSING | HAVE | HAVE | `new Image()` preloading |
| Streams | MISSING | `window_streams.rs` 325KB | HAVE | Needed by a real `fetch` |
| `import.meta` | MISSING | HAVE | HAVE | Already TODO item 1: a module using it is a parse error, so the whole file dies |

### We do not support ES modules at all, and it is not Boa's fault

`blitz-script/src/document.rs:252` says it plainly: *"`module` scripts are treated as
classic scripts for now."* Every script goes through
`context.eval(Source::from_bytes(code))`, which parses as a **Script**, never a **Module**.

Consequences, in order of severity:

- **`import` and `export` are syntax errors.** A `<script type="module">` shipping
  untranspiled ESM — which is most modern sites — dies on its first line, and the whole
  file never runs.
- `import.meta` is a parse error, because it is only legal inside a module. [TODO.md](TODO.md)
  item 1 calls this *"the only one that is Boa's rather than ours"*. **That is wrong.**
  Boa implements it — `core/ast/src/expression/import_meta.rs` plus VM opcodes in
  `core/engine/src/vm/opcode/meta/` — and Boa ships a module loader at
  `core/engine/src/module/` with `loader/`, `namespace.rs`, `source.rs`, `synthetic.rs`.
  We never call any of it.
- Dynamic `import()`, import maps and module preloading follow from the same gap.

**So `import.meta` is the symptom and ES module support is the item.** Fixing it properly
means routing `type="module"` scripts through Boa's `Module` parse and loader, with a
loader that resolves against the document's base URL through `blitz-net`. That is real
work, but it is host work in `blitz-script`, and it unblocks a class of sites rather than
one expression.

### Structural problem: the shims are in the wrong repo

`IntersectionObserver`, `MutationObserver`, `URL`, `localStorage` and the rest are
**injected by chuzz**, not implemented in `blitz-script`. Consequences:

- they are invisible to `blitz-script`'s 40-odd tests, so nothing guards them
- any other embedder of the engine gets none of them
- they are JS strings inside a Rust app rather than typed bindings

**Move them down into `blitz-script`** as real bindings with tests. That is a mechanical
change and it makes the surface auditable, which it currently is not.

---

## 2. CSS: what Stylo computes and we throw away

Stylo is not the weak point — it is Gecko's engine and it computes these correctly. We read
**35 `clone_*` accessors and 16 struct fields** and drop the rest.

### The stacking-context cluster — six bugs of the shape we fixed today

`is_stacking_context_root` (`blitz-dom/src/node/node.rs`) checks opacity, position,
transform, isolation. Its own TODOs name the rest.

| Property | We paint it | Creates a stacking context | Status |
|---|---|---|---|
| `filter` | **yes** | yes | **not checked — bug** |
| `clip-path` | **yes** | yes | **not checked — bug** |
| `mask-image` | **yes** | yes | **not checked — bug** |
| `mix-blend-mode` | no | yes | bug + unimplemented |
| `contain` | no | yes | bug + unimplemented |
| `will-change` | no | yes | bug + unimplemented |

We paint a blur or a clip and then order it as an ordinary box: content drawn, then painted
over. Exactly 24x.ai's symptom.

`establishes_containing_block` (`blitz-dom/src/resolve.rs`) checks transform/translate/
rotate/scale; its own TODO names `filter`, `backdrop-filter`, `will-change`, `contain`,
`perspective`. It decides which `position: fixed` descendants get hoisted, so a fixed child
inside a filtered ancestor resolves against the wrong box.

### Properties never read at all

`text-shadow` (ubiquitous, pure paint, model it on `box_shadow.rs`), `perspective`,
`transform-style`, `backface-visibility`, `background-blend-mode`, `border-image-source`,
`accent-color`, `appearance`, `text-orientation`, `shape-outside`, `column-count`,
`column-gap`.

---

## 3. Paint

| | Us | fastrender | Chrome |
|---|---|---|---|
| `backdrop-filter` | computed and passed, **dropped by the vello backends** | HAVE, with backdrop-root semantics | HAVE |
| Display list | **MISSING entirely** | 2MB across builder/renderer/optimise | HAVE |
| Colour fonts (COLR/sbix/SVG-in-OT) | **MISSING** — emoji are tofu | `text/color_fonts/` | HAVE |
| SVG filters | PARTIAL | `svg_filter.rs` 358KB | HAVE |
| Parallel paint | **MISSING** | HAVE | HAVE |
| Occlusion culling | MISSING | `optimize.rs` 79KB | HAVE |

`backdrop-filter` is the cheapest: **we own `~/code/ps-anyrender`**, and
`anyrender_skia/src/scene.rs:450-453` already implements it. Only the vello backends take
`_backdrop_filter` and discard it.

The display list is the structural one — it is the prerequisite for damage regions,
parallel paint and occlusion culling, which is three items for the price of one.

---

## 4. Concurrency

| Stage | Us | fastrender | Chrome |
|---|---|---|---|
| Style | **Stylo parallel rayon traversal**, default on | own, threading unclear | parallel |
| Box construction | `parallel-construct` exists — **not in default features, so off** | — | — |
| Layout | single-threaded (Taffy) | `benches/layout_parallel.rs` | parallel |
| Display list build | N/A, we have none | **parallel** | parallel |
| Rasterisation | **single-threaded** — no `rayon`/`par_iter` anywhere in `blitz-paint` | **parallel tiling** | parallel |
| Render thread | on the event loop | `ui/render_worker.rs` 555KB | separate |
| Process model | single | multi, `crates/fastrender-{ipc,shmem}` | multi |

**Two findings that are ours alone:**

`parallel-construct` is **dead code** — eight `cfg` sites and a `rayon` import in
`resolve.rs`, and the feature is not in `blitz-dom`'s `default` list. Put it in `default`
and measure it, or delete it.

**Our one piece of parallelism is documented as unsafe for our own use case.**
`blitz-dom/src/config.rs:13-18` warns that two `Document`s resolving on
`StyleThreading::Parallel` concurrently can panic with `already mutably borrowed`
([upstream #430](https://github.com/DioxusLabs/blitz/issues/430)). `Parallel` is the
default, **chuzz never overrides it**, and `iframe.rs:71` propagates it to every
sub-document — and chuzz mounts each tab as a sub-document. Either prove it unreachable or
set `Sequential`.

---

## 5. HTML platform

| | Us | fastrender | Chrome |
|---|---|---|---|
| `srcset` / `sizes` / `<picture>` | **MISSING** | `html/image_attrs.rs` 52KB | HAVE |
| Content Security Policy | **MISSING** | `html/content_security_policy.rs` 46KB | HAVE |
| `<meta http-equiv="refresh">` | **MISSING** | `html/meta_refresh.rs` 44KB | HAVE |
| Image prefetch at parse time | **MISSING** | `html/image_prefetch.rs` 52KB | HAVE |
| Shadow DOM | HAVE (feature-gated, landed today) | HAVE | HAVE |
| Custom elements | PARTIAL — constructor body does not run | HAVE | HAVE |
| `<iframe>` | PARTIAL | HAVE | HAVE |

`srcset` is the one to take: ubiquitous, parsing plus selection rather than layout.

---

## 6. Layout

| | Us | fastrender | Chrome |
|---|---|---|---|
| Flex, grid | HAVE (Taffy) | own, 1.8MB | HAVE |
| Tables | HAVE | HAVE | HAVE |
| Floats | HAVE | HAVE + `float_shape.rs` | HAVE |
| **Fragmentation / pagination** | **MISSING** | 482KB + 253KB of tests | HAVE |
| **Multi-column** | **MISSING** | HAVE | HAVE |
| `shape-outside` | MISSING | HAVE | HAVE |
| Hit testing | PARTIAL — `hit()`, no module | `interaction/hit_test.rs` 62KB | HAVE |
| Form submission | **MISSING** | `interaction/form_submit.rs` 55KB | HAVE |

---

## 7. Networking and resources

| | Us | fastrender | Chrome |
|---|---|---|---|
| Disk cache | **MISSING** — `blitz-net` is 15KB, every load is cold | `disk_cache.rs` 368KB + index | HAVE |
| `fetch` from JS | **MISSING** | HAVE | HAVE |
| Cache-Control / ETag | MISSING | HAVE | HAVE |

---

## 8. Testing and CI

| | Us | fastrender |
|---|---|---|
| Behaviour tests | 313 | many |
| **Criterion benches** | **0** | **16** |
| **Fuzz targets** | **0** | **3** |
| Perf regression CI | **MISSING** | `perf.yml` |
| Accuracy diff in CI | **MISSING** | `chrome_fixture_diff.yml` |
| Page corpus + scoreboard | **MISSING** | `progress/pages/*.json` |
| WPT | behind on upstream's runner | own importer |

313 tests assert behaviour and **none assert cost**, while frame pacing went 120.0 to
29.8fps today and more perf work is landing from the other session.

---

## 9. Security — the section where we score zero

The other sections are gaps in capability. This one is different: **we ship a signed,
notarised, publicly installable `Chuzz.app` that loads arbitrary websites with no
containment of any kind.** Nothing here is theoretical.

### Sandboxing

| | Us | fastrender | Chrome |
|---|---|---|---|
| macOS | **NONE** | `sandbox/macos.rs` 58KB, `macos_spawn.rs` 23KB, + a `macos_sandbox_probe` binary and `docs/macos_sandbox.md` | Seatbelt per renderer |
| Linux | **NONE** | `sandbox/linux_seccomp.rs` 22KB | seccomp-bpf + namespaces |
| Windows | **NONE** | `sandbox/windows.rs` 88KB + a whole `crates/win-sandbox` (`renderer_sandbox.rs`, `spawn.rs`, probe) + `docs/windows_sandbox.md` 43KB | AppContainer + job objects + token restriction |
| Process separation | **NONE** — one process | `sandbox/spawn.rs` 42KB, `crates/fastrender-ipc`, `fastrender-shmem` | renderer per site, brokered IPC |
| Site isolation | **NONE** | none | per-site processes |
| Verification | **NONE** | `bin/macos_sandbox_probe`, `bin/_real/sandbox_probe.rs`, `scripts/trace_renderer_syscalls.sh`, `appcontainer_temp_smoke.rs` | continuous fuzzing + bug bounty |

They treat it as a first-class subsystem: **~360KB of sandbox code, 62KB of sandbox
documentation, and three separate probe binaries** whose only job is to assert at runtime
that the sandbox is actually on. That last part is the bit worth copying — a sandbox with
no probe silently becomes a no-op after any refactor.

### Content-level protections

| | Us | fastrender | Chrome |
|---|---|---|---|
| CSP | **MISSING** | `html/content_security_policy.rs` 46KB | HAVE |
| Mixed-content blocking | **MISSING** | — | HAVE |
| CORS | **MISSING** | partial, via `resource/web_fetch/` | HAVE |
| Same-origin policy | **MISSING** | partial | HAVE |
| Cookie scoping / `SameSite` | **MISSING** | — | HAVE |
| Subresource integrity | **MISSING** | — | HAVE |
| URL spoofing defences in chrome UI | unknown | — | HAVE |

**The honest reading**: fastrender is ahead of us on process containment and CSP and behind
Chrome on everything origin-related, because it is fundamentally a renderer with a browser
attached. We are behind both, at zero.

### What this actually means for chuzz today

A page we load can: read and write any file the user can, reach any host including
`localhost` and LAN addresses, and — because there is no same-origin policy — there is
nothing structural stopping content from one origin reaching another's data through
whatever APIs we add next. The mitigating fact is our small API surface (no `fetch`, no
storage beyond a shim), which is security by absence, and it disappears the moment we
implement §1.

**Order to fix, and this is why it is ordered this way:**

1. **A macOS Seatbelt profile plus a probe that fails loudly if it is not applied.** We are
   macOS-only in practice and shipping today. A profile is tens of lines; the probe is what
   keeps it real.
2. **`credentials: "same-origin"` on `fetch`, and nothing more than that.**
   `blitz-net/src/lib.rs:77` sets `cookie_store(true)` — one jar for the whole process — so
   a page that can fetch any origin gets that origin's cookies and reads the response. The
   fetch spec's own default fixes it in a few lines.
3. **Not CORS, and not CSP.** CORS is how a server *relaxes* same-origin, so without
   same-origin it is meaningless. CSP protects a site from its own XSS — a service to page
   authors, worth nothing to a browser rendering our own sites. Both are cost without a
   threat model until chuzz is used for general browsing.
4. Same-origin policy proper, then process separation — when chuzz stops being a tool for
   rendering our own sites. A rearchitecture, not a fix.

---

## The ranked list

Ordered by value per hour, using fastrender's own triage order (panics → accuracy →
hotspots → polish) as the tiebreak.

| # | Item | Section | Size |
|---|---|---|---|
| 1 | **The stacking-context / containing-block property cluster** — `filter`, `clip-path`, `mask` are already painted and just need checking; then `will-change`, `contain`, `perspective` | §2 | one afternoon, test pattern exists |
| 2 | **`backdrop-filter` in `anyrender_vello_cpu`, then `anyrender_vello`** — we own the fork, skia already does it | §3 | small |
| 3 | **ES module support.** `type="module"` is executed as a classic script, so `import`/`export` are syntax errors and any untranspiled ESM bundle dies on line one. Boa already has the module loader; we never call it | §1 | medium, unblocks a class of sites |
| 4 | **Same-origin policy + CORS, then `fetch` / `Request` / `Response` / `Headers`.** In that order — shipping `fetch` with no origin model turns a missing feature into a vulnerability | §1, §9 | large, unavoidable, coupled |
| 4b | **macOS Seatbelt profile + a probe that fails loudly if it is not applied** | §9 | small, and we ship today |
| 5 | **`getComputedStyle`** | §1 | small, ubiquitous |
| 6 | **Move chuzz's JS shims down into `blitz-script`** with tests | §1 | mechanical |
| 7 | **Pageset scoreboard** + Chrome baseline from cached HTML | §8 | medium, unlocks everything else |
| 8 | **`StyleThreading` audit** — prove it unreachable or set `Sequential` | §4 | ten minutes, potential P0 |
| 9 | **`parallel-construct`: default it and measure, or delete it** | §4 | ten minutes |
| 10 | **`srcset` / `sizes` / `<picture>`** | §5 | small |
| 11 | **`text-shadow`** — pure paint, model on `box_shadow.rs` | §2 | small |
| 12 | **Disk cache in `blitz-net`** | §7 | medium |
| 13 | **A display list**, then parallel paint and damage regions on top | §3 | large, three payoffs |
| 14 | **`ResizeObserver`**, `AbortController`, `TextEncoder`/`TextDecoder`, `queueMicrotask`, `structuredClone` | §1 | each small |
| 15 | **Perf regression bench + `perf.yml`** | §8 | medium |
| 16 | **CSP**, once there is a resource-loading choke point to enforce it at | §9 | medium |
| 17 | **Fuzz targets** for the HTML and CSS parsers | §8 | small |
| 18 | Fragmentation, multicol, form submission, hit-test module, colour fonts | §3, §6 | large, none blocking today |
