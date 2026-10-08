# Runtime-free browser integration proposal

The companion patch contains the browser's standalone Izumo shell and Nagoya networking integration. It is source preparation, not enabled code or a verified browser release.

Target: this repository and PR #51. Source base: `787199f3ab64226d320af3e9f53eca76bcb728f8`; original integration head: `848e8dd56a4100734ed3254dde8ad9634d701d5f`; formatting follow-up: `55512e4`.

Activation requirements:

- Published compatible `ps-blitz` and `izumo` 0.5 packages. A normal published-dependency build currently fails resolution at `ps-blitz-dom ^0.5`; local path overrides are not release evidence.
- Explicitly attach the browser cache provider. The proposed network provider otherwise defaults to no cache.
- Resolve the companion network provider's plaintext Cookie persistence, disk quota, deleted regression coverage and HTTP/2/proxy/zstd gaps before adopting it.
- Update obsolete Tauri build and packaging instructions for the standalone shell.
- Pass formatting, all-feature Clippy and workspace tests on the activated code, then meaningful rendered browser acceptance: navigation, cookie persistence across graceful quit/reopen, WebSocket negotiation, editing/IME and cache reuse.

Required browser parity includes App/Edit/View/Window menus; focused-field Cmd-C/V/X/A/Z; View Source from both Cmd-U and menu; IME in both browser chrome and page inputs; and a painted mounted page through the existing headless and GUI control paths.

Networking acceptance must cover one shared client across document/subresource/script/fetch/XHR paths, bounded per-origin connections, bodies larger than transport defaults, all supported redirect statuses with cookies at each hop, cross-origin sensitive-header removal and browser method rewriting. Advertise only decoded content encodings. Verify private per-profile RFC9111 cache keys and reuse; heuristic caching of Set-Cookie responses requires an explicit security decision rather than assuming it is safe. Preserve WebSocket subprotocol negotiation and deliberate frame/message limits.

Use platform trust roots and one consistent process-wide crypto provider; blocking DNS must not occupy reactor/document threads. Existing application packaging and signing remain separate contracts: removing Tauri is not permission to remove or rotate signing identities.

Local build configuration, generated files and working notes are deliberately excluded. Applying this patch is not proof that those requirements passed.
