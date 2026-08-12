# The animation gap, for a browser

Written 2026-08-11, from reading the `ps-blitz` checkout this repository builds against,
Blink at
[`7e6a84f`](https://github.com/chromium/chromium/tree/7e6a84f5165fd617dbf3d032f755e11804bf8ff6),
and a survey of the Rust animation crate ecosystem. Engine line numbers are from
`ps-blitz/packages/...`, which differs from AgencyZero's `ps-blitz-render` checkout.
**Nothing here was measured.**

AgencyZero has the same engine finding at `agencyzero/docs/animation-gap.md`. **The
difference between the two repositories is the whole point of this document**, so it is
stated first.

## Why this is worse for a browser than for an app

AgencyZero authors every animation it runs. If one is too expensive, it can gate it on
focus, drop it, or write it differently. That is what its composer ring already does.

**Chuzz authors none of them.** Every animation it runs arrives in someone else's
stylesheet, and there is no negotiating with it. A spinner, a pulsing badge, a marquee, an
infinite `animation: fade 2s infinite` on a cookie banner: each one pins the process at
full frame rate with full-window repaints, for as long as the tab is open.

Two facts make that worse than it sounds:

1. **The flag is document-wide.** `has_active_animations` is computed as
   `sets.values().any(|state| state.needs_animation_ticks())`
   (`ps-blitz/packages/blitz-dom/src/stylo.rs:171`), and `is_animating()`
   (`document.rs:1640` area) ORs it with canvas, subdocuments, custom widgets and scroll.
   One animating element anywhere makes the entire document animating.
2. **There is no viewport or offscreen gating in this path.** An animation on an element
   scrolled thousands of pixels out of view still ticks and still drives full-window
   redraws. Whether Stylo cancels animations on `display: none` elements is a separate
   question and was not verified here; the viewport case definitely is not handled.

For an app, the frame clamp below is an optimisation. For a browser it is the only defense.

## What is missing, and what is not

**Not missing: an animation engine.** Stylo runs CSS animations and transitions.
`DocumentAnimationSet` is at `blitz-dom/src/document.rs`, ticked in `resolve_stylist`,
with `has_active_animations` set at `stylo.rs:171`. Pages animate correctly today. That is
the problem, not the gap.

**Missing: a cheap frame.**

| Step | Where |
|---|---|
| Any animation sets one document-wide boolean | `blitz-dom/src/document.rs` `is_animating()` |
| The shell then requests the next frame unconditionally | `blitz-shell/src/window.rs:614` |
| Each frame repaints the whole visible tree | `blitz-paint/src/render.rs:113` |
| and re-renders and presents the whole window | `anyrender_vello/src/window_renderer.rs:423` |

Blink answers this with `CompositorAnimations::CheckCanStartAnimationOnCompositor`
([`compositor_animations.h:146`](https://github.com/chromium/chromium/blob/7e6a84f5165fd617dbf3d032f755e11804bf8ff6/third_party/blink/renderer/core/animation/compositor_animations.h#L146)),
a bitfield of 21 enumerated reasons an animation cannot be accelerated, plus a compositor
to run the survivors on. We have a boolean.

## Do not reach for an animation library

Recorded because it is the obvious wrong move, and because it is worse here than intuition
suggests.

Stylo's CSS animation path already sets the narrowest possible hint on animating nodes:

```rust
// ps-blitz/packages/blitz-dom/src/stylo.rs:100
self.nodes[node_id].set_restyle_hint(RestyleHint::RESTYLE_SELF);
```

A JavaScript-driven animation, whether from a page's own `requestAnimationFrame` loop or
from a library, writes style through `set_attribute` (`mutator.rs:236`), which sets
`restyle_subtree()` on the node (`:243`) **and again on the parent** (`:252`).

So a page that animates in JavaScript already costs us more than one that animates in CSS,
and we cannot choose which the page does. That is another argument for item 1 rather than
against it.

Surveyed and rejected as inapplicable: [animato](https://github.com/AarambhDevHub/animato)
("Animato never renders", a tweener), [rust-animation](https://github.com/joone/rust-animation)
(a scene graph with its own wgpu renderer), and the crates.io animation category generally,
which is entirely value interpolation or other engines' animation systems. Nothing in Rust
does layerization, property trees or damage tracking as a reusable library, and the C++
equivalents (Chromium `cc`, Blink `PaintLayer`, Core Animation) are not extractable. ThorVG,
rlottie and Rive play animation **content** and are a different subject.

## TODO

### 1. Clamp animation-driven redraw to a lower cadence ENGINE, defensive DONE

**Done 2026-08-11, and measured: 120.0fps to 29.8fps on a CSS spinner.**

A page with `animation: turn 1s linear infinite` on one 120px element ran the whole
window at the display's rate, 120fps with `missed_refreshes=0`, for as long as it was
open. The same page now runs at 29.8fps. Four times fewer frames, each of which was a
full-window repaint.

Measured with `BLITZ_FRAME_STATS=1 BLITZ_FRAME_STATS_FILE=<path>` on a 120Hz display,
against a build of the immediately preceding commit, same page, same binary otherwise.

**How, and why not the obvious way.** The obvious clamp is to skip the
`request_redraw()` when too little time has passed. That stalls the animation
permanently, because that call is the only thing that schedules the next frame.
Animation-only frames now record when the next one is *due*
(`window.rs`, `animation_frame_due`), and `about_to_wait` turns the earliest deadline
across all windows into a `ControlFlow::WaitUntil`, so the event loop sleeps in between
rather than spinning.

Two details that cost a measurement each:

- The deadline runs from when the frame **started**, not from when it ended. The other
  way round, a 6ms frame plus a 33ms wait is a 39ms cadence, and the clamp silently runs
  slower than it claims: 24fps measured where 30 was asked for.
- The control flow is left alone when nothing is animating, rather than being set to
  `Wait`. Overwriting it there would fight whatever else set it, including the
  debug controller's 10ms poll.

Input, resize, navigation and every other event still redraw immediately, so nothing
clamped here is something a user is waiting on.

**Still open:** the same fix in AgencyZero's checkout, `ps-blitz-render`, which is a
separate tree.

### 2. Gate animation ticks on visibility ENGINE, browser-specific

- **Where:** the `has_active_animations` computation at `stylo.rs:171`.
- **What:** do not let an animation on an element outside the viewport keep the document
  animating. Blitz already culls offscreen elements at paint
  (`blitz-paint/src/render.rs:311`), so the geometry needed to answer this exists.
- **Why:** a long page can carry dozens of animated elements, almost all offscreen. This
  item does not exist for AgencyZero, whose UI fits the window.
- **Depends on:** nothing, though it composes with damage regions later.
- **Caveat:** CSS animations are specified to keep running while offscreen; the observable
  behaviour must stay correct if the element scrolls back into view, and `animationend`
  timing must not shift. This is a redraw-scheduling change, not an animation-clock change.
- **Size:** medium. Needs care.

### 3. Refuse animations that cannot change a pixel ENGINE

- **What:** Blink's `kAnimationHasNoVisibleChange`
  ([`compositor_animations.h:120`](https://github.com/chromium/chromium/blob/7e6a84f5165fd617dbf3d032f755e11804bf8ff6/third_party/blink/renderer/core/animation/compositor_animations.h#L120)).
- **Why:** real pages carry animations that animate nothing observable, especially after
  ad blockers and extensions remove their targets.
- **Size:** small to medium.

### 4. The real fix, recorded so this list is not mistaken for one

Damage regions, `agencyzero/docs/partial-paint.md`, with opacity and transform treated as
dependency values rather than damage triggers. Items 1 to 3 are stopgaps until that exists.

## Related

- `agencyzero/docs/animation-gap.md`, the same engine finding where the animations are
  authored in-house and can simply be dropped.
- `agencyzero/docs/blink-what-we-can-learn.md` section 3, the enumerated
  compositor-eligibility model.
- `agencyzero/docs/partial-paint.md`, why one animated element costs a whole frame.
- [TODO-dom-related-work.md](TODO-dom-related-work.md), which shares the ENGINE-in-two-trees
  problem items 1 to 3 have.
