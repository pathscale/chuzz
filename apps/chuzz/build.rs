use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use brotli::CompressorWriter;

const CSS_MARKER: &str = "__CHUZZ_EMBEDDED_CSS__";
const JS_URL: &str = "chuzz://ui/__chuzz__/app.js";

/// First line of a command's stdout, or `None` when it fails or prints nothing.
fn first_line(cmd: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(cmd).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let line = text.lines().next()?.trim().to_string();
    (!line.is_empty()).then_some(line)
}

/// Stamps the binary with which code it is and when it was compiled.
///
/// Ported from AgencyZero's `apps/gui/build.rs`, for the same reason it exists
/// there: `version` alone cannot answer "am I testing the fix?". A version is
/// bumped by hand and a stale bundle looks identical to a fresh one, which
/// already cost a round of testing here when a binary was swapped underneath a
/// running app and the old process kept serving the old code.
///
/// The commit says which code; a trailing `*` says the tree had uncommitted
/// edits on top of it; the timestamp says when, which is what gets compared
/// against "I just rebuilt".
fn stamp_build() {
    let sha =
        first_line("git", &["rev-parse", "--short=9", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let dirty = first_line("git", &["status", "--porcelain"]).is_some();
    println!(
        "cargo:rustc-env=CHUZZ_GIT_SHA={sha}{}",
        if dirty { "*" } else { "" }
    );

    // Local time on purpose: this string is read by a human comparing it to the
    // clock on the same machine that ran the build.
    let built = first_line("date", &["+%Y-%m-%d %H:%M:%S"]).unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=CHUZZ_BUILT_AT={built}");

    // A commit moves HEAD or a ref; a code edit touches `src`. Either has to
    // rerun this script, or the stamp would describe some earlier build.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs");
    println!("cargo:rerun-if-changed=src");
}

fn only_file_with_extension(directory: &Path, extension: &str) -> PathBuf {
    let mut matches = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", directory.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some(extension));
    let path = matches
        .next()
        .unwrap_or_else(|| panic!("no .{extension} asset in {}", directory.display()));
    assert!(
        matches.next().is_none(),
        "expected one .{extension} asset in {}",
        directory.display()
    );
    path
}

fn compress_asset(path: &Path, output: &Path, quality: u32) -> usize {
    let input =
        fs::read(path).unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let mut compressed = Vec::new();
    {
        let mut encoder = CompressorWriter::new(&mut compressed, 4096, quality, 22);
        encoder
            .write_all(&input)
            .unwrap_or_else(|error| panic!("cannot compress {}: {error}", path.display()));
    }
    fs::write(output, compressed).expect("write embedded Chuzz UI asset");
    input.len()
}

/// Compile and Brotli-embed the Solid browser chrome using the same asset
/// loading shape as AgencyZero's Blitz document factory.
fn build_frontend() {
    let manifest_dir = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo sets CARGO_MANIFEST_DIR"),
    );
    let frontend = manifest_dir.join("frontend");
    let output = Command::new("bun")
        .args(["run", "build"])
        .current_dir(&frontend)
        .output()
        .unwrap_or_else(|error| panic!("failed to start system bun: {error}"));
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail = stderr.lines().rev().take(30).collect::<Vec<_>>();
        panic!(
            "Solid frontend build failed:\n{}",
            tail.into_iter().rev().collect::<Vec<_>>().join("\n")
        );
    }

    let dist = manifest_dir.join("dist");
    let css_path = only_file_with_extension(&dist.join("static/css"), "css");
    let js_path = only_file_with_extension(&dist.join("static/js"), "js");
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
    let quality = if std::env::var("PROFILE").as_deref() == Ok("release") {
        9
    } else {
        2
    };
    let css_len = compress_asset(&css_path, &out_dir.join("embedded.css.br"), quality);
    let js_len = compress_asset(&js_path, &out_dir.join("embedded.js.br"), quality);
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta name=\"color-scheme\" content=\"dark light\"><title>Chuzz</title><style>{CSS_MARKER}</style></head><body><div id=\"root\"></div><script>globalThis.__CHUZZ_BLITZ__=true</script><script src=\"{JS_URL}\"></script></body></html>"
    );
    let generated = format!(
        "const CHUZZ_SHELL_HTML: &str = {html:?};\n\
         const CHUZZ_CSS_MARKER: &str = {CSS_MARKER:?};\n\
         const CHUZZ_JS_URL: &str = {JS_URL:?};\n\
         const CHUZZ_CSS_LEN: usize = {css_len};\n\
         const CHUZZ_JS_LEN: usize = {js_len};\n\
         const CHUZZ_CSS_BROTLI: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/embedded.css.br\"));\n\
         const CHUZZ_JS_BROTLI: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/embedded.js.br\"));\n"
    );
    fs::write(out_dir.join("chuzz_embedded.rs"), generated)
        .expect("write embedded Chuzz UI module");

    for path in [
        "frontend/src",
        "frontend/local-ui/src",
        "frontend/local-ui/package.json",
        "frontend/package.json",
        "frontend/bun.lock",
        "frontend/rsbuild.config.ts",
        "frontend/tsconfig.json",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
}

/// Drop framework load commands that nothing in this binary references.
///
/// This workspace takes `tauri` with `default-features = false`, so there is no
/// `wry` and no webview here at all. That is not enough. On macOS `tauri` and
/// `tauri-runtime` depend on `objc2-web-kit` unconditionally, because
/// `tauri-runtime`'s public API names WKWebView types, and that crate carries
/// `#[link(name = "WebKit", kind = "framework")]`. The directive travels in
/// rlib metadata rather than on the rustc command line, so it does not show up
/// in `cargo build -v` and it does not follow feature selection.
///
/// The result was a shipped binary declaring a dependency on a framework it
/// never calls, which dyld then loaded at every launch: `otool -L` on
/// `target/release/chuzz-gui` listed WebKit before this.
///
/// `-dead_strip_dylibs` drops load commands nothing references, so the decision
/// is made per link by the linker rather than guessed here.
fn strip_unused_frameworks() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    println!("cargo::rustc-link-arg-bins=-Wl,-dead_strip_dylibs");
}

fn main() {
    strip_unused_frameworks();
    stamp_build();
    build_frontend();
    tauri_build::build();
}
