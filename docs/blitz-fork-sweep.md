# Sweep of every Blitz fork, patch by patch

Written 2026-08-11. Compares `pathscale/ps-blitz` master against `DioxusLabs/blitz` main and
against every fork of it that has commits of its own. **Nothing here was applied or run**;
this is a reading of commit contents and trees.

The headline: **`git rev-list --left-right --count origin/main...pathscale/master` is
`74 59`.** Upstream holds 74 commits we do not have, and three of our open TODO items are
answered by them.

## The engine landscape

Of the 100 most recently pushed forks of `DioxusLabs/blitz`, all but eight are tracking
copies with zero commits of their own. The eight, and what they carry:

| Fork | Ahead | Content |
|---|---|---|
| `gterzian/blitz` | 2 | Cross-process embedded element, for [formal-web](formal-web-what-to-learn.md)'s content/graphics split. Touches inline layout, damage, paint |
| `markjacksoncerberus/blitz` | 7 | `<iframe>` as a replaced element with a 300x150 default object size, SVG `currentColor` resolved against the element's computed colour, `ResourceKind` tagging for CSP, clearing `content_size` wherever the layout cache is cleared. Commit messages include "idk what these are, yolo push" and "asdf" |
| `UMCEKO/blitz` | 6 | `display: contents` box-generation transparency, and overlay scrollbar painting with hover, active and thumb dragging. Both are upstream PRs (#453, #461) that have not landed |
| `notdanilo/blitz` | 1 | Container scroll bounds, factoring child layout heights |
| `Klemen2/blitz` | 1 | Fix double redraw on resize |
| `FastTrackStudios`, `varappdev`, `OmentaElvis` | 1 each | Event converter reset, renderer options struct mutation, a merge commit |

Nothing there is worth taking wholesale. The two worth a look are `Klemen2`'s double
redraw, since we have just been working on redraw scheduling, and `UMCEKO`'s overlay
scrollbars if scrollbar polish ever matters.

**`ps-blitz` is by far the largest divergence in the ecosystem**, at 59 commits. The next
largest is 7, and it contains the word "yolo".

## Upstream commits that answer a TODO item

### 1. The fixture corpus, which we wrote down as missing

Our [TODO.md](TODO.md) says "no fixture corpus... the gap is the corpus and the
comparison, not the renderer", and [formal-web-what-to-learn.md](formal-web-what-to-learn.md)
notes that formal-web solved it by vendoring WPT.

**Upstream Blitz already runs WPT.** Not vendored: a runner, in-repo, with CI.

- `wpt/runner/` with `src/main.rs`, `net_provider.rs`, `report.rs`, `panic_backtrace.rs`,
  and `src/test_runners/` for attribute tests, crash tests and more
- `.github/workflows/wpt.yml`, `wpt-post-results.yml`, and `.github/scripts/wpt_diff_to_pr.py`,
  so a pull request gets a diff of what it broke
- Eight commits of runner work we do not have: crashtest support (#666), testharness
  classification (#668), mismatch reftests and multiple `rel=match` links (#665), excluding
  manual tests and `notref` references (#667), skipping non-UTF-8 files instead of crashing
  (#659), handling missing ref files gracefully (#660), measuring `data-offset-x/y` from the
  CSSOM View offset parent (#683), and pinning the WPT commit tested against (#567)

Plus `packages/blitz-test-harness/` (#586) with `harness.rs`, `input.rs` and `inspect.rs`,
and integration tests moved to `tests/blitz-tests`.

**This is the single most valuable thing in the sweep.** We do not need to build a corpus
or a comparison. We need to stop being 74 commits behind the engine that has both.

### 2. The DOM storage work our census was measuring for

The census added today counts, on Hacker News, 457 class attributes holding 19 distinct
values, and two thirds of elements carrying a `final_layout` with no box. Four upstream
commits are that workstream:

- `5833d681` Use SlotMap instead of Slab to back the node tree
- `ed9cd35d` Box `ElementData` within `NodeData`
- `c66a9cf6` Use `ThinVec` for Node lists
- `0b637cfb` Move Node fields to `ElementData` and `DocumentData`

That is the shape [TODO-dom-related-work.md](TODO-dom-related-work.md) item 5 describes,
already written.

### 3. The `incremental` feature flag we carry a local hack for

`ed82c163` removes the `incremental` feature flag and makes it a runtime setting on
`DocumentConfig` (#599). The preserved working tree we reconciled this morning carried a
local `incremental_layout: cfg!(feature = "incremental")` line doing the same thing by
hand. Upstream did it properly and we did not take it.

### 4. A devtools inspector, where we hand-rolled a dump

`813c496b` adds DOM and paint support for remote devtools inspectors (#574), and
`ee6580bc` adds inline fragment rects for highlighting inline spans (#569). We wrote
`apps/chuzz/src/dump.rs` this morning for want of exactly this. The dump is still useful,
being diffable text rather than a protocol, but the engine-side support is there.

## Crashes, which matter more for a browser than for an app

Five upstream fixes turn a panic or a segfault into an error. A page that panics takes the
whole browser down, and we cannot fix the page.

| Commit | What |
|---|---|
| `85c5c077` | Documents without a root element no longer panic (#655) |
| `7fff21ae` | Panic when a selector deposits parent flags on the Document (#648) |
| `062769a1` | `@font-face` sources with unresolvable urls no longer panic (#616) |
| `1093b86c` | Non-UTF-8 files skipped rather than crashing |
| `7dcf159d` | Segfault on window close under Wayland, from the renderer surface outliving the window (#611) |

And four correctness fixes in the mutation and invalidation paths, which is where our own
[TODO-dom-related-work.md](TODO-dom-related-work.md) lives:

- `7167d7bd` Keep the `get_element_by_id` index in sync with id attribute mutations (#682)
- `8c7cfd91` Fix an inner HTML fragment root leak (#681)
- `e6b1e399` Clear the document flag when moving nodes out (#585)
- `b846f548` Fix style mutation layout invalidation (#582), and `eb932a7b` request a redraw
  after programmatic DOM mutations (#580)

## Rendering correctness we have been working around

- **`99e036db` implements `<iframe>`** (#635): a new `blitz-dom/src/iframe.rs` at 184
  lines, plus config, document, mutator, net and html-sink changes. Chuzz mounts each tab
  as a sub-document through `web-view`, so this is adjacent rather than identical, but a
  browser needs real iframes and this is 416 lines of them.
- **Replaced element sizing**, four commits: generalised intrinsic sizing for canvas,
  video, embed and iframe (#607), SVG intrinsic sizing for replaced elements (#606),
  replaced flex item measurement (#605), and `item_is_replaced` propagated to Taffy (#639).
  We hand-patched `restore_svg_attribute_case` for a related symptom.
- **Presentational attributes**, four commits: the full mapping per spec (#662),
  `img`/`object` width and height (#643), `iframe`/`embed`/`video` width and height (#609),
  and the `dir` attribute mapped with selectors the engine can parse (#650). A page setting
  `width="600"` on an image is not exotic.
- Float and inline layout: floats no longer split the inline flow (#629), no margin
  collapse across an atomic inline boundary (#641), relative inset offsets on atomic inline
  boxes (#637), static position of block-level abspos in inline layout (#608), duplicate
  table cell padding with floats (#675), `calc()` cell widths through to Taffy (#653),
  block-axis `align-content` (#674).
- Focus and input: focusability kept in step with its attributes (#620), Shift+Tab moves
  focus backwards (#617), the focus outline given a style so it is actually painted (#551).

## Correction: this sweep missed the open pull requests

Written first, and wrong. It compared `main` against forks and never enumerated upstream's
own **85 open PRs and its branches**, because PR refs live in `refs/pull/*` and are not
fetched by default, so `git branch -a` shows nothing. Three consequences:

- **`position: fixed` *is* fixed upstream**, on PR #549, "Stop `position: fixed` resolving
  against a positioned ancestor": one commit, +252/-0, purely additive, with its own tests.
  PR #578 does it more thoroughly and more invasively. This sweep originally said the
  defect stayed ours.
- **`blitz-script` is not ours.** It is Nico Burns's, from PR #491 "JavaScript support",
  open as a draft since 2026-07-05. Our fork branched off it and added 50 commits. The
  first two commits in the package are his, which `git log --format=%an` would have shown.
- **Shadow DOM exists**, on branch `devin/1782520416-shadow-dom-custom-elements`, one
  commit, 205 behind `main`. That is the wall pathscale.com hits.

`backdrop-filter` really is absent, being in the anyrender backends rather than in Blitz.

## Two we already have by another route

- `e6c13c4e` Upgrade to Taffy v0.13 (#625). We arrived at 0.13 through our own bump this
  morning.
- `114ae6c2` Upgrade to Stylo 0.20. Both trees declare `stylo 0.20.0`.

That is worth noticing for what it says about the divergence: we are re-doing upstream's
work independently, and finding out afterwards.

## What this argues for

Not a fourth fork, and not a merge in one go. The 59 commits we carry are real: the script
surface pages actually hit, SVG, macOS input, and the frame diagnostics that measured
120.0fps to 29.8fps this afternoon. They are also the reason nobody has rebased.

The tractable order, given the WPT runner exists on the other side:

1. Take the WPT runner and `blitz-test-harness` first, because after that every subsequent
   merge has a pass or fail attached instead of a hope.
2. Then the five crash fixes, which are small and independent.
3. Then the four DOM storage commits, measured against the census.
4. Then decide about iframes, replaced-element sizing and presentational attributes, which
   are the largest behavioural gap and the least urgent for the eight sites in
   [TODO.md](TODO.md).

## Related

- [formal-web-what-to-learn.md](formal-web-what-to-learn.md), the third fork, reviewed in
  its own right.
- [TODO.md](TODO.md), whose fixture-corpus, DOM-storage and crash items this bears on.
