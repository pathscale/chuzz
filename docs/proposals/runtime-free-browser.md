# Runtime-free browser integration proposal

The companion patch contains the browser's standalone Izumo shell and Nagoya networking integration. It is source preparation, not enabled code or a verified browser release.

Target: this repository and PR #51. Source base: `787199f3ab64226d320af3e9f53eca76bcb728f8`; original integration head: `848e8dd56a4100734ed3254dde8ad9634d701d5f`; formatting follow-up: `55512e4`.

Activation requirements:

- Published compatible `ps-blitz` and `izumo` 0.5 packages. A normal published-dependency build currently fails resolution at `ps-blitz-dom ^0.5`; local path overrides are not release evidence.
- Explicitly attach the browser cache provider. The proposed network provider otherwise defaults to no cache.
- Resolve the companion network provider's plaintext Cookie persistence, disk quota, deleted regression coverage and HTTP/2/proxy/zstd gaps before adopting it.
- Update obsolete Tauri build and packaging instructions for the standalone shell.
- Pass formatting, all-feature Clippy and workspace tests on the activated code, then meaningful rendered browser acceptance: navigation, cookie persistence across graceful quit/reopen, WebSocket negotiation, editing/IME and cache reuse.

Local build configuration, generated files and working notes are deliberately excluded. Applying this patch is not proof that those requirements passed.
