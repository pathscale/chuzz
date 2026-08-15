# Chuzz

Chuzz is a pure Rust web browser built on the Pathscale Blitz engine. No C++ engine, no JIT,
no OS webview: it does not embed WebKit, Chromium, or Electron.

"Pure Rust" is about the implementation stack, not about what the UI is written in. The browser
chrome is a SolidJS app interpreted by Boa, the same way page content is, and every crate under
it is Rust.

```sh
cargo run -p chuzz-gui                  # opens a blank tab
cargo run -p chuzz-gui -- example.com   # opens a bare hostname over HTTPS
cargo run -p chuzz-gui -- --wasm demo.wasm   # a tab a WebAssembly guest builds
```

A non-URL argument is not a search: anything that is neither a URL nor a hostname
does nothing, so a mistyped path cannot quietly navigate somewhere unrelated.

## Where the built binary lands

Chuzz's executable is `chuzz-gui`. It is this repository's counterpart to
AgencyZero's `az-gui`: the package in `apps/chuzz` and the binary it produces
share that name, the same way `apps/gui` produces `az-gui`. This is the binary to
run when you want the browser without going through cargo.

| What | Path |
| --- | --- |
| Release binary | `target/release/chuzz-gui` |
| Debug binary | `target/debug/chuzz-gui` |
| macOS bundle | `target/release/bundle/macos/Chuzz.app` |

```sh
./apps/chuzz/build-app.sh release    # builds, bundles, ad-hoc signs, prints the bundle path
./target/release/chuzz-gui           # or run the bare binary
```

The bundle's `Contents/MacOS/chuzz-gui` is a copy of `target/release/chuzz-gui` that has
been re-signed, so the two are the same build but differ in size and timestamp.
Compare `CFBundleShortVersionString` against `[workspace.package] version` if you
need to know which build a bundle came from; nothing else distinguishes them.

## Layout

The window is a Chrome-shaped shell: tab strip on top, toolbar under it, page filling the rest.

```text
apps/chuzz/src
  tauri_main.rs       the Tauri app: command surface, runtime, window
  browser.rs          tabs, session history, page sources, and the commands the chrome calls
  frontend.rs         the interface document: embedded Solid bundle, web-API shims
  document_loader.rs  the net provider, the web-API shim, and the headless capture path
  decode.rs           response bodies: gzip, brotli, and undecodable bytes
  nav.rs              what a typed string means: URL, bare hostname, or neither
  wasm_page.rs        build a document by running a WebAssembly guest (feature: wasm, default on)
  capture.rs          render a page to a PNG without a window (feature: capture, default on)
  dump.rs             write the node tree beside a capture (feature: capture, default on)

apps/chuzz/frontend   the interface itself: Solid, @pathscale/ui, and the local Layouts
crates/chuzz-control  built-in diagnostics and agent-control interface (in-process, no server)
```

## Two documents, not one

The browser UI is itself a Blitz document: a Solid application, compiled to a bundle and
embedded in the binary, running under Boa inside the window's own document. Each tab's page is
a separate child document mounted inside a `<web-view>` element in that UI. Page markup and
browser markup never share a DOM, so a site's CSS cannot restyle the chrome and the chrome's
CSS cannot leak into the site.

The rendezvous between the two is an element id, `chuzz-page-{tabId}`. That lookup is what
decides whether a fetched page is ever attached, so it has a regression test rather than a
comment: when the identifier drifted, every site loaded correctly and rendered nothing.

Everything a browser decides rather than renders stays in this binary: tabs, session history,
address-bar interpretation, and error pages. The engine only reports that a navigation was
requested; `browser.rs` decides what the back and forward stacks look like afterwards.

## Two ways to build a page

A page document is normally fetched and parsed. It can also be built by a WebAssembly
guest, which reaches the DOM through the `blitz-wasm` ABI: no URL, nothing fetched, no
HTML parsed, and no JavaScript anywhere in the path.

```sh
chuzz-gui --wasm demo.wasm       # a tab whose document the guest builds
```

The guest is handed an empty `<html><body>` and the body as its mount point, and the
result attaches at the same `chuzz-page-{tabId}` rendezvous a fetched page uses. It is
an ordinary tab addressed by a `file:` URL, so history, the address bar and the back and
forward stacks hold it without knowing what it is. The guest's entry export is `run`,
overridable with `CHUZZ_WASM_ENTRY`.

Events are not wired up yet: the page renders and does not respond.

## Rendering without a window

```sh
chuzz-gui --capture out.png https://example.com          # a fetched page
chuzz-gui --capture-wasm demo.wasm --out out.png --tree out.txt
```

`CHUZZ_CAPTURE_TREE` is a path, not a flag: it writes the laid-out tree beside the PNG,
one line per node with absolute boxes, which is the only way to tell a layout fault from
a paint fault. `CHUZZ_CAPTURE_SCALE` sets the device pixel ratio, defaulting to 1; the
window runs at 2 on a retina display, so a capture at 1 cannot reproduce a whole class of
paint fault.

`scripts/render-check.sh` runs a corpus of pages through this and writes a PNG, a tree
dump and a log per page.

Both flags need the `capture` feature, which is on by default. It used to be off, and a
plain `cargo build` then produced a binary where `--capture` was silently not a flag at
all, which reads as the flag being broken rather than absent.

## Engine

Pathscale Blitz supplies HTML parsing, Stylo CSS, Taffy layout, networking, Vello rendering,
input, and accessibility. The renderer is Vello on wgpu, following AgencyZero's Blitz
performance thesis: Metal on macOS, with the Vulkan and D3D12 paths preserved for Linux and
Windows.

## Building the app

```sh
apps/chuzz/build-app.sh          # release, the default
apps/chuzz/build-app.sh debug    # unoptimised, for a backtrace
```

Writes `target/<profile>/bundle/macos/Chuzz.app`, ad-hoc signed. Drag it to
/Applications, or `open` it in place.

The icon is `apps/chuzz/icons/icon.icns`, committed. Its source is
`icons/icon.html`, which Chuzz renders itself: `apps/chuzz/icons/build-icon.sh`
rebuilds the icns and is only needed when the markup changes.
