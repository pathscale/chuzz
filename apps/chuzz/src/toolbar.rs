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
    mut focus_address_bar: Signal<bool>,
) -> Element {
    // The address bar's element handle, captured when it mounts. A shortcut can
    // only ask for focus by name; something has to hold the node it names.
    let mut address_bar: Signal<Option<std::rc::Rc<MountedData>>> = use_signal(|| None);
    use_effect(move || {
        if !focus_address_bar() {
            return;
        }
        if let Some(element) = address_bar() {
            spawn(async move {
                let _ = element.set_focus(true).await;
            });
        }
        // Cleared whether or not the element was there: the signal is a request,
        // not a state, and leaving it set would make the next request a no-op
        // because the effect only re-runs on a change.
        focus_address_bar.set(false);
    });
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
                onmounted: move |event| address_bar.set(Some(event.data())),
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
