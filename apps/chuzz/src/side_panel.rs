//! The right-hand panel, ported from AgencyZero's `ProjectPanel`.
//!
//! Structure is theirs: a scrolling column of accordion sections, each a
//! bordered card with icon, title, count badge, and a chevron that flips
//! between up and down. A section's body only renders while open, so a
//! collapsed section is a header and nothing else.
//!
//! Contents are mock data. The shape is real; the rows are placeholders.

use dioxus_native::prelude::*;

/// Which sections start open. AgencyZero persists this in `UiPrefs` so the
/// panel you left open stays open; here it is process state.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PanelSections {
    pub page: bool,
    pub history: bool,
    pub network: bool,
    pub console: bool,
}

impl Default for PanelSections {
    fn default() -> Self {
        Self {
            page: true,
            history: true,
            network: false,
            console: false,
        }
    }
}

#[component]
pub fn SidePanel(collapsed: Signal<bool>, sections: Signal<PanelSections>) -> Element {
    let mut collapsed = collapsed;
    let is_collapsed = collapsed();

    // Collapsed, the panel stays on screen as a narrow rail: the affordance to
    // bring it back has to remain reachable.
    if is_collapsed {
        return rsx!(
            div { id: "side-panel", class: "collapsed",
                div {
                    class: "panel-rail-toggle",
                    title: "Expand panel",
                    onclick: move |_| collapsed.toggle(),
                    "\u{2039}"
                }
            }
        );
    }

    rsx!(
        div { id: "side-panel",
            div { id: "side-panel-header",
                div { id: "side-panel-title", "Inspector" }
                div {
                    id: "side-panel-toggle",
                    title: "Collapse panel",
                    onclick: move |_| collapsed.toggle(),
                    "\u{203a}"
                }
            }
            div { id: "side-panel-scroll",
                SectionPanel {
                    title: "Page",
                    count: 4,
                    count_tone: "primary",
                    is_open: sections().page,
                    on_toggle: move |_| {
                        let mut next = sections();
                        next.page = !next.page;
                        sections.set(next);
                    },
                    MockRows { rows: PAGE_ROWS }
                }
                SectionPanel {
                    title: "History",
                    count: 3,
                    count_tone: "neutral",
                    is_open: sections().history,
                    on_toggle: move |_| {
                        let mut next = sections();
                        next.history = !next.history;
                        sections.set(next);
                    },
                    MockRows { rows: HISTORY_ROWS }
                }
                SectionPanel {
                    title: "Network",
                    count: 6,
                    count_tone: "neutral",
                    is_open: sections().network,
                    on_toggle: move |_| {
                        let mut next = sections();
                        next.network = !next.network;
                        sections.set(next);
                    },
                    MockRows { rows: NETWORK_ROWS }
                }
                SectionPanel {
                    title: "Console",
                    count: 0,
                    count_tone: "neutral",
                    is_open: sections().console,
                    on_toggle: move |_| {
                        let mut next = sections();
                        next.console = !next.console;
                        sections.set(next);
                    },
                    MockRows { rows: CONSOLE_ROWS }
                }
            }
        }
    )
}

/// One accordion section: a card whose header is always present and whose body
/// exists only while open.
#[component]
fn SectionPanel(
    title: String,
    count: u32,
    count_tone: String,
    is_open: bool,
    on_toggle: EventHandler<()>,
    children: Element,
) -> Element {
    let chevron = if is_open { "\u{2303}" } else { "\u{2304}" };
    let badge_class = if count_tone == "primary" {
        "section-count primary"
    } else {
        "section-count"
    };

    rsx!(
        div { class: "section-panel",
            div {
                class: "section-header",
                onclick: move |_| on_toggle.call(()),
                span { class: "section-title", "{title}" }
                span { class: "{badge_class}", "{count}" }
                span { class: "section-chevron", "{chevron}" }
            }
            if is_open {
                div { class: "section-body", {children} }
            }
        }
    )
}

const PAGE_ROWS: &[(&str, &str)] = &[
    ("Title", "Example Domain"),
    ("Elements", "18 nodes"),
    ("Stylesheets", "1"),
    ("Images", "0"),
];

const HISTORY_ROWS: &[(&str, &str)] = &[
    ("example.com", "now"),
    ("duckduckgo.com", "2m"),
    ("rust-lang.org", "11m"),
];

const NETWORK_ROWS: &[(&str, &str)] = &[
    ("GET /", "200 \u{b7} 1.2 kB"),
    ("GET /style.css", "200 \u{b7} 0.4 kB"),
    ("GET /favicon.ico", "404"),
];

const CONSOLE_ROWS: &[(&str, &str)] = &[];

#[component]
fn MockRows(rows: &'static [(&'static str, &'static str)]) -> Element {
    if rows.is_empty() {
        return rsx!(div { class: "section-empty", "Nothing recorded." });
    }
    rsx!(
        for (label, value) in rows.iter() {
            div { class: "section-row",
                span { class: "section-row-label", "{label}" }
                span { class: "section-row-value", "{value}" }
            }
        }
    )
}
