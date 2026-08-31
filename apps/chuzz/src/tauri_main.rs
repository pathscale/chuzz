// Do not open a console window alongside the browser on Windows.
#![cfg_attr(all(not(test), target_os = "windows"), windows_subsystem = "windows")]

#[cfg(target_os = "macos")]
use tauri::Emitter as _;
use tauri::Manager;

/// The macOS menu bar, and the one item in it chuzz adds.
///
/// Cmd-U already worked: the binding lives in the Solid chrome, in
/// `resolveBrowserShortcut`, and reaches `runShortcut`. What was missing is the
/// menu entry, which is the only place a shortcut is *discoverable* on macOS.
/// Someone who does not already know the key has no way to find it.
///
/// The whole bar has to be built, not just the one submenu. Setting a menu
/// replaces Tauri's default wholesale, so leaving out the app submenu would
/// take Quit, Hide and About with it, and leaving out Edit would break Copy and
/// Paste in the address bar. Everything except View is predefined, so this adds
/// an entry rather than reimplementing a menu bar.
///
/// The item emits `menu-view-source` and the chrome runs it through the same
/// `runShortcut("view-source")` the keystroke does. A second implementation
/// here could drift from the keystroke's, and the interesting part of that
/// action, refusing to open `view-source:view-source:`, is a fact about the
/// active tab that the frontend knows and this does not.
#[cfg(target_os = "macos")]
fn build_menu<R: tauri::Runtime>(
    handle: &tauri::AppHandle<R>,
) -> tauri::Result<tauri::menu::Menu<R>> {
    use tauri::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu};

    let app = Submenu::with_items(
        handle,
        "Chuzz",
        true,
        &[
            &PredefinedMenuItem::about(handle, None, Some(AboutMetadata::default()))?,
            &PredefinedMenuItem::separator(handle)?,
            &PredefinedMenuItem::services(handle, None)?,
            &PredefinedMenuItem::separator(handle)?,
            &PredefinedMenuItem::hide(handle, None)?,
            &PredefinedMenuItem::hide_others(handle, None)?,
            &PredefinedMenuItem::show_all(handle, None)?,
            &PredefinedMenuItem::separator(handle)?,
            &PredefinedMenuItem::quit(handle, None)?,
        ],
    )?;

    // Without this the address bar cannot copy or paste: on macOS those are
    // menu-driven, and the webview never sees the keystroke if no item claims
    // it.
    let edit = Submenu::with_items(
        handle,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(handle, None)?,
            &PredefinedMenuItem::redo(handle, None)?,
            &PredefinedMenuItem::separator(handle)?,
            &PredefinedMenuItem::cut(handle, None)?,
            &PredefinedMenuItem::copy(handle, None)?,
            &PredefinedMenuItem::paste(handle, None)?,
            &PredefinedMenuItem::select_all(handle, None)?,
        ],
    )?;

    // `CmdOrCtrl+U` rather than `Cmd+U`, to match what the chrome accepts.
    let view_source = MenuItem::with_id(
        handle,
        MENU_VIEW_SOURCE,
        "View Source",
        true,
        Some("CmdOrCtrl+U"),
    )?;
    let view = Submenu::with_items(handle, "View", true, &[&view_source])?;

    let window = Submenu::with_items(
        handle,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(handle, None)?,
            &PredefinedMenuItem::close_window(handle, None)?,
        ],
    )?;

    Menu::with_items(handle, &[&app, &edit, &view, &window])
}

/// The menu item's id, and the event the chrome listens for. One constant so
/// the two cannot drift apart.
#[cfg(target_os = "macos")]
const MENU_VIEW_SOURCE: &str = "menu-view-source";

mod browser;
#[cfg(feature = "capture")]
mod capture;
mod decode;
mod document_loader;
// `capture` writes the tree beside the PNG, so the two arrive together or the
// pixels have nothing to be explained by.
#[cfg(feature = "capture")]
mod dump;
mod frontend;
mod nav;
// Shared by `--wasm` in the window and `--capture-wasm` headlessly, so a
// guest-built page cannot render one way in a tab and another in a capture.
#[cfg(feature = "wasm")]
mod wasm_page;

use browser::Browser;

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

fn main() {
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
        // No tokio runtime: nothing here is fetched, so nothing here awaits.
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
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("capture needs a tokio runtime");
        let (width, height) = capture::capture_viewport();
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

    // `--wasm <module.wasm>`: the first tab's document is built by a
    // WebAssembly guest instead of fetched. Everything after this point is the
    // ordinary browser: the tab, the strip, the toolbar and the mount are the
    // same ones a fetched page uses.
    let arguments: Vec<String> = std::env::args().collect();
    if arguments.iter().any(|arg| arg == "--wasm") && flag_value(&arguments, "--wasm").is_none() {
        eprintln!("chuzz: --wasm needs a path to a .wasm module");
        std::process::exit(2);
    }
    let browser = match flag_value(&arguments, "--wasm") {
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
    };
    let document_browser = browser.clone();
    tauri_runtime_blitz::set_document_factory(move |url| {
        frontend::document(document_browser.clone(), url)
    });

    // The UI thread has to be inside a Tokio runtime for the whole run.
    //
    // Attaching a fetched page to its `<web-view>` happens here, on this
    // thread, and the page's document starts loading its own subresources as
    // soon as it is attached. Blitz's net provider issues those with
    // `tokio::spawn`, which panics outright when no reactor is entered:
    // "there is no reactor running". That is not a fetch that fails and
    // reports, it is the window going away.
    //
    // Tauri's own runtime rather than a second one of our own: `spawn` calls
    // elsewhere in the browser already go there, and two runtimes would mean
    // page loads and commands running on separate thread pools with separate
    // shutdown.
    let runtime = tauri::async_runtime::handle();
    let _runtime_guard = runtime.inner().enter();

    let setup_browser = browser.clone();
    tauri_runtime_blitz::builder()
        .manage(browser)
        .invoke_handler(tauri::generate_handler![
            browser::list_tabs,
            browser::active_tab_id,
            browser::select_tab,
            browser::open_tab,
            browser::close_tab,
            browser::navigate,
            browser::go_back,
            browser::go_forward,
            browser::reload,
            browser::panel_state,
            browser::set_panel_collapsed,
            browser::toggle_section,
            browser::status,
            browser::debug_log,
            browser::diagnostics,
            browser::set_diagnostics,
        ])
        .setup(move |app| {
            setup_browser.attach_app(app.handle().clone());
            // macOS only: it is the only platform here with a menu bar, and the
            // item exists to make Cmd-U discoverable rather than to add a
            // second way of doing it.
            #[cfg(target_os = "macos")]
            {
                let menu = build_menu(app.handle())?;
                app.set_menu(menu)?;
                app.on_menu_event(|app, event| {
                    if event.id() == MENU_VIEW_SOURCE {
                        // Emitted rather than handled here: the chrome owns
                        // what view-source means for the active tab.
                        let _ = app.emit(MENU_VIEW_SOURCE, ());
                    }
                });
            }
            // The Settings switches, as they were left, with `CHUZZ_CONTROL`
            // able to force inspection on but never off. That asymmetry is the
            // way back in: a window whose stored choice left inspection off can
            // still be started with the variable and reached.
            let stored = browser::stored_diagnostics();
            tauri_runtime_blitz::apply_runtime_debug_options(
                tauri_runtime_blitz::RuntimeDebugOptions {
                    inspection_and_agent_control: control_override_enabled() || stored.inspection,
                    deep_intrusive_profiling: stored.profiling,
                },
            )
            .map_err(|error| format!("could not configure Blitz diagnostics: {error}"))?;
            if let Some(window) = app.get_webview_window("main") {
                window.show()?;
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build Chuzz")
        .run(|_, _| {});
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    /// The menu item's id is also the event name the chrome listens for, and
    /// the two live in different languages: `MENU_VIEW_SOURCE` here, and the
    /// `"menu-view-source"` key of `BrowserEvents` in `api/client.ts`.
    ///
    /// Nothing else connects them. Renaming one and not the other leaves a menu
    /// item that emits into the void, the item stays enabled, clicking it does
    /// nothing, and no error is reported anywhere. So the string is pinned
    /// against the TypeScript that consumes it, read from disk rather than
    /// copied here, because a copy would agree with itself forever.
    #[test]
    fn the_menu_id_matches_the_event_the_chrome_listens_for() {
        let client = include_str!("../frontend/src/api/client.ts");
        assert!(
            client.contains(&format!("\"{}\":", super::MENU_VIEW_SOURCE)),
            "`{}` is not declared in BrowserEvents; the menu item would emit an \
             event nothing is listening for",
            super::MENU_VIEW_SOURCE
        );

        let store = include_str!("../frontend/src/stores/browser.tsx");
        assert!(
            store.contains(&format!("api.on(\"{}\"", super::MENU_VIEW_SOURCE)),
            "the chrome does not subscribe to `{}`",
            super::MENU_VIEW_SOURCE
        );

        // And the mock has to carry it too, or a dev build throws on
        // registration instead of quietly having no menu.
        let mock = include_str!("../frontend/src/api/mock.ts");
        assert!(
            mock.contains(&format!("\"{}\":", super::MENU_VIEW_SOURCE)),
            "the mock api has no handler set for `{}`",
            super::MENU_VIEW_SOURCE
        );
    }
}
