// Do not open a console window alongside the browser on Windows.
#![cfg_attr(all(not(test), target_os = "windows"), windows_subsystem = "windows")]

use std::sync::Arc;

// The modules live in the library beside this binary, because the headless
// host is the same browser and compiles the same tree. See `lib.rs`.
#[cfg(feature = "capture")]
use chuzz::capture;
use chuzz::{browser, frontend, nav};

use browser::Browser;

fn window_spec() -> izumo::WindowSpec {
    izumo::WindowSpec {
        label: "main".to_owned(),
        identifier: "com.pathscale.chuzz".to_owned(),
        url: "chuzz://ui/".to_owned(),
        title: "Chuzz".to_owned(),
        width: 1440.0,
        height: 960.0,
        min_width: Some(960.0),
        min_height: Some(640.0),
        resizable: true,
        visible: false,
        transparent: false,
        title_bar_style: izumo::TitleBarStyle::Overlay,
        hidden_title: true,
        background_color: Some([9, 13, 18, 255]),
        ..Default::default()
    }
}

fn control_override_enabled() -> bool {
    matches!(
        std::env::var("CHUZZ_CONTROL").ok().as_deref(),
        Some("1") | Some("true") | Some("on")
    )
}

#[global_allocator]
static ALLOC: rusty_alloc::RustyAlloc = rusty_alloc::RustyAlloc;

#[path = "bin_caps.rs"]
mod app_caps;

fn main() {
    chuzz::wasm_retired::reject_flags();
    chuzz::platform_hardening::startup();
    chuzz::sandbox::startup();
    let _caps = app_caps::start(&[]);
    chuzz::tune_allocator();
    // Before any of the entry points below can reach the network. See the
    // function's own comment for why the provider is named here rather than
    // left to the resolution.
    chuzz::install_crypto_provider();

    // Keep the process worker pool and socket reactor alive for captures,
    // page loads, and cookie persistence.
    let _runtime = nagoya::runtime::background();
    let _reactor = nagoya::reactor::Reactor::start().unwrap_or_else(|error| {
        eprintln!("chuzz: could not start the network reactor: {error}");
        std::process::exit(1);
    });

    #[cfg(feature = "capture")]
    if let Some(index) = std::env::args().position(|arg| arg == "--capture") {
        let args: Vec<String> = std::env::args().collect();
        let output = args.get(index + 1).cloned().unwrap_or_else(|| {
            eprintln!("chuzz: --capture needs an output path");
            std::process::exit(2);
        });
        let url = args
            .iter()
            .enumerate()
            .skip(1)
            .find(|(position, arg)| {
                *position != index && *position != index + 1 && !arg.starts_with("--")
            })
            .map(|(_, arg)| arg.clone())
            .unwrap_or_else(|| nav::HOME_URL.to_owned());
        let (width, height) = capture::capture_viewport();
        let result = nagoya::block_on(capture::capture(
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

    // Worker tasks use the process pool and need no UI-thread runtime guard.
    // Attaching a page starts its subresource loads on this thread.
    let arguments: Vec<String> = std::env::args().collect();
    let startup = arguments
        .iter()
        .skip(1)
        .find_map(|argument| nav::request_from_input(argument).map(|request| request.url));
    let browser = Arc::new(Browser::new(startup));

    let cookie_directory = chuzz::cookie_store::profile_directory().join("cookies");
    if let Err(error) = nagoya::block_on(browser.open_cookie_profile(cookie_directory)) {
        eprintln!("chuzz: could not open browser cookies: {error}");
        std::process::exit(1);
    }

    let document_browser = Arc::clone(&browser);
    izumo::set_document_factory(move |url| {
        let document = frontend::document(document_browser.as_ref().clone(), url)?;
        let host = document.application_host();
        Ok(izumo::ApplicationDocument::new(Box::new(document), host))
    });

    let setup_browser = Arc::clone(&browser);
    let builder = izumo::App::builder()
        .manage(Arc::clone(&browser))
        .commands(browser::commands(Arc::clone(&browser)))
        .window(window_spec());

    // izumo owns the standard menu bar and forwards Edit keystrokes through
    // winit's focused responder. The chrome owns View Source for the active tab.
    #[cfg(target_os = "macos")]
    let builder = {
        let menu_handle = builder.handle();
        let menu_browser = Arc::clone(&browser);
        builder.menu(
            izumo::MenuBar::new("Chuzz")
                .on_menu_event(izumo::menu_id::VIEW_SOURCE, move || {
                    let _ = menu_handle.emit(izumo::menu_id::VIEW_SOURCE, ());
                })
                .on_menu_override(izumo::menu_id::CLOSE, move || {
                    browser::close_active_tab(Arc::clone(&menu_browser));
                }),
        )
    };

    let result = builder
        .setup(move |app| {
            setup_browser.attach_app(app.clone());
            // The Settings switches, as they were left, with `CHUZZ_CONTROL`
            // able to force inspection on but never off. That asymmetry is the
            // way back in: a window whose stored choice left inspection off can
            // still be started with the variable and reached.
            let stored = browser::stored_diagnostics();
            izumo::apply_runtime_debug_options(izumo::RuntimeDebugOptions {
                inspection_and_agent_control: control_override_enabled() || stored.inspection,
                deep_intrusive_profiling: stored.profiling,
            })
            .map_err(|error| format!("could not configure Blitz diagnostics: {error}"))?;
            app.show()?;
            Ok(())
        })
        .run();

    // Drain persistence before reporting a startup or event-loop failure too.
    if let Err(error) = nagoya::block_on(browser.flush_cookie_store()) {
        eprintln!("chuzz: could not flush browser cookies: {error}");
    }
    if let Err(error) = result {
        eprintln!("chuzz: browser failed: {error}");
        std::process::exit(1);
    }
}
