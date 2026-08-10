// Do not open a console window alongside the browser on Windows.
#![cfg_attr(all(not(test), target_os = "windows"), windows_subsystem = "windows")]

//! Chuzz: a pure Rust web browser.
//!
//! Page rendering is Blitz (HTML parsing, Stylo CSS, Taffy layout, Vello
//! painting). The browser UI is a Blitz document of its own, driven by
//! Dioxus Native. Everything a browser decides rather than renders (tabs,
//! session history, what a typed address means) lives in this binary.

use std::sync::Arc;

use blitz_traits::net::Url;
use dioxus_native::{WindowAttributes, prelude::*};

#[cfg(target_os = "macos")]
use dioxus_native::winit::platform::macos::WindowAttributesMacOS;

mod document_loader;
mod history;
mod nav;
mod shortcuts;
mod side_panel;
mod status_strip;
mod tab;
mod tab_strip;
mod toolbar;
mod ui;

use document_loader::NetProvider;
use nav::HOME_URL;
use shortcuts::{apply, resolve};
use side_panel::{PanelEdgeHandle, PanelSections, SidePanel};
use status_strip::StatusStrip;
use tab::{Tab, TabId, TabStoreImplExt, TabView, active_tab, open_tab, tab_display_title};
use tab_strip::TitleBar;
use toolbar::Toolbar;
use ui::BROWSER_UI_CSS;

/// The URL named on the command line, handed to the first tab.
#[derive(Clone)]
struct StartupUrl(Option<Url>);

fn main() {
    let startup_url = std::env::args()
        .skip(1)
        .find_map(|argument| nav::request_from_input(&argument).map(|req| req.url));

    let window_attributes = WindowAttributes::default().with_title("Chuzz");
    #[cfg(target_os = "macos")]
    let window_attributes = window_attributes.with_platform_attributes(Box::new(
        // Tabs live in the title row, so the native titlebar is hidden and the
        // content view is extended to full size behind the traffic lights.
        WindowAttributesMacOS::default()
            .with_titlebar_transparent(true)
            .with_fullsize_content_view(true)
            .with_title_hidden(true)
            .with_unified_titlebar(true),
    ));

    let startup = StartupUrl(startup_url);
    let contexts: Vec<Box<dyn Fn() -> Box<dyn std::any::Any> + Send + Sync>> =
        vec![Box::new(move || {
            Box::new(startup.clone()) as Box<dyn std::any::Any>
        })];

    dioxus_native::launch_cfg(app, contexts, vec![Box::new(window_attributes)]);
}

fn app() -> Element {
    // `Url::parse` of a constant literal cannot fail.
    #[allow(clippy::expect_used)]
    let home_url = use_hook(|| Url::parse(HOME_URL).expect("home URL is a valid constant"));
    let startup_url = use_hook(|| try_consume_context::<StartupUrl>().and_then(|ctx| ctx.0));
    let net_provider = use_context::<Arc<NetProvider>>();
    let shortcut_net_provider = net_provider.clone();
    let focus_address_bar = use_signal(|| false);

    let url_input_value = use_signal(String::new);
    // Collapsed by default: the page gets the full width until asked otherwise.
    let panel_collapsed = use_signal(|| true);
    let panel_sections = use_signal(PanelSections::default);
    let tabs: Store<Vec<Tab>> = use_store(Vec::new);

    let mut active_tab_id: Signal<TabId> = use_hook(|| {
        let first_url = startup_url.clone().unwrap_or_else(|| home_url.clone());
        Signal::new(open_tab(tabs, first_url, net_provider.clone()).tab_id())
    });

    let open_new_tab = use_callback(move |url: Url| {
        let opened = open_tab(tabs, url, net_provider.clone());
        active_tab_id.set(opened.tab_id());
    });

    let window_title = tab_display_title(active_tab(tabs, active_tab_id()));
    let is_loading = active_tab(tabs, active_tab_id()).is_loading();

    rsx!(
        div {
            id: "frame",
            tabindex: 0,
            onkeydown: move |event| {
                let modifiers = event.modifiers();
                if let Some(shortcut) = resolve(
                    &event.key().to_string(),
                    &event.code().to_string(),
                    modifiers.meta(),
                    modifiers.ctrl(),
                    modifiers.alt(),
                    modifiers.shift(),
                ) {
                    event.prevent_default();
                    apply(
                        shortcut,
                        tabs,
                        active_tab_id,
                        focus_address_bar,
                        shortcut_net_provider.clone(),
                    );
                }
            },
            title { "{window_title}" }
            // A bare `style {}` element does not reach the document: Dioxus
            // overloads `style` as an attribute namespace, so the stylesheet is
            // silently dropped and the whole UI renders unstyled. `document::Style`
            // routes inline CSS through the supported head path instead.
            document::Style { "{BROWSER_UI_CSS}" }
            TitleBar { tabs, active_tab_id, home_url, open_new_tab }
            Toolbar { tabs, active_tab_id, url_input_value }
            if is_loading {
                div { id: "loading-bar" }
            }
            div { id: "content-row",
                div { id: "page-area",
                    for tab in tabs.iter() {
                        TabView { key: "{tab.tab_id()}", tab, active_tab_id }
                    }
                    PanelEdgeHandle { collapsed: panel_collapsed }
                }
                SidePanel { collapsed: panel_collapsed, sections: panel_sections }
            }
            StatusStrip {
                is_loading,
                current_url: active_tab(tabs, active_tab_id()).current_url().to_string(),
                tab_count: tabs.iter().count(),
            }
        }
    )
}
