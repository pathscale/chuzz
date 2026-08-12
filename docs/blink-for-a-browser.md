# Blink, read as a browser: what Chuzz is missing

Written 2026-08-11, from reading Blink at
[`7e6a84f`](https://github.com/chromium/chromium/tree/7e6a84f5165fd617dbf3d032f755e11804bf8ff6),
Stylo 0.20.0 in the local cargo registry, and the `ps-blitz` checkout this repository
builds against. All line numbers below are from **our** checkout at
`../ps-blitz/packages/blitz-dom/src/`, which differs slightly from the sibling
`ps-blitz-render` tree. **Nothing here was measured or built.**

Chuzz is 3,522 lines of browser policy on top of Blitz, so almost everything here is an
engine finding rather than an app finding. That is the point: the gaps that matter for a
browser are in the engine, and knowing which ones are missing is what tells you whether a
page will render wrong or just slowly.

Three findings, in order of how badly they bite a browser.

## 1. Shadow DOM is not implemented, and that is a rendering correctness gap

**What Blink treats as core.** `core/dom/README.md` is 834 lines and the majority of it is
shadow trees: shadow roots, tree scopes, the composed tree, the **flat tree**, slots and
node assignment, slot assignment recalc, `FlatTreeTraversal`, and event path retargeting.
For a browser this is not an optional feature. It is how selectors, layout order and event
targeting work on any page built with web components.

**Ours.** The Stylo integration declares shadow roots and then refuses:

```rust
// ps-blitz/packages/blitz-dom/src/stylo.rs:224 and :231
fn host(&self) -> ... { todo!("Shadow roots not implemented") }
fn style_data<'b>(&self) -> ... { todo!("Shadow roots not implemented") }
```

and `as_shadow_root` returns `None` unconditionally (`stylo.rs:300`, with
`// TODO: implement shadow DOM`). `blitz-script` exposes no `attachShadow`, so a page
cannot create one either.

**What this costs a browser.** Any site using custom elements with shadow roots renders
with the wrong tree: light DOM children never get slotted, shadow stylesheets never apply,
and `::slotted` and `:host` match nothing. This is not a slow path, it is a wrong result,
and it fails silently. For comparison, [Aurora](https://github.com/JohannaWeb/Aurora),
another Blitz-based browser, carries a 20 KB `src/dom/shadow.rs` plus a 138 KB custom
elements polyfill precisely because this gap is unavoidable for real pages.

**Action.** This is the largest single engine gap between Chuzz and a browser that renders
the modern web. It is worth sizing before anything on this list, because it changes what
"works" means for whole categories of site. The `todo!()`s are load-bearing: they will
panic, not degrade, if anything ever reaches them.

## 2. We ship Stylo's invalidation sets and override them twice

This one is shared with AgencyZero and is written up at length in
`agencyzero/docs/style-invalidation-we-already-ship.md`. It is repeated here because the
line numbers differ in our checkout and because **a browser is hit far harder by it**: a
real page has thousands of selectors from stylesheets we did not write, so the difference
between "restyle what this class can affect" and "restyle the subtree" is much larger than
in an app with one Tailwind sheet.

**Blink's design.** Precompute from the stylesheet which elements a class or id change can
affect: for `.y .z {}`, class `y` invalidates only `.z` descendants
([`invalidation_set.h:63`](https://github.com/chromium/chromium/blob/7e6a84f5165fd617dbf3d032f755e11804bf8ff6/third_party/blink/renderer/core/css/invalidation/invalidation_set.h#L63)),
with `wholeSubtreeInvalid` as the explicit fallback. Their doc names the alternative:
"The simplest possible approach is to invalidate everything in response to every change"
([`style-invalidation.md:10`](https://github.com/chromium/chromium/blob/7e6a84f5165fd617dbf3d032f755e11804bf8ff6/third_party/blink/renderer/core/css/style-invalidation.md#L10)).

**Stylo already implements it**, because Gecko drives it the same way:

| Piece | Stylo 0.20.0 |
|---|---|
| `pub struct InvalidationMap` | `invalidation/element/invalidation_map.rs:318` |
| `class_to_selector` / `id_to_selector` | `invalidation_map.rs:321` / `:324` |
| `other_attribute_affecting_selectors` | `invalidation_map.rs:330` |
| Snapshot-driven processor | `invalidation/element/state_and_attributes.rs:282`, `:314` |
| Entry point | `invalidation/stylesheets.rs:252` |

**And we already call it.** `ps-blitz/packages/blitz-dom/src/stylo.rs:77`:

```rust
self.stylist.flush(&guards).process_style(root, Some(&self.snapshots));
```

**Then we override it twice:**

- `snapshot_node` sets `class_changed: true`, `id_changed: true` and
  `other_attributes_changed: true` unconditionally
  (`ps-blitz/packages/blitz-dom/src/document.rs:1294-1296`). Those are the exact flags
  `state_and_attributes.rs:282` and `:314` read to choose which maps to walk, so every
  mutation forces the union of all of them.
- `set_attribute` adds `RestyleHint::restyle_subtree()` on the node
  (`mutator.rs:243`) and again on the parent (`mutator.rs:252`), under a standing
  `// TODO: make this fine grained`.

**And you cannot just delete the sledgehammer.** `document.rs:1252` reads:

```rust
if let Some(_existing_snapshot) = self.snapshots.get_mut(&opaque_node_id) {
    // Do nothing
    // TODO: update snapshot
}
```

The first snapshot of an element in a frame wins and later mutations to it are not
recorded. The subtree hint is currently what covers that gap. Removing it first turns a
performance problem into stale styles.

**Ordered fix:** make snapshots updatable, then set the changed flags honestly from the
attribute actually being written, then narrow the hints, starting with the parent's.

## 3. DOM storage: attribute sharing and the whitespace tax

Two memory findings from `core/dom` that matter more to a browser than to an app, because
a browser loads pages it did not author.

### Attribute sets are shared and copy-on-write in Blink

`ShareableElementData` is produced by the parser for elements with identical attributes and
managed by an `ElementDataCache`
([`element_data.h:172`](https://github.com/chromium/chromium/blob/7e6a84f5165fd617dbf3d032f755e11804bf8ff6/third_party/blink/renderer/core/dom/element_data.h#L172)):

> This is a memory optimization since it's very common for many elements to have duplicate
> sets of attributes (ex. the same classes).

Attributes live inline after the object rather than in a separate allocation
([`:192`](https://github.com/chromium/chromium/blob/7e6a84f5165fd617dbf3d032f755e11804bf8ff6/third_party/blink/renderer/core/dom/element_data.h#L192)),
and a `UniqueElementData` is created only when an element actually mutates
([`:219`](https://github.com/chromium/chromium/blob/7e6a84f5165fd617dbf3d032f755e11804bf8ff6/third_party/blink/renderer/core/dom/element_data.h#L219)).

**Ours.** `Attribute` is `{ name: QualName, value: String }` and `Attributes` wraps a plain
`Vec<Attribute>` (`ps-blitz/packages/blitz-dom/src/node/attributes.rs`). Every element
owns a separate heap `String` per attribute, with no sharing and no copy-on-write. A page
with a thousand table rows carrying identical `class` attributes allocates a thousand
copies of the same string.

### Whitespace: we do the layout half and pay for the DOM half

Blink avoids creating layout objects for insignificant whitespace "to save memory, and save
CPU by having fewer layout objects to traverse"
([`WhitespaceLayoutObjects.md:8`](https://github.com/chromium/chromium/blob/7e6a84f5165fd617dbf3d032f755e11804bf8ff6/third_party/blink/renderer/core/dom/WhitespaceLayoutObjects.md#L8)).

We already do that for the layout tree: whitespace is filtered out of layout children in
`blitz-dom/src/layout/construct.rs`. But the per-frame damage-clear loop in
`blitz-dom/src/resolve.rs` iterates the **DOM slab**, not the layout tree, so every
whitespace text node in a page still costs a full `Node` struct and a visit on every frame.
Hand-written HTML is full of them.

**Cheap measurement:** compare total DOM node count against the count that survives into
the layout tree on a real page. `chuzz-control` already exposes diagnostics, so this is a
counter, not a project.

## What this list is not

Not included, deliberately:

- **Layout caching.** Covered against Taffy, Yoga, Chromium, Gecko, Servo, Slint and
  Masonry in `agencyzero/docs/layout-caching-prior-art.md`. It is engine-wide and applies
  here unchanged.
- **Compositor animations, paint result caching, the document lifecycle.** Reviewed in
  `agencyzero/docs/blink-what-we-can-learn.md`. They matter to a browser too, but they are
  renderer-layer concerns and nothing in Chuzz's own 3,522 lines touches them.
- **V8 versus Boa.** Covered in `small-things-to-learn-from-aurora.md`. The engine swap is
  the small part; the DOM bindings are the project.

## Ranked for a browser

1. **Size the shadow DOM gap** (section 1). It is the only item here that makes pages
   render *wrong* rather than slowly, and the `todo!()`s panic rather than degrade.
2. **Make snapshots updatable** (section 2, step 1). A correctness fix on its own merits,
   independent of any performance argument.
3. **Count DOM nodes against layout nodes** (section 3). One counter, and it sizes a
   per-frame cost that scales with every page loaded.
4. **Honest snapshot flags, then narrower hints** (section 2, steps 2 and 3). Follows the
   layout-cache work, per the ordering in the AgencyZero docs.
5. **Shared attribute data** (section 3). Well-understood, self-contained, and it grows in
   value with page size.

## Related

- `agencyzero/docs/style-invalidation-we-already-ship.md` for section 2 in full, including
  why the earlier experiment that narrowed the parent hint measured nothing.
- `agencyzero/docs/blink-what-we-can-learn.md` for the renderer-layer parts of the same
  review.
- `agencyzero/docs/layout-caching-prior-art.md` for the layout cache, which gates the
  measurable payoff of section 2.
- [small-things-to-learn-from-aurora.md](small-things-to-learn-from-aurora.md) for the
  other Blitz-based browser, which hit the shadow DOM gap and solved it with polyfills.
