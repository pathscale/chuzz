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

/// The value following `flag`, for the hand-rolled argument parsing.
fn flag_value(args: &[String], flag: &str) -> Option<String> {
    let index = args.iter().position(|arg| arg == flag)?;
    args.get(index + 1).cloned()
}

#[global_allocator]
static ALLOC: rusty_alloc::RustyAlloc = rusty_alloc::RustyAlloc;

fn main() {
    chuzz::tune_allocator();
    // Before any of the entry points below can reach the network. See the
    // function's own comment for why the provider is named here rather than
    // left to the resolution.
    chuzz::install_crypto_provider();

    // Before `--capture`, and matched by equality rather than by prefix, so the
    // two flags cannot be confused for each other in either direction.
    #[cfg(feature = "capture")]
    if let Some(index) = std::env::args().position(|arg| arg == "--capture-wasm") {
        let args: Vec<String> = std::env::args().collect();
        let module = args.get(index + 1).cloned().unwrap_or_else(|| {
            eprintln!("chuzz: --capture-wasm needs a path to a .wasm module");
            std::process::exit(2);
        });
        let output = flag_value(&args, "--out").unwrap_or_else(|| {
            eprintln!("chuzz: --capture-wasm needs --out <page.png>");
            std::process::exit(2);
        });
        // `--tree` is a real flag here rather than the environment variable
        // `--capture` has to use: that path takes the URL as a positional, so a
        // second one would have been ambiguous. This one does not.
        let tree = flag_value(&args, "--tree").or_else(|| {
            std::env::var("CHUZZ_CAPTURE_TREE")
                .ok()
                .filter(|path| !path.is_empty())
        });
        // Nothing here is fetched, so this path needs no runtime.
        let result = capture::capture_wasm(
            std::path::Path::new(&module),
            1440,
            960,
            std::path::Path::new(&output),
            tree.as_deref().map(std::path::Path::new),
        );
        match result {
            Ok(()) => match &tree {
                Some(tree) => println!("chuzz: wrote {output} and {tree}"),
                None => println!("chuzz: wrote {output}"),
            },
            Err(error) => {
                eprintln!("chuzz: wasm capture failed: {error}");
                std::process::exit(1);
            }
        }
        return;
    }

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
    // `--wasm <module.wasm>`: the first tab's document is built by a
    // WebAssembly guest instead of fetched. Everything after this point is the
    // ordinary browser: the tab, the strip, the toolbar and the mount are the
    // same ones a fetched page uses.
    let arguments: Vec<String> = std::env::args().collect();
    if arguments.iter().any(|arg| arg == "--wasm") && flag_value(&arguments, "--wasm").is_none() {
        eprintln!("chuzz: --wasm needs a path to a .wasm module");
        std::process::exit(2);
    }
    let browser = Arc::new(match flag_value(&arguments, "--wasm") {
        Some(module) => match Browser::with_wasm_page(std::path::PathBuf::from(module)) {
            Ok(browser) => browser,
            Err(error) => {
                eprintln!("chuzz: {error}");
                std::process::exit(2);
            }
        },
        None => {
            let startup = arguments
                .iter()
                .skip(1)
                .find_map(|argument| nav::request_from_input(argument).map(|request| request.url));
            Browser::new(startup)
        }
    });

    let cookie_directory = chuzz::cookie_store::profile_directory().join("cookies");
    if let Err(error) = nagoya::block_on(browser.open_cookie_profile(cookie_directory)) {
        eprintln!("chuzz: could not open browser cookies: {error}");
        std::process::exit(1);
    }

    let document_browser = Arc::clone(&browser);
    izumo::set_document_factory(move |url| {
        frontend::document(document_browser.as_ref().clone(), url)
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
        builder.menu(izumo::MenuBar::new("Chuzz").on_menu_event(
            izumo::menu_id::VIEW_SOURCE,
            move || {
                let _ = menu_handle.emit(izumo::menu_id::VIEW_SOURCE, ());
            },
        ))
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
