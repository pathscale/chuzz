# Plan: what already exists, and what we actually have to build

Written 2026-08-11. Companion to
[TODO-audit-us-vs-fastrender-vs-chrome.md](TODO-audit-us-vs-fastrender-vs-chrome.md),
which says what is missing. This says **how to get it**, and the answer is usually not
"write it".

## The thesis

fastrender wrote 57MB of Rust from scratch — their own cascade, their own text stack, their
own flex and grid, their own JS engine. We compose: Stylo, Parley, Taffy, Boa, vello,
reqwest. **That is the advantage, not the compromise**, and this plan is about pressing it.

Every gap in the audit sorts into one of three buckets, and the first two are most of them:

| | Bucket | Count |
|---|---|---|
| **A** | Already in our tree, switched off | 3 |
| **B** | A crate we already depend on, or one `cargo add` away | 7 |
| **C** | Genuinely ours to build | the rest |

---

## Bucket A — already ours, switched off. Do these first, they are nearly free

### A1. The HTTP disk cache already exists and is disabled

`packages/blitz-net/Cargo.toml` has a `cache` feature wiring `http-cache-reqwest`,
`http-cache` (with `manager-cacache`), `reqwest-middleware` and `directories`. It is fully
implemented.

`chuzz/apps/chuzz/Cargo.toml:41` enables `["http2", "cookies", "compression"]`. **Not
`cache`.**

So the audit's "disk cache: MISSING, medium build" was wrong. It is **one word**, and it
makes every pageset run and every navigation faster. Verify the cache directory lands
somewhere sane via `directories`, then ship it.

### A2. `parallel-construct` is built but never compiled in

Eight `cfg` sites and a `rayon` import in `blitz-dom/src/resolve.rs`, and the feature is
absent from `blitz-dom`'s `default` list. Either put it in `default` and measure it, or
delete it. Carrying an unbuilt parallel path is the worst of both.

### A3. `StyleThreading::Parallel` is the default and documented as unsafe for us

`blitz-dom/src/config.rs:13-18` warns two concurrent `Document` resolves can panic with
`already mutably borrowed` ([#430](https://github.com/DioxusLabs/blitz/issues/430)). chuzz
never overrides it and `iframe.rs:71` propagates it to sub-documents. Ten minutes to prove
unreachable or set `Sequential`.

---

## Bucket B — a crate away

| Gap | What exists | In our tree already? | Work left |
|---|---|---|---|
| **`fetch` / `Request` / `Response` / `Headers`** | `reqwest` 0.13.4, `http` 1.4.2 | **yes, both** | **only the JS binding.** The transport, TLS, redirects, compression and cookies are already solved and already shipping in `blitz-net` |
| **`URL` / `URLSearchParams`** | `url` 2.5.8 (Servo's, the same parser Chrome-adjacent engines use) | **yes** | replace chuzz's JS-string shim with a real binding. The parser is already there |
| **`TextEncoder` / `TextDecoder`** | `encoding_rs` 0.8.35 (Gecko's) | **yes** | a thin binding; the hard part, encoding sniffing and conversion, is done |
| **Benches** | `criterion` 0.8.2 (updated 2026-02) | no | `cargo add --dev criterion`, then write the benches |
| **Fuzzing** | `cargo-fuzz` + `arbitrary` + `libfuzzer-sys` | no | targets for the HTML and CSS parsers |
| **Image diff for the scoreboard** | `image-compare` 0.5.0, or `dssim` 3.4.0 for a perceptual metric | `image` yes, in chuzz | pick one; `dssim` if we want a single number that tracks human judgement |
| **Chrome baseline capture** | `chrome --headless --screenshot`, or `chromiumoxide` for control | no | a shell script is enough to start |

**The `fetch` point deserves emphasis.** The audit called it "large, unavoidable" and that
was measured against fastrender's 598KB `window_fetch.rs`. But they were writing an HTTP
stack. We already ship one: `blitz-net` wraps `reqwest` with TLS, redirects, compression,
cookies and now caching. What is missing is the *binding* — `Request`/`Response`/`Headers`
objects over Boa, plumbed to the existing loader. That is a different and much smaller job,
and it is why bucket B matters.

---

## Bucket C — genuinely ours

No crate does these, because they are engine semantics rather than infrastructure.

| Gap | Why no crate helps | Size |
|---|---|---|
| **Stacking-context property cluster** | two functions in `blitz-dom`, and the TODOs are already written in our source | one afternoon |
| **`backdrop-filter` in the vello backends** | we own `~/code/ps-anyrender`; `anyrender_skia` already implements it, the vello ones take `_backdrop_filter` and drop it | small |
| **ES modules** | Boa already has `core/engine/src/module/` with a loader — we simply never call it. The work is routing `type="module"` through `Module` parse with a loader resolving against the document base URL | medium |
| **`getComputedStyle`** | Stylo computes it; we expose it. Purely our plumbing | small |
| **Cookie scoping on `fetch`** | not CORS, not CSP. `blitz-net/src/lib.rs:77` sets `cookie_store(true)`, one jar for the whole process. The fetch spec's own `credentials: "same-origin"` default is the fix | **a few lines** |
| **Display list** | a design decision about our paint architecture | large, three payoffs |
| **`srcset` / `sizes` / `<picture>`** | ~200 lines of parsing and selection against the spec | small |
| **`structuredClone`, `Blob`/`File`/`FormData`, `ResizeObserver`** | web semantics over our own DOM | each small |
| **Colour fonts** | `skrifa` 0.44 is in our graph and does read COLR; the constraint is downstream in `vello_cpu` **0.0.9**, which is very early. Check what it can draw before planning anything | unknown — measure first |

### Sandboxing is bucket C, reluctantly

`birdcage` 0.8.1 is the only cross-platform embeddable option (Linux Landlock + macOS
Seatbelt), and it was **last updated 2024-04**. `extrasafe` 0.5.1 is Linux-only, same
vintage. `ErickJ3/sandbox-rs` is MIT and well-built but **Linux-only and archived**:
kernel 5.10+, Landlock, seccomp, namespaces, cgroups, and nothing for macOS, which is what
we actually ship.

So for macOS: a Seatbelt profile applied through `sandbox_init`. It is a deprecated API and
still what everything uses, the profile is tens of lines, and **the probe matters more than
the profile** — fastrender ships three separate probe binaries whose only job is asserting
at runtime that the sandbox is on. Without one it silently becomes a no-op after any
refactor.

---

## The plan, in order

Ordered so each phase makes the next cheaper or safer.

### Phase 0 — free wins, today

1. Turn on `blitz-net`'s `cache` feature in chuzz (**A1**)
2. Resolve `parallel-construct`: default and measure, or delete (**A2**)
3. Resolve `StyleThreading` (**A3**)
4. Fix the stacking-context cluster — `filter`, `clip-path`, `mask` are already painted (**C**)
5. `backdrop-filter` in `anyrender_vello_cpu` (**C**)

Nothing here needs a new dependency, and items 4 and 5 are the two biggest visual defects
we know of.

### Phase 1 — make quality measurable

6. Pageset scoreboard: cached HTML, Chrome baseline from that cache, `image-compare` or
   `dssim` for the diff, per-page JSON with `status`/`stages_ms`/`hotspot`/`notes`
7. `criterion` benches for resolve, layout and paint, plus `perf.yml`

Until this exists every later item is unmeasured. It is also what makes phase 0's fixes
provable rather than asserted.

### Phase 2 — the JS surface, in a safe order

8. Move chuzz's nine JS shims down into `blitz-script` as real bindings with tests
9. `URL`/`URLSearchParams` on the `url` crate; `TextEncoder`/`TextDecoder` on `encoding_rs`
10. `getComputedStyle`, `queueMicrotask`, `structuredClone`, `AbortController`
11. ES modules through Boa's existing loader
12. `fetch`, bound to the existing `blitz-net` loader, **defaulting to
    `credentials: "same-origin"`**

An earlier draft of this plan put "same-origin policy and CORS" ahead of `fetch` and called
the ordering non-negotiable. That was over-engineering, and it conflated three unrelated
things:

- **CSP** protects a *site* from its own XSS. It is a service to page authors, enforced by
  the browser as a courtesy. For a browser rendering our own sites it buys us nothing.
  **Cut.**
- **CORS** is not a protection. It is the protocol by which a server *relaxes* same-origin.
  Without same-origin it is meaningless, and as a standalone item it is worthless. **Cut.**
- **Same-origin policy** is the real thing, and it only becomes load-bearing when someone
  browses untrusted sites in chuzz with credentials. That is a "before we tell anyone this
  is their browser" item, not a "before `fetch`" one.

What survives is small and concrete. `blitz-net` sets `cookie_store(true)` — **one cookie
jar for the whole process** — and chuzz enables `cookies`. So a page that can `fetch` any
origin gets that origin's cookies attached and reads the response back. The fix is not a
subsystem: it is the fetch spec's own default, `credentials: "same-origin"`, applied where
we build the binding. Do that, and ship `fetch`.

### Phase 3 — platform and hardening

14. macOS Seatbelt profile **and a probe that fails loudly**
15. `srcset`/`sizes`/`<picture>`, `text-shadow`
16. Fuzz targets for the HTML and CSS parsers

Same-origin policy proper, and CSP, belong here **only if chuzz stops being a tool for
rendering our own sites**. Until then they are cost without a threat model.

### Phase 4 — architecture

18. A display list, then damage regions, parallel paint and occlusion culling on top
19. Colour fonts, after measuring what `vello_cpu` can actually draw
20. Fragmentation, multicol, form submission, a real hit-test module

---

## What this plan refuses to do

- **Fork Boa.** Our pin is 34 commits behind, Boa has `import.meta` and a module loader,
  and every JS gap is host-side. A fork buys nothing and costs us upstream.
- **Replace Stylo.** It is Gecko's engine and it computed `isolation` correctly; we never
  read it. Our gap is downstream of Stylo, in the 35 accessors we call.
- **Write our own HTTP stack, URL parser or encoding layer.** They are in the tree already.
- **Copy from fastrender.** It carries no licence.
