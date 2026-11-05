# Browser runtime, rendering, and robustness follow-ups

This plan consolidates browser-host work raised by the DOM/CSS/layout, Web API,
robustness, publishing, and canvas/media reviews. Recheck ownership and the
current implementation before starting each item. The engine and renderer
requirements listed at the end belong to their maintainers and are not Chuzz
implementation tasks.

## Browser-host requirements

### Document lifecycle and resource bounds

- Give every served document a bounded lifecycle: enforce wall-time and memory
  limits, detect parent termination where supported, and clean up its control
  socket and descriptor on normal exit, cap failure, timeout, and listener
  failure. Do not leave orphan processes or stale control files.
- Ensure a navigation replaces and tears down the prior script host, document,
  and observers before releasing their resources. Keep GUI and headless teardown
  behavior aligned.
- Park an unobserved idle host when it has no actionable jobs or timers. Define
  which page activity wakes it and avoid periodic polling for idle work.
- Bound document-created network requests and response bytes. Preserve the
  initiating document across navigation, reload, history traversal, redirects,
  workers, and subresource loads so file and private-network policies are
  enforced at the shared request boundary.
- Bound script work, host-object tables, event retention, node handles, and
  memory growth. Reclaim host objects when their script objects are no longer
  live; avoid permanent pinning of every node ever exposed to script.
- Keep BroadcastChannel names, per-realm/process channel counts, per-channel
  queued message counts, and Web Locks requests bounded. Current source caps
  names at 1 KiB, channels and lock requests at 32 per realm/128 process-wide,
  and each BroadcastChannel queue at 32 messages. Verify these limits and
  observable refusal behavior against the supported workload before treating
  them as compatibility-complete; focused tests are present but still need to
  run.
- Report console errors, runtime exceptions, panics, and useful per-document
  resource metrics through diagnostics. A successful command acknowledgement
  alone is not evidence that asynchronous events were delivered.

### Browser policy and observable API behavior

- Revalidate API-inventory and prototype-probe reports against observable
  behavior before implementing a reported missing API; rule out probe false
  positives and assign renderer or engine causes to their respective owners.

- Keep site permissions and user activation in the browser app. Gate WebAuthn
  and audio resume on the appropriate site identity, focused tab, and user
  gesture; arbitrate prompts per tab. Page-side API shims must not invent
  permission grants or bypass browser policy.
- The native WebAuthn `begin` path still lacks a focused-tab and one-use
  activation capability. Decide whether to deny create/get until that mediation
  is implemented or implement native capability consumption with blur
  cancellation and an RP-named prompt. Origin validation alone is not focus or
  user activation.
- Validate browser compatibility APIs through observable behavior, including
  abort and reentrant event delivery, option validation, and media stream
  claims. Keep unsupported operations explicit and consistent.
- Preserve cross-origin canvas and GPU tainting, prevent SVG image loads from
  accessing host files, and keep CSP checks at document and fetch boundaries
  distinct from browser-owned capability policy.
- The shared video pixel-source resolver currently rejects every video pixel
  import until origin-clean provenance is carried through decode and rendering.
  Verify same-origin video imports and cross-origin taint before removing that
  conservative guard.
- Ensure keyboard focus and scrolling have coherent document behavior,
  including initial tab focus, tabindex ordering, contenteditable focus, and
  keyboard scrolling. Confirm window scroll and resize events reflect actual
  layout state.

### Rendering and media host integration

- Revalidate URL audio playback in headless mode with a neutral fixture.
  Require observable audio and video delivery, rather than connection success
  alone, while preserving autoplay and permission policy.

- Drive animation callbacks from the document frame lifecycle with one
  generation per frame, timestamps on the page performance clock, and no
  callbacks for hidden or discarded pages. Avoid coupling animation delivery
  to unrelated geometry polling.
- Exercise the browser host's 2D canvas, WebGPU, image-source, video, and audio
  paths through deterministic local fixtures. Keep validation errors
  synchronous where the API requires it, and make asynchronous decoding and
  device waits event-driven rather than sleep-polled.
- Preserve browser policy for autoplay and media permissions. Deliver video
  frames against the media clock, account for stale frames, and ensure
  `requestVideoFrameCallback` observes frames actually presented to the page.
- For GUI presentation, prefer sharing renderer-owned GPU resources when the
  renderer exposes a supported surface contract. Keep a bounded fallback for
  headless capture and explicit readback; do not make ordinary GUI presentation
  depend on a blocking readback.
- Keep binary payload transfer bounded and avoid unnecessary conversion or
  cloning between script, host, and renderer. Treat this as an implementation
  constraint, not a performance claim until measured.

### Deterministic acceptance and release readiness

- Verify TLS certificate-path selection under the approved crypto policy using
  browser-native probes and the TLS fixture backend. Distinguish path-selection
  failures from unsupported algorithms and application-level refusal.

- Maintain a small, redistributable set of local pages with bundled fonts,
  stable viewport and scale, and explicit checks for DOM interaction, layout,
  paint, hit testing, scrolling, geometry, forms, canvas, GPU, media, and
  lifecycle behavior. Use opaque fixture identifiers and no live-site content.
- Include failure-class fixtures for hangs, event storms, node churn, deep
  documents, unsupported features, timer load, navigation churn, large scripts,
  error delivery, and clean single-page applications. Apply fixed per-run
  resource caps and verify that no process or control artifact remains after a
  run.
- Base compatibility and performance claims on recorded, reproducible evidence
  with workload and environment stated. Keep rendering failures distinct from
  explicit refusals and unsupported layout. Do not treat an aggregate pass rate
  alone as proof of improvement.
- Keep external acceptance workflows manual by default. A workflow should use
  local fixtures, document its resource limits, and avoid publishing fixture
  output as source content.
- Before publishing, verify the public dependency graph, package contents,
  supported targets, license and repository metadata, and whether each changed
  package requires a version change. Keep local engine overrides untracked and
  opt-in; release builds must resolve from published dependency ranges.

## Separate ownership: renderer and engine

The following requirements need work in the relevant Pathscale renderer or
script-engine repositories before Chuzz can consume them. They are recorded
here to preserve the dependency, not to assign their implementation to Chuzz:

- DOM/CSS/layout correctness: selector state and invalidation, focus and
  scrolling semantics, geometry and window events, intrinsic sizing, text and
  font behavior, clipping and hit testing, top-layer behavior, and paint
  fidelity.
- Script-engine host integration: lifecycle-aware animation scheduling,
  garbage-collection-aware host object reclamation, bounded allocation and
  event delivery, borrowed byte views with safe non-reentrant lifetimes, and
  hooks for browser-owned media state and frame delivery.
- Renderer integration: a supported custom-surface/widget contract for shared
  GPU presentation, renderer-device capability limits, and redraw scheduling
  for animated content.
- Registry and release prerequisites for those components must be resolved by
  their owning repositories before Chuzz changes its dependency ranges. Do not
  add sibling path, Git, or exact-version dependencies to work around a
  publication gap.
