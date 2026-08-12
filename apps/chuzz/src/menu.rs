//! The macOS application menu, which exists for one item: About.
//!
//! AgencyZero reports its build through a Tauri `About` menu item. There is no
//! Tauri here and `dioxus-native` exposes no menu at all, so this builds one
//! directly with `muda`, the same crate Tauri uses underneath. It is pure Rust
//! under MIT/Apache-2.0, so it does not cost the invariant.
//!
//! **Deliberately just App > About and Quit.** An Edit menu is the obvious
//! thing to add next and would be a mistake here: its accelerators would take
//! Cmd+C/X/V before they reach the document, and this browser handles the
//! clipboard itself in `blitz-dom`'s text input rather than through Cocoa
//! responders, so the standard items would swallow the keys and do nothing.
//! For the same reason there is no `close_window` item, because Cmd+W already
//! means "close tab" here.

use muda::{AboutMetadata, Menu, PredefinedMenuItem, Submenu};

/// Build the menu and attach it to the running application.
///
/// Must run after the event loop exists, since attaching needs a live NSApp.
/// Failures are swallowed: a browser without a menu is still a browser, and a
/// panic here would take the window with it.
pub fn install() {
    let menu = Menu::new();
    let app = Submenu::new("Chuzz", true);
    if menu.append(&app).is_err() {
        return;
    }

    let about = PredefinedMenuItem::about(Some("About Chuzz"), Some(metadata()));
    let _ = app.append_items(&[
        &about,
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::hide(None),
        &PredefinedMenuItem::hide_others(None),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::quit(None),
    ]);

    #[cfg(target_os = "macos")]
    menu.init_for_nsapp();
}

/// What the About panel shows.
///
/// The version is bumped by hand, so on its own it cannot answer "am I testing
/// the fix?" — a stale bundle looks identical to a fresh one. The commit says
/// which code, a trailing `*` says the tree had uncommitted edits on top of it,
/// and the timestamp is what gets compared against "I just rebuilt".
fn metadata() -> AboutMetadata {
    AboutMetadata {
        name: Some("Chuzz".to_owned()),
        version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        short_version: Some(env!("CHUZZ_GIT_SHA").to_owned()),
        comments: Some(format!("Built {}", env!("CHUZZ_BUILT_AT"))),
        copyright: Some("Copyright © 2026 PathScale Pte Ltd".to_owned()),
        license: Some("MIT OR Apache-2.0".to_owned()),
        website: Some("https://github.com/pathscale/chuzz".to_owned()),
        website_label: Some("pathscale/chuzz".to_owned()),
        ..Default::default()
    }
}
