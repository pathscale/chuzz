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

fn main() {
    // Before the loader can reach the network. See the function's own comment
    // for why the provider is named here rather than left to the resolution.
    chuzz::install_crypto_provider();
    chuzz::set_device_audio_enabled(false);

    let args: Vec<String> = std::env::args().collect();
    let target = match serve::target_from(&args) {
        Ok(target) => target,
        Err(error) => {
            eprintln!("chuzz-headless: {error}");
            std::process::exit(2);
        }
    };
    // The script runtime allows about 8000 JS frames, and a frame that calls
    // through a native accessor back into JS also holds Rust stack. The main
    // thread's 8 MB ran out first on YouTube's Polymer, aborting the worker,
    // so the page runs on a thread whose stack fits the script limit. The
    // reservation is virtual; pages are committed only as they are touched.
    let page = std::thread::Builder::new()
        .name("chuzz-page".into())
        .stack_size(PAGE_STACK_BYTES)
        .spawn(move || serve::serve(&target).map_err(|error| error.to_string()))
        .expect("spawn the page thread");
    match page.join() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            eprintln!("chuzz-headless: {error}");
            std::process::exit(1);
        }
        Err(_) => std::process::exit(101),
    }
}

const PAGE_STACK_BYTES: usize = 256 * 1024 * 1024;
