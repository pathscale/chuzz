# mega-blitz: one engine tree, and what the node should look like

Written 2026-08-11. The plan for `~/code/blitz-mega`, branch `mega-blitz`, a worktree of
`~/code/blitz-rust`. Working there rather than in the main checkout so the running browser
and the other session are untouched.

Background for every claim here: [blitz-fork-sweep.md](blitz-fork-sweep.md) for what each
fork holds, [formal-web-what-to-learn.md](formal-web-what-to-learn.md) for the third fork,
and the census in `apps/chuzz/src/dump.rs` for the measurements.

## Why not a rebase

Tried and abandoned at 24 of 59 commits. Upstream's `5833d681` replaced the Slab with a
SlotMap, so node ids became a versioned `NodeId(u64)` where ours are `usize`, and
`NodeId` deliberately has no `From<usize>`. Our 40 commits touching `blitz-dom` and
`blitz-script` are written against `usize`, so the replay is a type migration performed
blind, with no compile until the end and 391 `usize` occurrences across 36 files to judge
by eye.

Worse, the automated conflict resolution I started with takes one side wholesale, which in
files where upstream changed types silently discards their half. A branch built that way
compiles at best and is wrong at worst.

**The one fact that makes this tractable: `main` has no `blitz-script`.** It never
conflicts with `main`; it only needs porting.

**It is not ours, though.** It is Nico Burns's, from
[PR #491 "JavaScript support"](https://github.com/DioxusLabs/blitz/pull/491), open as a
draft since 2026-07-05: 8 commits, +5,511 lines across 35 files. Our fork branched off it
at `60d840176` and added 50 commits on top. The first two commits in the package are his,
which one `git log --format=%an` would have shown and which the first version of this plan
did not check.

It matters because #491 has moved since we forked: it now carries a fix for a panic when
the selection anchor node is removed from the DOM, plus DOM API inventories for React and
the WPT harness. Knowing the package has an upstream home means watching that branch for
fixes rather than writing them again.

**We do not send anything upstream.** Everything lands on `pathscale/ps-blitz`. Pulling
from upstream is one-directional for now, and the fixes we find in their code, such as the
`absolute_position` scroll bug below, stay ours until someone decides otherwise.

## The plan

Each step ends with something that compiles.

| # | Step | State |
|---|---|---|
| 1 | Reset `mega-blitz` to `origin/main`, build it | |
| 2 | Bring `packages/blitz-script` across whole, port `usize` to `NodeId` against the compiler | |
| 3 | Replay our unique work in themed chunks: the script surface, SVG, macOS input, frame diagnostics | |
| 4 | Drop what is obsolete: the Python-free Stylo fork (upstream is on registry Stylo 0.20, and our own later commits went back to it), the reverted SVG rasterisation, the namespacing chores | |
| 5 | Take the queued upstream PRs, below | #549 **done** |
| 6 | Cherry-pick from other forks: `Klemen2`'s double-redraw-on-resize, `UMCEKO`'s overlay scrollbars | |

## The upstream PRs and branches to pull in

Upstream has **85 open PRs**. The first version of this plan missed all of them, having
swept forks and merged history only; PR refs live in `refs/pull/*` and are not fetched by
default. These bear on defects we have recorded:

| Source | What | State |
|---|---|---|
| **PR #491** `js-engine` | `blitz-script` itself, and its later commits we do not have | Base of our port; its newer commits still to take |
| **PR #549** | `position: fixed` resolving against the viewport, not a positioned ancestor. One commit, +252/-0, purely additive, own tests | **Done.** Cherry-picked, 4 tests pass, suite green at 277 |
| **PR #578** | The same defect, more thoroughly: containing-block-aware abspos, 5 commits, +725/-125 across 12 files, includes a local taffy patch | Consider after #549 settles |
| **branch `devin/1782520416-shadow-dom-custom-elements`** | Shadow DOM and custom elements. One commit, 205 behind `main` | Queued. This is pathscale.com's wall |
| **PR #583** | `position: sticky` via paint-time scroll-clamped offsets, +263/-11, mergeable clean | Queued |
| **PR #497** | Programmatic and animated scrolling | Queued |
| **PR #601, #614** (`Klemen2`) | Windows redraw when requested; winit upgrade | Low priority, we are macOS |

## The order of work, unambiguously

1. **Merge upstream.** Gets decision 1 below, and half of decision 3, for free.
2. **Split layout off the node.** The other half of decision 3: the 12 MB, and the thing no
   fork has done.
3. **Test `SmallVec` against `ThinVec`.** Decision 2. Small and independent of 1 and 2.
4. **bumpalo for per-frame scratch.** Decision 4. A separate workstream that touches none
   of the above.

Attribute sharing sits outside this list entirely and is unclaimed by every fork; see the
last section.

## The node tree: four decisions, two of them open

The confusion worth avoiding is that these are independent. Only 2 and 3 are open.

| # | Decision | Ours | Upstream | Blink | Verdict |
|---|---|---|---|---|---|
| 1 | How nodes are addressed | Slab + `usize` | SlotMap + versioned `NodeId(u64)` | GC pointers | **Settled: take upstream** |
| 2 | How children are stored | `Vec<usize>` | `ThinVec<NodeId>` | prev/next sibling pointers | **Open** |
| 3 | What lives on the node | all inline, **1,600 bytes** | `ElementData` boxed off | rare fields in a side table, layout in a **separate tree** | **Open, biggest win** |
| 4 | Bump allocation | none | none | n/a | **Not the node tree.** Per-frame scratch |

### Measurements these rest on

Taken with `CHUZZ_CAPTURE_TREE` on three pages, and consistent across all three:

| | Wikipedia | Hacker News | 24x.ai |
|---|---|---|---|
| Nodes | 7,667 | 1,303 | 149 |
| Leaves, no children | 50% | 48% | 52% |
| Exactly one child | 39% | 40% | 23% |
| Mean fanout | 1.00 | 1.00 | 0.99 |
| Max fanout | 147 | 92 | 8 |
| Elements with a layout box | 34% | 32% | 81% |
| Distinct class values | 22% | **4%** (19 of 457) | 75% |

And `size_of`, measured in our tree: `Node` **1,600**, `Cache` 368, `Layout` 84 (twice per
node), `NodeData` 208, `ElementData` 200.

So one Wikipedia page is roughly 12 MB of node array, and over half of it is text leaves
carrying a Taffy cache and two `Layout`s they never use.

## Decision 1: settled

SlotMap *is* an arena, a flat Vec of slots exactly like our Slab, plus a generation
counter. It costs about 4 bytes per node and one compare per lookup, and removes id-reuse
aliasing: an id outliving its node and silently addressing whatever took the slot. Not
theoretical, and not only an engine problem: the same bug shape killed the browser this
morning through `dioxus-stores`, where a positional lens outlived the row it addressed.

Nothing to decide. It arrives with step 1.

## Decision 2: `SmallVec` against `ThinVec`, open and cheap

`ThinVec` is 8 bytes on the node but heap-allocates for **any** non-empty list. With 40% of
nodes holding exactly one child, that is roughly 3,800 allocations on one Wikipedia page
that `SmallVec<[NodeId; 2]>` would make zero, at a cost of about 24 more bytes per node.
Against 1,600 bytes that is noise.

**Inferred, not measured.** The measurement is an afternoon: swap the type, count
allocations and resident size on the same three pages.

There is a third option nobody in the ecosystem has taken, which is Blink's: no child list
at all, just `previous_` and `next_` sibling pointers. With a mean fanout of 1.00 that is
strictly less memory again, at the cost of every indexed child access becoming a walk.

## Decision 3: split layout off the node

The largest win available, and the one no Blitz fork has taken. Upstream took the first
step by boxing `ElementData` off `NodeData`; Blink took the whole one, keeping layout in a
separate `LayoutObject` tree so that a node generating no box has no layout object at all.

For us that is `Cache` at 368 bytes, `unrounded_layout` and `final_layout` at 84 each, and
`Style<Atom>`, none of which a text node or a `display: none` element needs. 66% of
elements on a content page have no box.

This is a design change, not a merge, and it should follow step 3 rather than complicate
it.

## Decision 4: bumpalo, and where it does not go

Bump allocators cannot free individually, so they are wrong for a DOM that `removeChild`
mutates over a tab's lifetime: a page churning 100k nodes would hold all of them until
navigation. `typed-arena` additionally hands out `&'arena T`, which fights aliasing for a
tree script mutates, and pushes every field into a `Cell`, which is what indices exist to
avoid.

They are right for what is allocated and discarded wholesale, which is exactly the thrash
`agencyzero/docs/allocations.md` is about: per-frame paint and layout temporaries, per-parse
scratch. Reset is a pointer store and there are no frees at all. A separate workstream from
everything above.

## Also not the node tree, and also unclaimed

Attribute sharing. Blink's `ShareableElementData` with an `ElementDataCache`, copy-on-write
to `UniqueElementData` only when an element mutates its attributes. Hacker News holds 457
class attributes with **19 distinct values**, and we allocate a fresh `String` for each.
Neither we nor upstream nor any fork does this.
