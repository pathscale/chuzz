//! The status strip along the bottom of the window.
//!
//! Ported from AgencyZero's footer readout: a live dot, the current state in
//! prose on the left, and monospace counters on the right. Numbers other than
//! the URL are mock.

use dioxus_native::prelude::*;

#[component]
pub fn StatusStrip(is_loading: bool, current_url: String, tab_count: usize) -> Element {
    let dot_class = if is_loading {
        "status-dot loading"
    } else {
        "status-dot"
    };
    let state = if is_loading { "loading" } else { "idle" };

    rsx!(
        div { id: "status-strip",
            div { class: "{dot_class}" }
            span { "{state}" }
            span { "\u{b7}" }
            span { "{current_url}" }
            div { class: "status-spacer" }
            span { "{tab_count} tabs" }
            span { "\u{b7}" }
            span { "18 nodes" }
            span { "\u{b7}" }
            span { class: "status-accent", "1.2 kB" }
            span { "\u{b7}" }
            // Which build this is, in the window itself.
            //
            // AgencyZero reports the same three facts through an About menu
            // item; there is no menu bar here, so they live in the strip where
            // they cost no click. The version is bumped by hand and a stale
            // bundle looks identical to a fresh one, so the sha and the
            // timestamp are what actually answer "am I testing the fix?".
            span {
                title: "built {env!(\"CHUZZ_BUILT_AT\")}",
                "v{env!(\"CARGO_PKG_VERSION\")} {env!(\"CHUZZ_GIT_SHA\")}"
            }
        }
    )
}
