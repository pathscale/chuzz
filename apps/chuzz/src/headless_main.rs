//! Serve one page over the inspection socket, with no window.
//!
//! A second binary rather than a flag on the browser, because `ps-qa` launches
//! a host by running the path it was given with no arguments and
//! `QA_INSPECT_PAGE` in its environment. A flag would need that contract
//! changed on both sides; a binary is a drop-in for the `--host` it already
//! takes.
//!
//! Everything it does lives in [`serve`](serve::serve), which loads the page
//! through the same loader, the same web-API shim and the same engine a tab
//! uses. See that module for why the host is a mode of the browser instead of a
//! second one.

use chuzz::serve;

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
    // Before the loader can reach the network. See the function's own comment
    // for why the provider is named here rather than left to the resolution.
    chuzz::install_crypto_provider();
    chuzz::set_device_audio_enabled(false);
    chuzz::webauthn::set_authenticator(chuzz::webauthn::AuthenticatorChoice::Software);

    let args: Vec<String> = std::env::args().collect();
    let target = match serve::target_from(&args) {
        Ok(target) => target,
        Err(error) => {
            eprintln!("chuzz-headless: {error}");
            std::process::exit(2);
        }
    };
    if !serve::is_page_worker() {
        if let Err(error) = serve::serve(&target) {
            eprintln!("chuzz-headless: {error}");
            std::process::exit(1);
        }
        return;
    }
    // JavaScript frames live on a heap value stack. Native re-entry (a getter
    // or dispatchEvent nesting `Context::run`) holds this thread's stack. The
    // main thread's 8 MB ran out first on YouTube's Polymer, aborting the
    // worker, so the page runs on a thread sized for that nesting. The
    // reservation is virtual; pages are committed only as they are touched.
    let target = std::sync::Arc::new(target);
    // owner ruling: typed Arc<T> hand-off, concurrency only via nagoya/ps-st3; owner exception: one isolation thread per page JS engine.
    #[allow(clippy::disallowed_methods)]
    let page = std::thread::Builder::new()
        .name("chuzz-page".into())
        .stack_size(PAGE_STACK_BYTES)
        .spawn(move || {
            std::sync::Arc::new(serve::serve(&target).map_err(|error| error.to_string()))
        })
        .expect("spawn the page thread");
    let result = page.join().unwrap_or_else(|_| std::process::exit(101));
    if let Err(error) = result.as_ref() {
        eprintln!("chuzz-headless: {error}");
        std::process::exit(1);
    }
}

const PAGE_STACK_BYTES: usize = 256 * 1024 * 1024;
