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
    if let Err(error) = serve::serve(&target) {
        eprintln!("chuzz-headless: {error}");
        std::process::exit(1);
    }
}

