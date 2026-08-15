# Chuzz working agreement

- Chuzz is a pure Rust web browser. The invariant is the implementation stack, not the
  authoring language of the UI: no C++ engine, no JIT, no OS webview, every dependency Rust.
  Do not add WebKit, Chromium, or Electron. Tauri, Boa and a Solid bundle are all Rust or data
  the Rust stack interprets, and are in scope.
- Use the Pathscale Blitz stack for DOM, CSS, layout, networking, painting, and window integration.
- The browser chrome is a SolidJS app in `apps/chuzz/frontend`, built with rsbuild and
  `@pathscale/ui`, the same stack AgencyZero ships. It talks to Rust through one interface
  (`src/api/client.ts`) with a Tauri implementation and a mock; nothing above that file knows
  which it has.
- Keep browser policy separate from the renderer. The browser app owns tabs, navigation, history, permissions, persistence, and downloads.
- Keep the local control surface protocol-compatible with AgencyZero's Blitz interface: `blitz.agent.control` and `blitz.diagnostics` over local MCP framing.
- Control is opt-in, local-only, and unauthenticated only because the socket and descriptor are owner-readable. Never bind it to a network interface.
- **The engine is a pinned revision, not a sibling checkout.** `ps-blitz`,
  `tauri-runtime-blitz` and `endpoint-libs` are git dependencies with a `rev` in
  `Cargo.toml`, so `Cargo.lock` records exactly what a release builds and an
  ordinary `cargo build` fetches it. Do not turn them back into `path =
  "../..."`. That is what this repository did before, and it put the revision CI
  used in a `BLITZ_REF` env var in release.yml while every developer built
  against whatever happened to be on disk. The pin sat 44 commits behind, and
  the release job broke the day a new engine package was added, because a path
  dependency resolves against the filesystem and no local build can notice.
- **Building against a working checkout is opt-in and never edits a tracked
  file.** Put the `[patch]` tables in `.cargo/local-engine.toml`, which is
  gitignored, and reach for them per command:
  `scripts/local-engine.sh check -p chuzz-gui`. The wrapper snapshots and
  restores `Cargo.lock`, because a redirected build rewrites it to point at
  directories that exist on one machine. Patch only the crates you are actually
  changing; every entry is a pin that stops being tested.
- When you move the engine pin, move `tauri-runtime-blitz`'s to match. Both name
  a ps-blitz revision, and two different ones put two engines in the graph,
  which surfaces as missing methods and unrelated `PaintScene` traits rather
  than as a version error.
- Work on a branch and ship through a pull request. Do not commit to `main`.
- Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and `cargo test --workspace --all-features` before delivery.

