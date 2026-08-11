# TODO: what to work on, and what is still research

Written 2026-08-11. A high-level index. Every line points at the document carrying the
evidence and line numbers. **Read the linked doc before acting**, because several items
carry ordering constraints or caveats that do not survive a one-line summary.

Everything here is source reading, not measurement. No performance number in this
repository has been measured; the figures quoted in the linked docs come from AgencyZero's
`performance.md`, taken on that app rather than on a browser workload.

## The active task

**Make Chuzz load and render 24x.ai correctly.**
[HANDOVER-24x-rendering.md](HANDOVER-24x-rendering.md) is the working document: isolation
contract, disk limits, known engine gaps, headless diagnosis, and an anti-yield contract
for the failure mode the previous attempt hit.

Everything below is secondary to that until it lands.

## Two constraints that govern everything

1. **Engine changes land in `~/code/blitz-rust`**, which AgencyZero does not build for its
   main app but which `agencyzero/apps/blitz-preview` path-depends on. Items marked ENGINE
   in either repository need landing in both trees, or the trees need converging.
2. **Correctness before performance.** One item on this list makes pages render *wrong*.
   The rest make them render slowly. Do not reorder that.

## Do next

| # | Item | Detail |
|---|---|---|
| 1 | **Size the shadow DOM gap.** `stylo.rs:224` and `:231` are `todo!()`, `:300` returns `None`. Any page using web components renders with the wrong tree, and the `todo!()`s panic rather than degrade. Decide between implementing it and polyfilling as Aurora did. | [blink-for-a-browser.md](blink-for-a-browser.md) section 1 |
| 2 | Clamp animation-driven redraw (`blitz-shell/src/window.rs:614`). For a browser this is a defense, not an optimisation: an arbitrary page's animation pins the process at full frame rate and we cannot fix the page. | [animation-gap.md](animation-gap.md) |
| 3 | Two counters: DOM node count against layout node count, and distinct versus total attribute values on a real page. Both size per-page costs that scale with everything loaded. | [blink-for-a-browser.md](blink-for-a-browser.md) section 3 |
| 4 | Make Stylo snapshots updatable (`document.rs:1252`). A correctness fix, and it unblocks the invalidation work. | [TODO-dom-related-work.md](TODO-dom-related-work.md) item 1a |

## Then

| # | Item | Gated on | Detail |
|---|---|---|---|
| 5 | Gate animation ticks on visibility, so offscreen animations stop driving full-window redraws | none | [animation-gap.md](animation-gap.md) item 2 |
| 6 | Honest snapshot flags, then narrow the restyle hints | item 4, plus the Taffy cache work | [TODO-dom-related-work.md](TODO-dom-related-work.md) items 1b, 1c |
| 7 | Narrow `ALL_DAMAGE` on the mutation paths | Taffy cache work | [TODO-dom-related-work.md](TODO-dom-related-work.md) item 3 |
| 8 | Shared attribute data and rare-data storage | item 3 first | [TODO-dom-related-work.md](TODO-dom-related-work.md) item 5 |
| 9 | Pending invalidations: batch instead of invalidating at the mutation site | none | [TODO-dom-related-work.md](TODO-dom-related-work.md) item 2 |

The full DOM plan with verification steps and sizes is
[TODO-dom-related-work.md](TODO-dom-related-work.md).

## Web platform gaps, recorded

Not scheduled, but they bound what "works" means for a browser:

- **No `MutationObserver`, no `customElements`.** `blitz-script`'s DOM surface is 33
  methods. [small-things-to-learn-from-aurora.md](small-things-to-learn-from-aurora.md)
- **Hand-written, partial web API shims.** `apps/chuzz/src/document_loader.rs:58`, whose
  `URL` implementation says in its own comment that it is not WHATWG conformant.
- **No fixture corpus.** `capture.rs` already renders headlessly; the gap is the corpus and
  the comparison, not the renderer. For a program whose input is the entire web, unit tests
  answer a much narrower question than "does this page still render".

## Open research

- **JavaScript engine.** Boa has no rope strings. Brimstone has cons strings, is
  unpublished on crates.io and self-describes as not production ready. The bindings, not
  the engine, are the project: `blitz-script` is 6,659 lines against Boa's API. Revisit
  when Brimstone publishes. `agencyzero/docs/js-engine-big-problem.md`
- **Layout cache.** Taffy's 0.12.0 changelog documents its own cache regression as a
  deliberate 10 to 60 percent trade for correctness, and we run 0.12.2. Yoga, Chromium and
  Gecko each solve it better. 0.13.0 has not been A/B tested.
  `agencyzero/docs/layout-caching-prior-art.md`
- **Damage regions.** The paint-side plan, which applies here unchanged.
  `agencyzero/docs/partial-paint.md`
- **Per-document health.** Aurora carries `healthy` and `consecutive_panics` beside its
  document so a failing page degrades instead of retrying forever. Natural companion to the
  Stylo panic guard.
  [small-things-to-learn-from-aurora.md](small-things-to-learn-from-aurora.md)

## Reference: what each document covers

| Document | Covers |
|---|---|
| [HANDOVER-24x-rendering.md](HANDOVER-24x-rendering.md) | the active task, isolation, disk, diagnosis |
| [blink-for-a-browser.md](blink-for-a-browser.md) | shadow DOM, invalidation, DOM storage, whitespace |
| [TODO-dom-related-work.md](TODO-dom-related-work.md) | the DOM plan in full, theirs versus ours |
| [animation-gap.md](animation-gap.md) | why a page's animations pin the process |
| [small-things-to-learn-from-aurora.md](small-things-to-learn-from-aurora.md) | the other Blitz-based browser |

AgencyZero has a parallel set at `agencyzero/docs/`, including its own `TODO.md`, the
measured `performance.md`, and the renderer research this repository inherits.
