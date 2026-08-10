//! The titlebar: back chevron, tabs, and the right-hand controls.
//!
//! Ported from AgencyZero's `TabStrip`. Tabs live in the window's title row
//! rather than under it, so there is no separate bar showing the page title.
//! Each tab carries a status dot; the close affordance is on the active tab
//! only, and the new-tab button is a dashed empty slot.

use blitz_traits::net::Url;
use dioxus_native::prelude::*;

use crate::tab::{Tab, TabId, TabStoreImplExt, tab_display_title};

#[component]
pub fn TitleBar(
    mut tabs: Store<Vec<Tab>>,
    mut active_tab_id: Signal<TabId>,
    home_url: Url,
    open_new_tab: Callback<Url>,
) -> Element {
    // Closing the active tab moves focus to the tab on its left, or to the
    // first remaining tab when the leftmost one closes.
    let close_tab = use_callback(move |id: TabId| {
        let open: Vec<TabId> = tabs.iter().map(|tab| tab.tab_id()).collect();
        if open.len() <= 1 {
            return;
        }
        let Some(index) = open.iter().position(|open_id| *open_id == id) else {
            return;
        };
        if id == active_tab_id() {
            let next = if index == 0 { 1 } else { index - 1 };
            active_tab_id.set(open[next]);
        }
        tabs.write().remove(index);
    });

    rsx!(
        div { id: "titlebar",
            div {
                id: "nav-back",
                title: "Back",
                onclick: move |_| {
                    let open: Vec<TabId> = tabs.iter().map(|tab| tab.tab_id()).collect();
                    if let Some(tab) = tabs.iter().find(|tab| tab.tab_id() == active_tab_id()) {
                        let _ = &open;
                        tab.go_back();
                    }
                },
                "\u{2039}"
            }
            div { id: "tab-strip",
                for tab in tabs.iter() {
                    {
                        let id = tab.tab_id();
                        let is_active = id == active_tab_id();
                        let class = if is_active { "tab active" } else { "tab" };
                        let dot_class = if tab.is_loading() { "tab-dot loading" } else { "tab-dot" };
                        rsx!(
                            div {
                                key: "{id}",
                                class: "{class}",
                                onclick: move |_| active_tab_id.set(id),
                                div { class: "{dot_class}" }
                                span { class: "tab-title", "{tab_display_title(tab)}" }
                                if is_active {
                                    div {
                                        class: "tab-close",
                                        onclick: move |event| {
                                            // Without this the click also selects
                                            // the tab that is about to be removed.
                                            event.stop_propagation();
                                            close_tab.call(id);
                                        },
                                        "\u{00d7}"
                                    }
                                }
                            }
                        )
                    }
                }
                div {
                    id: "new-tab",
                    title: "New tab",
                    onclick: move |_| open_new_tab.call(home_url.clone()),
                    "+"
                }
            }
            div { class: "titlebar-button", title: "Forward", "\u{203a}" }
            div { class: "titlebar-button", title: "Metrics", "\u{25d4}" }
            div { class: "titlebar-button", title: "Settings", "\u{2699}" }
            div { id: "avatar", "N" }
        }
    )
}
