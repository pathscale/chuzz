// Do not open a console window alongside the browser on Windows.
#![cfg_attr(all(not(test), target_os = "windows"), windows_subsystem = "windows")]

use tauri::Manager;

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

use browser::Browser;

fn control_override_enabled() -> bool {
    matches!(
        std::env::var("CHUZZ_CONTROL").ok().as_deref(),
        Some("1") | Some("true") | Some("on")
    )
}

fn main() {
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
        let result = runtime.block_on(capture::capture(
            &url,
            1440,
            960,
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

    let startup = std::env::args()
        .skip(1)
        .find_map(|argument| nav::request_from_input(&argument).map(|request| request.url));
    let browser = Browser::new(startup);
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
            browser::diagnostics,
            browser::set_diagnostics,
        ])
        .setup(move |app| {
            setup_browser.attach_app(app.handle().clone());
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
