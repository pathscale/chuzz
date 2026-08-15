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

    let engine = Engine::default();
    let module = Module::new(&engine, &bytes[..])?;
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
    if status != blitz_wasm::OK {
        return Err(format!(
            "`{entry}` reported {} ({status}): {:?}",
            blitz_wasm::status::name(status),
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
