# Chuzz working agreement

- Chuzz is a pure Rust web browser. Do not add WebKit, Chromium, Electron, Tauri, or a JavaScript host shell.
- Use the Pathscale Blitz stack for DOM, CSS, layout, networking, painting, and window integration.
- Keep browser policy separate from the renderer. The browser app owns tabs, navigation, history, permissions, persistence, and downloads.
- Keep the local control surface protocol-compatible with AgencyZero's Blitz interface: `blitz.agent.control` and `blitz.diagnostics` over local MCP framing.
- Control is opt-in, local-only, and unauthenticated only because the socket and descriptor are owner-readable. Never bind it to a network interface.
- Work on a branch and ship through a pull request. Do not commit to `main`.
- Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and `cargo test --workspace --all-features` before delivery.

