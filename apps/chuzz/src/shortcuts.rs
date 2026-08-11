//! Keyboard bindings, mapped from AgencyZero's menu accelerators.
//!
//! Their set is defined in `apps/gui/src/main.rs` as native menu accelerators:
//! Cmd+N new project, Cmd+W close tab, Cmd+1 / Cmd+2 cycle tabs, Cmd+S home,
//! Ctrl+T new project, Cmd+Q quit. The same chords carry over here with the
//! browser's meaning, plus the two a browser cannot do without: Cmd+L to focus
//! the address bar and Cmd+R to reload.
//!
//! Matched on `code` as well as `key`, so a layout that puts something else on
//! the physical key still works.
//!
//! Chuzz has no native menu bar yet, so these are plain keydown bindings. On
//! macOS a menu accelerator would take precedence over the document, which is
//! what makes AgencyZero's Cmd chords reliable inside text fields; until that
//! menu exists, a Cmd chord pressed inside the address bar may be consumed by
//! the field first.

use dioxus_native::prelude::*;

use crate::nav::{HOME_URL, NEW_TAB_URL};
use crate::tab::{Tab, TabId, TabStoreImplExt, active_tab, open_tab};
use crate::tab_strip::close_tab;

/// What a keypress asked for, resolved before anything is mutated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortcut {
    NewTab,
    CloseTab,
    PreviousTab,
    NextTab,
    Reload,
    FocusAddressBar,
    Home,
    Back,
    Forward,
}

/// Resolve a keypress. Separated from the acting on it so the mapping can be
/// tested without a running UI.
pub fn resolve(
    key: &str,
    code: &str,
    meta: bool,
    ctrl: bool,
    alt: bool,
    shift: bool,
) -> Option<Shortcut> {
    if alt {
        return None;
    }

    // Ctrl+T is AgencyZero's webview binding for a new project; here, a new tab.
    if ctrl && !meta && !shift && (key == "t" || code == "KeyT") {
        return Some(Shortcut::NewTab);
    }

    if !meta {
        return None;
    }

    // Cmd+Shift+[ / ] cycle tabs the way every browser does, alongside the
    // Cmd+1 / Cmd+2 pair AgencyZero uses.
    if shift {
        return match (key, code) {
            ("{", _) | (_, "BracketLeft") => Some(Shortcut::PreviousTab),
            ("}", _) | (_, "BracketRight") => Some(Shortcut::NextTab),
            _ => None,
        };
    }

    match (key, code) {
        ("t", _) | (_, "KeyT") => Some(Shortcut::NewTab),
        ("w", _) | (_, "KeyW") => Some(Shortcut::CloseTab),
        ("1", _) | (_, "Digit1") => Some(Shortcut::PreviousTab),
        ("2", _) | (_, "Digit2") => Some(Shortcut::NextTab),
        ("r", _) | (_, "KeyR") => Some(Shortcut::Reload),
        // Cmd+L and Cmd+D both reach the address bar, as Chrome does.
        ("l", _) | (_, "KeyL") => Some(Shortcut::FocusAddressBar),
        ("d", _) | (_, "KeyD") => Some(Shortcut::FocusAddressBar),
        ("s", _) | (_, "KeyS") => Some(Shortcut::Home),
        ("[", _) | (_, "BracketLeft") => Some(Shortcut::Back),
        ("]", _) | (_, "BracketRight") => Some(Shortcut::Forward),
        _ => None,
    }
}

/// Apply a resolved shortcut to the browser's state.
pub fn apply(
    shortcut: Shortcut,
    tabs: Store<Vec<Tab>>,
    mut active_tab_id: Signal<TabId>,
    mut focus_address_bar: Signal<bool>,
    net_provider: std::sync::Arc<crate::document_loader::NetProvider>,
) {
    let open: Vec<TabId> = tabs.iter().map(|tab| tab.tab_id()).collect();
    let current = open.iter().position(|id| *id == active_tab_id());

    match shortcut {
        Shortcut::NewTab => {
            // `Url::parse` of a constant literal cannot fail.
            #[allow(clippy::expect_used)]
            let blank = Url::parse(NEW_TAB_URL).expect("blank URL is a valid constant");
            let opened = open_tab(tabs, blank, net_provider);
            active_tab_id.set(opened.tab_id());
        }
        Shortcut::CloseTab => close_tab(tabs, active_tab_id, active_tab_id()),
        Shortcut::PreviousTab => {
            if let Some(index) = current
                && !open.is_empty()
            {
                let next = if index == 0 {
                    open.len() - 1
                } else {
                    index - 1
                };
                active_tab_id.set(open[next]);
            }
        }
        Shortcut::NextTab => {
            if let Some(index) = current
                && !open.is_empty()
            {
                active_tab_id.set(open[(index + 1) % open.len()]);
            }
        }
        Shortcut::Reload => active_tab(tabs, active_tab_id()).reload(),
        Shortcut::Back => active_tab(tabs, active_tab_id()).go_back(),
        Shortcut::Forward => active_tab(tabs, active_tab_id()).go_forward(),
        Shortcut::FocusAddressBar => focus_address_bar.set(true),
        Shortcut::Home => {
            // `Url::parse` of a constant literal cannot fail.
            #[allow(clippy::expect_used)]
            let home = Url::parse(HOME_URL).expect("home URL is a valid constant");
            active_tab(tabs, active_tab_id()).navigate(Request::get(home));
        }
    }
}

use blitz_traits::net::{Request, Url};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_agencyzero_accelerators_carry_over() {
        assert_eq!(
            resolve("w", "KeyW", true, false, false, false),
            Some(Shortcut::CloseTab)
        );
        assert_eq!(
            resolve("1", "Digit1", true, false, false, false),
            Some(Shortcut::PreviousTab)
        );
        assert_eq!(
            resolve("2", "Digit2", true, false, false, false),
            Some(Shortcut::NextTab)
        );
        assert_eq!(
            resolve("s", "KeyS", true, false, false, false),
            Some(Shortcut::Home)
        );
        assert_eq!(
            resolve("t", "KeyT", false, true, false, false),
            Some(Shortcut::NewTab)
        );
    }

    #[test]
    fn the_browser_chords_resolve() {
        assert_eq!(
            resolve("l", "KeyL", true, false, false, false),
            Some(Shortcut::FocusAddressBar)
        );
        assert_eq!(
            resolve("d", "KeyD", true, false, false, false),
            Some(Shortcut::FocusAddressBar)
        );
        assert_eq!(
            resolve("r", "KeyR", true, false, false, false),
            Some(Shortcut::Reload)
        );
        assert_eq!(
            resolve("[", "BracketLeft", true, false, false, false),
            Some(Shortcut::Back)
        );
        assert_eq!(
            resolve("]", "BracketRight", true, false, false, false),
            Some(Shortcut::Forward)
        );
    }

    #[test]
    fn a_bare_key_is_never_a_shortcut() {
        assert_eq!(resolve("t", "KeyT", false, false, false, false), None);
        assert_eq!(resolve("w", "KeyW", false, false, false, false), None);
    }

    #[test]
    fn alt_is_left_to_the_platform() {
        assert_eq!(resolve("t", "KeyT", true, false, true, false), None);
    }

    #[test]
    fn an_unmapped_chord_is_ignored_rather_than_swallowed() {
        assert_eq!(resolve("j", "KeyJ", true, false, false, false), None);
    }

    #[test]
    fn the_physical_key_works_on_a_layout_that_moved_it() {
        // A layout where Cmd+W produces something other than "w" still closes.
        assert_eq!(
            resolve("\u{3c3}", "KeyW", true, false, false, false),
            Some(Shortcut::CloseTab)
        );
    }
}
