//! Back, forward, reload, and the address bar.

use dioxus_native::prelude::*;

use crate::nav::request_from_input;
use crate::tab::{Tab, TabId, TabStoreImplExt, active_tab};

fn is_enter(key: &Key) -> bool {
    matches!(key, Key::Enter) || matches!(key, Key::Character(text) if text == "\n")
}

#[component]
pub fn Toolbar(
    tabs: Store<Vec<Tab>>,
    active_tab_id: Signal<TabId>,
    mut url_input_value: Signal<String>,
) -> Element {
    // The address bar follows the active tab, but only when that tab actually
    // navigates. Tracking the URL itself is what makes typing possible: an
    // effect that re-ran on every render would overwrite each keystroke with
    // the current URL, and the field could never hold anything typed.
    let current_url = active_tab(tabs, active_tab_id()).current_url().to_string();
    use_effect(use_reactive!(|current_url| {
        *url_input_value.write_unchecked() = current_url;
    }));

    let tab = active_tab(tabs, active_tab_id());
    let can_go_back = tab.can_go_back();
    let can_go_forward = tab.can_go_forward();

    let submit = use_callback(move |_| {
        let typed = url_input_value.read().clone();
        if let Some(req) = request_from_input(&typed) {
            active_tab(tabs, active_tab_id()).navigate(req);
        }
    });

    let back_class = if can_go_back {
        "tool-button"
    } else {
        "tool-button disabled"
    };
    let forward_class = if can_go_forward {
        "tool-button"
    } else {
        "tool-button disabled"
    };

    rsx!(
        div { id: "toolbar",
            div {
                class: "{back_class}",
                title: "Back",
                onclick: move |_| active_tab(tabs, active_tab_id()).go_back(),
                "\u{2190}"
            }
            div {
                class: "{forward_class}",
                title: "Forward",
                onclick: move |_| active_tab(tabs, active_tab_id()).go_forward(),
                "\u{2192}"
            }
            div {
                class: "tool-button",
                title: "Reload",
                onclick: move |_| active_tab(tabs, active_tab_id()).reload(),
                "\u{21bb}"
            }
            input {
                id: "url-bar",
                r#type: "text",
                value: "{url_input_value}",
                placeholder: "Search or enter address",
                oninput: move |event| url_input_value.set(event.value()),
                onkeydown: move |event| {
                    if is_enter(&event.key()) {
                        submit.call(());
                    }
                },
            }
        }
    )
}
