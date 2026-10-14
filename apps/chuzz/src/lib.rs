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
//! headless host used to be a separate crate in another repository, with its own
//! document builder, and the web-API shim that lets a routed page reach its
//! first render lived only here. A gap closed for the browser stayed open for
//! the harness, so the harness measured a browser nobody ships. There is one
//! loader now, and one place a gap gets fixed.

/// Install RustCrypto for consumers that use rustls' process default.
///
/// Both binaries call this before reaching the network. Browser HTTPS and
/// WSS configurations also name the provider explicitly, so an embedder's
/// previously installed default cannot change their cryptography.
pub fn install_crypto_provider() {
    let _ = blitz_net::chacha_provider()
        .as_ref()
        .clone()
        .install_default();
}

/// Configure the global allocator (rusty_alloc) for a long-running process: freed memory is purged
/// back to the OS after 10 ms, which keeps resident memory flat. Opt-in in rusty_alloc's defaults;
/// `RUSTY_ALLOC_PURGE_DELAY` in the environment still overrides it.
pub fn tune_allocator() {
    if let Some(purge_delay) = rusty_alloc::options::OPTION_NAMES
        .iter()
        .position(|name| *name == "purge_delay")
    {
        rusty_alloc::options::set_default(purge_delay, 10);
    }
}

/// Set host policy before opening media. The device-audio feature is also
/// required to open a physical output. Live stream QA uses native PCM output.
#[cfg(feature = "javascript")]
pub fn set_device_audio_enabled(enabled: bool) {
    video::set_device_audio_enabled(enabled);
}

#[cfg(all(feature = "headless-fonts", not(feature = "system-fonts")))]
mod headless_fonts;

/// Give a document the compiled-in faces when this build has no system catalogue; a no-op
/// otherwise, so callers invoke it unconditionally.
#[allow(unused_variables)]
pub(crate) fn install_headless_fonts(config: &mut blitz_dom::DocumentConfig) {
    #[cfg(all(feature = "headless-fonts", not(feature = "system-fonts")))]
    headless_fonts::install(config);
}

/// A CSS family list with the compiled-in stand-ins appended in a build without system fonts.
#[cfg(feature = "javascript")]
pub(crate) fn headless_family_list(list: &str) -> std::borrow::Cow<'_, str> {
    #[cfg(all(feature = "headless-fonts", not(feature = "system-fonts")))]
    return std::borrow::Cow::Owned(headless_fonts::family_list(list));
    #[cfg(not(all(feature = "headless-fonts", not(feature = "system-fonts"))))]
    std::borrow::Cow::Borrowed(list)
}

#[cfg(feature = "javascript")]
pub mod audio;
// The window and its izumo command surface. Behind `gui` because `izumo` is,
// and because a headless build has no window to drive.
#[cfg(feature = "gui")]
pub mod browser;
#[cfg(feature = "capture")]
pub mod capture;
pub mod cookie_store;
pub mod decode;
pub mod document_loader;
#[cfg(unix)]
pub mod headless_io;
mod resource_parameters;
// `capture` writes the tree beside the PNG, so the two arrive together or the
// pixels have nothing to be explained by.
#[cfg(feature = "capture")]
pub mod dump;
// Draws the browser chrome, which only exists when there is a window.
#[cfg(feature = "gui")]
pub mod frontend;
pub mod identity;
pub mod image_document;
pub mod languages;
// The browser's own documents: view source, the error page. Pages rather than
// chrome, so the headless host reaches them too.
pub mod internal_pages;
#[cfg(feature = "javascript")]
pub mod media;
pub mod nav;
pub mod net_bridge;
#[cfg(feature = "gui")]
mod tab_native;
#[cfg(feature = "gui")]
pub mod tab_records;
#[cfg(feature = "gui")]
mod tab_resources;
#[cfg(feature = "gui")]
mod tab_session;
#[cfg(feature = "javascript")]
pub mod tab_stash;
// A built site needs an origin before it is a site. Only the headless host
// reaches for it, but it is not conditional on that mode: what it does is a
// fact about serving a directory, not about having no window.
pub mod page_server;
#[cfg(feature = "pdf")]
pub mod pdf;
pub mod platform_hardening;
pub mod sandbox;
pub mod script_fetch;
pub(crate) mod telemetry;
mod tls;
pub mod tls_status;
#[cfg(feature = "javascript")]
mod video;
#[cfg(feature = "javascript")]
mod webapi;
#[cfg(feature = "javascript")]
pub mod webauthn;
pub mod webgpu;
#[cfg(feature = "javascript")]
pub mod webrtc;
#[cfg(feature = "javascript")]
mod workers;
pub mod ws_bridge;
// Loading a page needs the rasteriser for a visual assertion and the script
// engine for a page worth asserting about, which is what `capture` and
// `javascript` carry.
#[cfg(all(feature = "capture", feature = "javascript"))]
pub mod serve;
#[cfg(test)]
mod wasm_page;
pub mod wasm_retired;
