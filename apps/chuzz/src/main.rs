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
use dioxus_native::{LogicalSize, WindowAttributes, prelude::*, use_window_event};

#[cfg(target_os = "macos")]
use dioxus_native::winit::platform::macos::WindowAttributesMacOS;

#[cfg(feature = "capture")]
mod capture;
mod control;
mod decode;
mod document_loader;
#[cfg(feature = "capture")]
mod dump;
mod history;
#[cfg(target_os = "macos")]
mod menu;
mod nav;
mod shortcuts;
mod side_panel;
mod status_strip;
mod tab;
mod tab_strip;
mod toolbar;
mod ui;

use control::ControlHandle;
use document_loader::NetProvider;
use nav::NEW_TAB_URL;
use shortcuts::{apply, resolve};
use side_panel::{PanelEdgeHandle, PanelSections, SidePanel};
use status_strip::StatusStrip;
use tab::{
    Tab, TabId, TabStoreExt, TabStoreImplExt, TabView, active_tab, open_tab, tab_display_title,
};
use tab_strip::TitleBar;
use toolbar::Toolbar;
use ui::BROWSER_UI_CSS;

/// The URL named on the command line, handed to the first tab.
#[derive(Clone)]
struct StartupUrl(Option<Url>);

fn main() {
    // `--capture <file>` renders the page headlessly and exits. It shares the
    // browser's loader, so what it writes is what a tab would show.
    #[cfg(feature = "capture")]
    if let Some(index) = std::env::args().position(|arg| arg == "--capture") {
        let args: Vec<String> = std::env::args().collect();
        let output = args.get(index + 1).cloned().unwrap_or_else(|| {
            eprintln!("chuzz: --capture needs an output path");
            std::process::exit(2);
        });
        // Skip the flag and its value explicitly. Comparing against `output`
        // here was a type error that always compared unequal, so the output
        // path itself was taken as the URL and quietly navigated to.
        let url = args
            .iter()
            .enumerate()
            .skip(1)
            .find(|(position, arg)| {
                *position != index && *position != index + 1 && !arg.starts_with("--")
            })
            .map(|(_, arg)| arg.clone())
            .unwrap_or_else(|| nav::HOME_URL.to_owned());
        let width = std::env::var("CHUZZ_CAPTURE_WIDTH")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(1440);
        let height = std::env::var("CHUZZ_CAPTURE_HEIGHT")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(960);

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("capture needs a tokio runtime");
        let result = runtime.block_on(capture::capture(
            &url,
            width,
            height,
            std::path::Path::new(&output),
        ));
        match result {
            Ok(()) => println!("chuzz: wrote {output}"),
            Err(error) => {
                eprintln!("chuzz: capture failed: {error}");
                std::process::exit(1);
            }
        }
        return;
    }

    let startup_url = std::env::args()
        .skip(1)
        .find_map(|argument| nav::request_from_input(&argument).map(|req| req.url));

    // Wide enough that a desktop layout renders as its author intended: at
    // 770px the responsive breakpoints collapse to the tablet layout, which
    // makes any comparison against a desktop browser meaningless.
    let window_attributes = WindowAttributes::default()
        .with_title("Chuzz")
        .with_surface_size(LogicalSize::new(1440.0, 960.0));
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
    // The new tab button opens a blank page, not the home page: a new tab
    // should cost nothing until you ask it for something. Startup is the same:
    // opening the browser is not a request to load a site, and starting on one
    // means every launch pays for a fetch nobody asked for. Name a URL on the
    // command line, or type one.
    // `Url::parse` of a constant literal cannot fail.
    #[allow(clippy::expect_used)]
    let new_tab_url = use_hook(|| Url::parse(NEW_TAB_URL).expect("blank URL is a valid constant"));

    // Built here rather than in `main`: attaching a menu needs a live NSApp,
    // and that only exists once `launch_cfg` has created the event loop. A
    // `use_hook` runs on the main thread on first render, which is the first
    // moment both are true.
    //
    // The handle it returns is kept, not discarded: the hook outlives the app,
    // and dropping the menu leaves NSApp holding pointers into freed memory.
    // See `menu::install`.
    #[cfg(target_os = "macos")]
    let _menu = use_hook(menu::install);

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
        let first_url = startup_url.clone().unwrap_or_else(|| new_tab_url.clone());
        Signal::new(open_tab(tabs, first_url, net_provider.clone()).tab_id())
    });

    let mut focus_new_tab_bar = focus_address_bar;
    let open_new_tab = use_callback(move |url: Url| {
        let opened = open_tab(tabs, url, net_provider.clone());
        active_tab_id.set(opened.tab_id());
        // A new tab is a request to go somewhere, and the only thing that can
        // say where is the address bar. Opening one focused means typing works
        // immediately, instead of needing a click first.
        focus_new_tab_bar.set(true);
    });

    // The control socket is opened once and polled on a timer.
    //
    // Not during render: `doc_mut` takes the document's RefCell, which Dioxus
    // still holds while a component body runs, and borrowing it there panics.
    // Not from a plain effect either: an effect re-runs only when a signal it
    // read changes, so on an idle page nothing would ever drain the queue and
    // every request would time out.
    let control = use_hook(|| ControlHandle::start().map(std::rc::Rc::new));
    {
        let control = control.clone();
        use_future(move || {
            let control = control.clone();
            async move {
                let Some(control) = control else { return };
                loop {
                    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                    if !control.has_pending() {
                        continue;
                    }
                    let tab = active_tab(tabs, active_tab_id());
                    let url = tab.current_url().to_string();
                    let title = tab.title().cloned();
                    let Some(handle) = tab.node_handle().cloned() else {
                        control.drain_unavailable();
                        continue;
                    };
                    // Fallible: the animation clock borrows the same document.
                    let Some(mut doc) = handle.try_doc_mut() else {
                        continue;
                    };
                    if let Some(sub) = doc.subdoc_mut(handle.node_id()) {
                        control.service(&sub.inner_mut(), Some(url), Some(title));
                    } else {
                        drop(doc);
                        control.drain_unavailable();
                    }
                }
            }
        });
    }

    // Shortcuts come from the window, not the document: the frame is a div, so
    // it only receives keys while focused, and nothing focuses it at startup.
    // A browser's chords have to work the moment the window opens.
    //
    // This is the *only* place they are handled. `#frame` used to carry an
    // onkeydown doing the same resolve-and-apply, which was not a fallback but
    // a duplicate: this handler runs for every key the window receives
    // regardless of focus, so once anything focused the frame both fired and
    // every chord happened twice. Ctrl+T opened two tabs, Cmd+W closed two.
    //
    // The DOM handler's prevent_default was not protecting the page either.
    // #frame is an ancestor of the page area, so a key aimed at the page has
    // already been delivered by the time it bubbles up here.
    {
        let net = shortcut_net_provider.clone();
        let mut modifiers_state = std::rc::Rc::new(std::cell::Cell::new(
            dioxus_native::winit::keyboard::ModifiersState::empty(),
        ));
        let tracked = std::rc::Rc::clone(&modifiers_state);
        use_window_event(move |event, _target| {
            use dioxus_native::winit::event::{ElementState, WindowEvent};
            use dioxus_native::winit::keyboard::Key;
            match event {
                WindowEvent::ModifiersChanged(new) => tracked.set(new.state()),
                WindowEvent::KeyboardInput { event, .. }
                    if event.state == ElementState::Pressed =>
                {
                    let mods = tracked.get();
                    let key = match &event.logical_key {
                        Key::Character(text) => text.to_string(),
                        _ => String::new(),
                    };
                    let code = format!("{:?}", event.physical_key);
                    let code = code
                        .strip_prefix("Code(")
                        .and_then(|rest| rest.strip_suffix(')'))
                        .unwrap_or(code.as_str())
                        .to_owned();
                    if let Some(shortcut) = resolve(
                        &key,
                        &code,
                        mods.meta_key(),
                        mods.control_key(),
                        mods.alt_key(),
                        mods.shift_key(),
                    ) {
                        apply(
                            shortcut,
                            tabs,
                            active_tab_id,
                            focus_address_bar,
                            net.clone(),
                        );
                    }
                }
                _ => {}
            }
        });
        let _ = &mut modifiers_state;
    }

    let window_title = tab_display_title(active_tab(tabs, active_tab_id()));
    let is_loading = active_tab(tabs, active_tab_id()).is_loading();

    rsx!(
        div {
            id: "frame",
            tabindex: 0,
            title { "{window_title}" }
            // A bare `style {}` element does not reach the document: Dioxus
            // overloads `style` as an attribute namespace, so the stylesheet is
            // silently dropped and the whole UI renders unstyled. `document::Style`
            // routes inline CSS through the supported head path instead.
            document::Style { "{BROWSER_UI_CSS}" }
            TitleBar { tabs, active_tab_id, home_url: new_tab_url, open_new_tab }
            Toolbar { tabs, active_tab_id, url_input_value, focus_address_bar }
            if is_loading {
                div { id: "loading-bar" }
            }
            div { id: "content-row",
                div { id: "page-area",
                    for tab in tabs.iter() {
                        TabView { key: "{tab.tab_id()}", tabs, tab, active_tab_id }
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
