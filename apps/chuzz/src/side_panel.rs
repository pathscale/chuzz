//! The right-hand panel.
//!
//! Structure follows AgencyZero's shell: a fixed-width column beside the
//! content area that collapses to a narrow rail rather than disappearing, so
//! the expand affordance is always on screen.
//!
//! The body is a placeholder. What belongs here (inspector, agent prompt,
//! history) is an open product decision, and the container is deliberately
//! agnostic about it.

use dioxus_native::prelude::*;

#[component]
pub fn SidePanel(collapsed: Signal<bool>) -> Element {
    let mut collapsed = collapsed;
    let is_collapsed = collapsed();

    let panel_class = if is_collapsed {
        "collapsed"
    } else {
        ""
    };
    // The chevron points the way the panel will move when clicked.
    let toggle_glyph = if is_collapsed { "\u{2039}" } else { "\u{203a}" };
    let toggle_title = if is_collapsed {
        "Expand panel"
    } else {
        "Collapse panel"
    };

    rsx!(
        div { id: "side-panel", class: "{panel_class}",
            div { id: "side-panel-header",
                if !is_collapsed {
                    div { id: "side-panel-title", "Panel" }
                }
                div {
                    id: "side-panel-toggle",
                    title: "{toggle_title}",
                    onclick: move |_| collapsed.toggle(),
                    "{toggle_glyph}"
                }
            }
            if !is_collapsed {
                div { id: "side-panel-body",
                    "Nothing here yet."
                }
            }
        }
    )
}
