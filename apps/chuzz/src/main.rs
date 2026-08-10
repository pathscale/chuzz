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
mod tab;
mod tab_strip;
mod toolbar;
mod ui;

use document_loader::NetProvider;
use nav::HOME_URL;
use tab::{Tab, TabId, TabStoreImplExt, TabView, active_tab, open_tab, tab_display_title};
use tab_strip::TabStrip;
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
        WindowAttributesMacOS::default()
            .with_titlebar_transparent(false)
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

    let url_input_value = use_signal(String::new);
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
        div { id: "frame",
            title { "{window_title}" }
            style { "{BROWSER_UI_CSS}" }
            TabStrip { tabs, active_tab_id, home_url, open_new_tab }
            Toolbar { tabs, active_tab_id, url_input_value }
            if is_loading {
                div { id: "loading-bar" }
            }
            div { id: "page-area",
                for tab in tabs.iter() {
                    TabView { key: "{tab.tab_id()}", tab, active_tab_id }
                }
            }
        }
    )
}
