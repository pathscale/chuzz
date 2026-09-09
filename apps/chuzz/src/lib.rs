//! The browser, as a library, so that more than one binary can be it.
//!
//! There are two: `chuzz-gui`, which puts the engine in a window, and
//! `chuzz-headless`, which puts the same engine behind the inspection socket
//! with no window at all. They share every module below, which is the point.
//! When each declared its own `mod` tree the compiler treated the modules the
//! other binary reaches as dead code, and `-D warnings` in CI then made
//! "compiles as a second binary" and "compiles at all" different questions.
//!
//! More importantly, sharing is the design and not an optimisation. The
//! headless host used to be a separate crate in another repository with its own
//! document builder, and the web-API shim that lets a routed page reach its
//! first render lived only here. A gap closed for the browser stayed open for
//! the harness, so the harness measured a browser nobody ships. There is one
//! loader now, and one place a gap gets fixed.

// The window and its Tauri command surface. Behind `gui` because `tauri` is,
// and because a headless build has no window to drive.
#[cfg(feature = "gui")]
pub mod browser;
#[cfg(feature = "capture")]
pub mod capture;
pub mod decode;
pub mod document_loader;
// `capture` writes the tree beside the PNG, so the two arrive together or the
// pixels have nothing to be explained by.
#[cfg(feature = "capture")]
pub mod dump;
// Draws the browser chrome, which only exists when there is a window.
#[cfg(feature = "gui")]
pub mod frontend;
pub mod identity;
// The browser's own documents: view source, the error page. Pages rather than
// chrome, so the headless host reaches them too.
pub mod internal_pages;
pub mod nav;
pub mod net_bridge;
// A built site needs an origin before it is a site. Only the headless host
// reaches for it, but it is not conditional on that mode: what it does is a
// fact about serving a directory, not about having no window.
pub mod page_server;
pub mod script_fetch;
pub mod ws_bridge;
// Loading a page needs the rasteriser for a visual assertion and the script
// engine for a page worth asserting about, which is what `capture` and
// `javascript` carry.
#[cfg(all(feature = "capture", feature = "javascript"))]
pub mod serve;
// Shared by `--wasm` in the window and `--capture-wasm` headlessly, so a
// guest-built page cannot render one way in a tab and another in a capture.
#[cfg(feature = "wasm")]
pub mod wasm_page;
