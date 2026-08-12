use std::process::Command;

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

fn main() {
    stamp_build();
}
