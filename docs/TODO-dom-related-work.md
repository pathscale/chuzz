# TODO: DOM-related work

Written 2026-08-11. A plan, not a design doc: each item says what to change, where, why,
what it depends on, and how to tell it worked.

**Engine paths here are `blitz-rust/packages/blitz-dom/src/...`**, the checkout this
repository builds against by path. AgencyZero patches in a *different* checkout
(`ps-blitz-render`) whose line numbers differ, and keeps its own list at
`agencyzero/docs/TODO-dom-related-work.md`. **A fix in one tree does not reach the other.**
Items marked ENGINE need landing in both, or the two checkouts need converging first.

Nothing below is measured. Numbers cited come from AgencyZero's `docs/performance.md`,
2026-08-10, measured on that app rather than on a browser workload.

## Read first

1. **Item 0 is not like the others.** Shadow DOM is a correctness gap: pages render wrong,
   and the code path panics rather than degrades. Everything else on this list is
   performance or memory.
2. **Snapshot correctness before hint narrowing.** Item 1a must land before 1c, or a
   performance fix becomes stale styles.
3. **The Taffy layout cache gates every speed number.** See
   `agencyzero/docs/layout-caching-prior-art.md`. Nothing here promises milliseconds until
   that is addressed.

---

## 0. Shadow DOM is `todo!()` ENGINE, correctness

| | Theirs | Ours |
|---|---|---|
| Treated as core | Blink `core/dom/README.md` is 834 lines, mostly shadow trees, tree scopes, the composed tree, the flat tree, slots, slot assignment recalc, `FlatTreeTraversal`, event retargeting | none of it exists |
| Shadow root host | implemented | `stylo.rs:224` `todo!("Shadow roots not implemented")` |
| Shadow root style data | implemented | `stylo.rs:231` `todo!("Shadow roots not implemented")` |
| Element to shadow root | implemented | `stylo.rs:300` `as_shadow_root` returns `None`, `// TODO: implement shadow DOM` |
| Script surface | `attachShadow` | absent from `blitz-script` |

- **What:** size the gap before anything else on this list. Decide between implementing it
  in the engine and doing what the other Blitz-based browser did: Aurora carries
  `src/dom/shadow.rs` (20 KB) plus a 138 KB custom-elements polyfill.
- **Why:** any site using custom elements with shadow roots renders with the wrong tree.
  Light DOM children never slot, shadow stylesheets never apply, `:host` and `::slotted`
  match nothing. It fails silently, and the `todo!()`s panic if reached.
- **Depends on:** nothing.
- **Verify:** a fixture page using a web component; today it should render visibly wrong.
- **Size:** large. See [blink-for-a-browser.md](blink-for-a-browser.md) section 1 and
  [small-things-to-learn-from-aurora.md](small-things-to-learn-from-aurora.md).

---

## 1. Invalidation sets: stop overriding the ones we already ship

Stylo implements Blink's invalidation-set design, `blitz-dom` already calls it every frame,
and then overrides the result twice. **A browser is hit harder than an app**: real pages
carry thousands of selectors from stylesheets we did not write, so the gap between
"restyle what this class can affect" and "restyle the subtree" is much wider.

| | Theirs | Ours |
|---|---|---|
| The design | Blink `invalidation_set.h:63` (`.y .z {}` invalidates only `.z` descendants), `style-invalidation.md:10` | n/a |
| Same design in Rust | Stylo `invalidation/element/invalidation_map.rs:318`, `class_to_selector:321`, `id_to_selector:324`, `other_attribute_affecting_selectors:330` | compiled into our binary today |
| Processor | Stylo `invalidation/element/state_and_attributes.rs:282`, `:314` | reached via `stylo.rs:77` `process_style(root, Some(&self.snapshots))` |
| Snapshot input | Gecko supplies per-element snapshots | `document.rs:1232` `snapshot_node` |
| Override 1 | n/a | `document.rs:1294-1296`: `class_changed: true, id_changed: true, other_attributes_changed: true`, unconditionally |
| Override 2 | n/a | `mutator.rs:243` and `:252`: `restyle_subtree()` on node **and parent** |

### 1a. Make snapshots updatable ENGINE

- **Where:** `document.rs:1252`, the `// Do nothing / TODO: update snapshot` branch.
- **What:** merge the newly changed attribute into an existing snapshot. Today the first
  snapshot of an element in a frame wins and later mutations are lost.
- **Why:** correctness. Also the reason override 2 cannot simply be deleted.
- **Depends on:** nothing.
- **Verify:** a test mutating two attributes on one element in one frame, asserting both
  land in the snapshot.
- **Size:** small.

### 1b. Set the changed flags honestly ENGINE

- **Where:** `document.rs:1294-1296` and its callers, chiefly `mutator.rs:236`
  `set_attribute`.
- **What:** pass the `QualName` being written into `snapshot_node` and set only the matching
  flag. `set_attribute` already has it and discards it.
- **Why:** those flags are what Stylo reads at `state_and_attributes.rs:282` and `:314` to
  choose which invalidation maps to walk. All-true forces the union of every map on every
  mutation.
- **Depends on:** 1a.
- **Verify:** distinct-node count per restyle on a real page.
- **Size:** small to medium.

### 1c. Narrow the restyle hints ENGINE

- **Where:** `mutator.rs:252` (parent) first, then `:243` (node).
- **What:** drop the parent's `restyle_subtree()`, then reduce the node's own hint where the
  snapshot path covers it.
- **Why:** a hint is a floor, not a ceiling; it is unioned on top of whatever the
  invalidator computed and wins.
- **Depends on:** 1a **and** 1b. AgencyZero already measured this step done alone: 18.17 ms
  against 18.90 ms, noise, because the snapshot still claimed everything changed.
- **Size:** small, once 1a and 1b are in.

Full reasoning: `agencyzero/docs/style-invalidation-we-already-ship.md`, and
[blink-for-a-browser.md](blink-for-a-browser.md) section 2 against our line numbers.

---

## 2. Pending invalidations: batch instead of invalidating at the mutation site ENGINE

| | Theirs | Ours |
|---|---|---|
| Model | Blink `style-invalidation.md:134`: record into a `PendingInvalidationsMap` (`:92`), push only when style is read (`:155`), because "there may be more mutations coming" | every mutation does its own ancestor walk inline |
| Sites | n/a | `mutator.rs:174` `set_node_text`, `:236` `set_attribute`, `:545` `add_children_to_parent` |

- **Why:** a page that mutates many nodes in one script turn pays one ancestor walk per
  mutation before anything reads style.
- **Size:** medium.

---

## 3. Narrow `ALL_DAMAGE` on the mutation paths ENGINE

| | Theirs | Ours |
|---|---|---|
| Granularity | Blink separates style, layout, prepaint and paint invalidation as lifecycle phases (`document_lifecycle.h:47-79`) | `ALL_DAMAGE` is `0b0111_1111` at `layout/damage.rs:30`, including `CONSTRUCT_BOX`, `CONSTRUCT_FC`, `CONSTRUCT_DESCENDENT` |
| Text change | content cannot change the parent's box construction | `mutator.rs:174` inserts `ALL_DAMAGE` on the text node **and the parent** |

- **What:** `RELAYOUT` without `CONSTRUCT_BOX` for a text-content change.
- **Why:** defensible on spec grounds, not only as an optimisation.
- **Depends on:** the Taffy cache work for any measurable payoff.
- **Size:** small.

---

## 4. `cloneNode(true)` is a naive recursive deep copy ENGINE

| | Theirs | Ours |
|---|---|---|
| Attribute storage | Blink `element_data.h:172` shares identical attribute sets via `ElementDataCache`, copy-on-write at `:219`, attributes inline at `:192` | `document.rs:856` `deep_clone_node`: per node a `data.clone()`, a `children.clone()`, a `Vec<usize>` collect, then recursion |

- **Why:** frameworks on real pages clone templates per row. Several allocations per node
  per instantiation, with no structural sharing.
- **Size:** medium.

---

## 5. DOM storage: shared attributes and rare data ENGINE

| | Theirs | Ours |
|---|---|---|
| Shared attribute sets | `element_data.h:172`: "very common for many elements to have duplicate sets of attributes (ex. the same classes)" | `node/attributes.rs:7` `Attribute { name, value: String }`, `:15` a plain `Vec<Attribute>`. One owned `String` per attribute per element |
| Rare fields in a side table | `node_rare_data.h:108`, "sparse storage of fields for Node and Element" | `node/node.rs:91` carries everything inline for every node; `node/element.rs:35` puts `background_images` and `mask_images` on every element |

- **What:** measure distinct versus total attribute values on a real page first.
- **Why:** a page with a thousand table rows carrying identical `class` attributes allocates
  a thousand copies of the same string. This scales with every page loaded, unlike an app
  with one stylesheet.
- **Verify:** `chuzz-control` already exposes diagnostics; this is a counter, not a project.
- **Size:** measurement small, fix medium.

---

## 6. The whitespace tax on the DOM slab ENGINE

| | Theirs | Ours |
|---|---|---|
| Rationale | `WhitespaceLayoutObjects.md:8`: avoid layout objects for insignificant whitespace "to save memory, and save CPU by having fewer layout objects to traverse" | we already do the layout half: `layout/construct.rs:296`, `:318` |
| The gap | n/a | `resolve.rs:98` iterates the **DOM slab**, not the layout tree, so every whitespace text node costs a `Node` struct and a visit every frame |

- **Why:** hand-written HTML is full of whitespace text nodes, and we keep all of them.
- **Verify:** total DOM node count against layout node count on a real page.
- **Size:** measurement trivial.

---

## 7. Class lists are re-split on every query ENGINE, low priority

| | Theirs | Ours |
|---|---|---|
| Parsed once | Blink stores parsed class names on element data (`element_data.h:54`) | `stylo.rs:514` `has_class` and `:633` `each_class` both call `split_ascii_whitespace()` on every Stylo query |

**Caveat:** AgencyZero measured the whole style phase at 167 microseconds and ruled selector
matching out. That measurement is from an app with one Tailwind sheet, not from a browser
with real-world CSS, so it may not transfer. Worth re-measuring here before acting, and
worth not assuming either way.

---

## 8. Browser gaps outside the DOM, recorded so they are not lost

Not DOM work, but adjacent and already identified:

- **Web API shims are hand-written and partial.** `apps/chuzz/src/document_loader.rs:58`
  `WEB_API_SHIM` covers `localStorage`, `sessionStorage` and a `URL` whose own comment
  says it is "not a WHATWG-conformant implementation".
- **No `MutationObserver`, no `customElements`.** `blitz-script`'s DOM surface is 33
  methods. See [small-things-to-learn-from-aurora.md](small-things-to-learn-from-aurora.md).
- **No fixture corpus.** `capture.rs` already renders headlessly; the gap is the corpus and
  the comparison, not the renderer.

---

## Suggested order

1. **Item 0.** Size the shadow DOM gap. It changes what "works" means for whole categories
   of site, and nothing else on this list matters if pages render wrong.
2. Items 5 and 6 measurements. One counter each, and they size real per-page costs.
3. Item 1a. Correctness, and it unblocks the rest of item 1.
4. Taffy cache work, which gates any speed number from 1b, 1c and 3.
5. Items 1b, 1c, then 3.
6. Items 2 and 4 when convenient.
7. Item 7 only after re-measuring on a browser workload.

## Related

- [blink-for-a-browser.md](blink-for-a-browser.md), items 0, 1, 5, 6.
- [small-things-to-learn-from-aurora.md](small-things-to-learn-from-aurora.md), items 0 and 8.
- `agencyzero/docs/style-invalidation-we-already-ship.md`, item 1 in full.
- `agencyzero/docs/layout-caching-prior-art.md`, the gate on most of this.
- `agencyzero/docs/blink-what-we-can-learn.md`, items 2, 3, 5, 6, 7.
