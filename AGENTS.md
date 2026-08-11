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
- Work on a branch and ship through a pull request. Do not commit to `main`.
- Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and `cargo test --workspace --all-features` before delivery.

