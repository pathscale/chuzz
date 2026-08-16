//! Building a document out of a WebAssembly guest instead of out of HTML.
//!
//! There is no URL here, nothing is fetched, no HTML is parsed and no
//! JavaScript runs. The tree exists only because the guest called
//! `create_element` and `append_child` across the `blitz-wasm` ABI.
//!
//! Two callers share this: the headless capture (`--capture-wasm`) and the
//! window (`--wasm`). They differ only in the `DocumentConfig` they hand in and
//! in what they do with the document afterwards, which is the whole point of
//! keeping the guest-running part here. A guest-built page that rendered in the
//! window but not in a capture, or the other way round, would make the two
//! useless for explaining each other.

use std::path::Path;

use blitz_dom::{BaseDocument, DocumentConfig, NodeId, qual_name};
use blitz_wasm::Host;
use wasmi::{Engine, Linker, Module, Store};

/// The entry point a guest module is expected to export.
///
/// `run`, to match `blitz-wasm`'s ABI.md. Overridable because the name is a
/// convention rather than part of the ABI: nothing in `blitz-wasm` knows it.
pub fn entry_export() -> String {
    std::env::var("CHUZZ_WASM_ENTRY").unwrap_or_else(|_| "run".to_owned())
}

/// An empty `<html><body>` document, and the body, which is what a guest
/// mounts on.
///
/// The seed is not a convenience. Every operation in the ABI either creates a
/// detached node or needs one that already exists, so without a node to start
/// from a guest can build a whole tree and have nowhere to put it.
pub fn empty_document(config: DocumentConfig) -> (BaseDocument, NodeId) {
    let mut document = BaseDocument::new(config);
    let root_id = document.root_node().id;

    let mut changes = document.mutate();
    let html = changes.create_element(qual_name!("html"), vec![]);
    let body = changes.create_element(qual_name!("body"), vec![]);
    changes.append_children(html, &[body]);
    changes.append_children(root_id, &[html]);
    drop(changes);

    (document, body)
}

/// Instantiate `module_path` with the host bound to `document`, call the entry
/// export, and give the document back with whatever the guest built in it.
///
/// Layout is deliberately *not* resolved here. The capture resolves once and
/// paints; the window hands the document to the shell, which owns sizing and
/// decides when to lay out. Doing it here would be a wasted pass for one caller
/// and the wrong viewport for the other.
pub fn run_guest(
    module_path: &Path,
    document: BaseDocument,
    mount: NodeId,
) -> Result<BaseDocument, Box<dyn std::error::Error>> {
    let bytes = std::fs::read(module_path)
        .map_err(|error| format!("could not read {}: {error}", module_path.display()))?;
    run_guest_bytes(&bytes, document, mount)
}

/// The same, for a module that is already in memory.
///
/// Split out because a module fetched from a page never touches the disk, and
/// two copies of the instantiate sequence would drift. Everything that decides
/// whether a guest ran correctly lives here and nowhere else.
pub fn run_guest_bytes(
    bytes: &[u8],
    document: BaseDocument,
    mount: NodeId,
) -> Result<BaseDocument, Box<dyn std::error::Error>> {
    let engine = Engine::default();
    let module = Module::new(&engine, bytes)?;
    let mut store = Store::new(&engine, Host::new(document, mount));
    let mut linker = <Linker<Host>>::new(&engine);
    blitz_wasm::add_to_linker(&mut linker)?;
    // `instantiate_and_start`, not `instantiate`: the start section runs the
    // module's static initialisers, and a guest whose statics fail to set up
    // would otherwise build a partial tree and report success.
    let instance = linker.instantiate_and_start(&mut store, &module)?;

    let entry = entry_export();
    let status = instance
        .get_typed_func::<(), i32>(&store, &entry)
        .map_err(|error| format!("the module does not export `{entry}`: {error}"))?
        .call(&mut store, ())?;
    // `Status` is a newtype over the raw `i32` the guest returns, and the ABI
    // moved to it from a bare constant and a free function. Wrapping here keeps
    // the comparison and the name reading from the same source.
    let status = blitz_wasm::Status(status);
    if status != blitz_wasm::Status::OK {
        return Err(format!(
            "`{entry}` reported {} ({}): {:?}",
            status.name(),
            status.0,
            store.data().counters().last_dom_error
        )
        .into());
    }
    // A guest can return OK having built nothing at all, and the result of that
    // is a blank page with no error anywhere to explain it. Say so here, where
    // the reason is still known.
    if !store.data().mutated() {
        eprintln!(
            "chuzz: warning: `{entry}` returned OK without mutating the document, \
             so the page will be empty"
        );
    }

    Ok(store.into_data().into_document())
}

/// chuzz's own guest, assembled from `fixtures/panel.wat`, written to a `.wasm`
/// on disk so a test goes in through the same door the CLI does.
///
/// Deliberately not `blitz-wasm`'s demo guest. That one is a fixture for *its*
/// tests and is free to change into whatever proves the binding next; borrowing
/// it would make these tests fail for reasons that have nothing to do with
/// chuzz. This one is pinned to the ABI instead: change a signature in
/// `add_to_linker` and it stops instantiating.
///
/// `CHUZZ_TEST_WASM` swaps in another module, which is how a real guest can be
/// pointed at these paths by hand.
#[cfg(test)]
pub(crate) fn fixture_module() -> std::path::PathBuf {
    use std::sync::OnceLock;
    static MODULE: OnceLock<std::path::PathBuf> = OnceLock::new();
    MODULE
        .get_or_init(|| {
            if let Some(path) = std::env::var_os("CHUZZ_TEST_WASM") {
                return std::path::PathBuf::from(path);
            }
            let wasm = wat::parse_str(include_str!("../fixtures/panel.wat"))
                .expect("fixtures/panel.wat should assemble");
            let dir = std::env::temp_dir().join("chuzz-wasm-tests");
            std::fs::create_dir_all(&dir).expect("a scratch directory");
            let path = dir.join("panel.wasm");
            std::fs::write(&path, wasm).expect("the fixture module should be writable");
            path
        })
        .clone()
}

/// A `<script type="application/wasm">` a page declares.
///
/// The tag names a module and where to put it:
///
/// ```html
/// <script type="application/wasm" src="/demo.wasm" mount="#root"></script>
/// ```
///
/// `runtime="..."` is read and ignored. The design has it selecting a
/// preloaded runtime by version, nothing implements preloaded runtimes yet, and
/// failing on an attribute whose meaning has not been decided would make the
/// page harder to write rather than safer.
#[derive(Debug, Clone)]
pub struct WasmScript {
    pub src: String,
    pub mount: String,
}

/// The first wasm script tag in a parsed document, if it has one.
///
/// Returns `None` for the overwhelming majority of pages, which is the point:
/// a document without the tag must take no new code path at all.
pub fn find_wasm_script(document: &BaseDocument) -> Option<WasmScript> {
    let node = document
        .query_selector(r#"script[type="application/wasm"]"#)
        .ok()
        .flatten()?;
    let element = document.get_node(node)?.element_data()?;
    let attr = |name: &str| {
        element
            .attrs
            .iter()
            .find(|attr| attr.name.local.as_ref() == name)
            .map(|attr| attr.value.to_string())
    };
    Some(WasmScript {
        src: attr("src")?,
        // No `mount` means the guest has nowhere to build, which is a page
        // authoring error rather than something to guess at.
        mount: attr("mount")?,
    })
}

/// Whether fetched bytes are plausibly a WebAssembly module.
///
/// Worth doing separately from `Module::new`, because the failure this catches
/// is not a corrupt module. A CDN that serves its index page under `200` for
/// every unknown path hands back `<!doctype html>`, and wasmi then reports a
/// parse error describing byte offsets in a file that is not a module at all.
/// Whoever is debugging needs to be told the server answered with a page.
pub fn validate_module(bytes: &[u8]) -> Result<(), String> {
    const MAGIC: &[u8] = b"\0asm";
    if bytes.starts_with(MAGIC) {
        return Ok(());
    }
    let head = &bytes[..bytes.len().min(64)];
    let looks_like_html = head
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .is_some_and(|start| head[start] == b'<');
    if looks_like_html {
        return Err(
            "the server returned HTML, not a module. A CDN that answers 200 with its \
             index page for unknown paths does this; check the module is actually \
             deployed at that path"
                .to_owned(),
        );
    }
    Err(format!(
        "not a WebAssembly module: expected the magic bytes \\0asm, found {:02x?}",
        &bytes[..bytes.len().min(4)]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three answers have to be distinguishable, because they send whoever
    /// is debugging to three different places: the deploy, the server config,
    /// and the module itself.
    #[test]
    fn validation_tells_html_apart_from_a_corrupt_module() {
        assert!(validate_module(b"\0asm\x01\0\0\0").is_ok());

        let html = validate_module(b"<!doctype html><html>").unwrap_err();
        assert!(html.contains("HTML"), "{html}");

        let leading_space = validate_module(b"\n  <html>").unwrap_err();
        assert!(
            leading_space.contains("HTML"),
            "whitespace before the tag still means a page: {leading_space}"
        );

        let corrupt = validate_module(&[0xde, 0xad, 0xbe, 0xef]).unwrap_err();
        assert!(
            !corrupt.contains("HTML") && corrupt.contains("magic"),
            "a corrupt module must not be reported as HTML: {corrupt}"
        );

        // Shorter than the magic bytes, and the slicing must not panic.
        assert!(validate_module(b"\0as").is_err());
        assert!(validate_module(b"").is_err());
    }
}
