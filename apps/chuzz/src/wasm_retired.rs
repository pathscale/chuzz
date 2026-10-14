//! Refusal contract for retired WebAssembly entry points.

use std::fmt;

#[derive(Debug)]
pub struct WasmUnsupported;

impl fmt::Display for WasmUnsupported {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("wasm-unsupported: WebAssembly support has been retired")
    }
}

impl std::error::Error for WasmUnsupported {}

/// Refuse before any entry point starts, including builds without capture.
pub fn reject_flags() {
    if std::env::args_os().skip(1).any(|argument| {
        let argument = argument.to_string_lossy();
        matches!(argument.as_ref(), "--wasm" | "--capture-wasm")
            || argument.starts_with("--wasm=")
            || argument.starts_with("--capture-wasm=")
    }) {
        eprintln!("chuzz: {WasmUnsupported}");
        std::process::exit(2);
    }
}
