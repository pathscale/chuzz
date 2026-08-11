# Chuzz

Chuzz is a pure Rust web browser built on the Pathscale Blitz engine. No C++ engine, no JIT,
no OS webview: it does not embed WebKit, Chromium, or Electron.

"Pure Rust" is about the implementation stack, not about what the UI is written in. The browser
chrome is a SolidJS app interpreted by Boa, the same way page content is, and every crate under
it is Rust.

```sh
cargo run -p chuzz-gui                  # opens a blank tab
cargo run -p chuzz-gui -- example.com   # opens a bare hostname over HTTPS
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
  main.rs             window, startup URL, and the shell layout
  tab_strip.rs        tab row: select, close, new tab
  toolbar.rs          back, forward, reload, address bar
  tab.rs              one tab: history + loader + the document on screen
  document_loader.rs  fetch a URL, parse it into a page document, abort the previous load
  history.rs          per-tab session history and the engine's navigation hook
  nav.rs              what a typed string means: URL, bare hostname, or search
  ui.rs               stylesheet for the browser UI

crates/chuzz-control  built-in diagnostics and agent-control interface (in-process, no server)
```

## Two documents, not one

The browser UI is itself a Blitz document, driven by Dioxus Native. Each tab's page is a
separate child document mounted inside the UI's `web-view` element. Page markup and browser
markup never share a DOM, so a site's CSS cannot restyle the toolbar and the toolbar's CSS
cannot leak into the site.

Everything a browser decides rather than renders stays in this binary: tabs, session history,
address-bar interpretation, and error pages. The engine only reports that a navigation was
requested; `history.rs` decides what the back and forward stacks look like afterwards.

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
