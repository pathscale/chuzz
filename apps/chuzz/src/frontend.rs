use std::io::Read;

use crate::browser::Browser;

include!(concat!(env!("OUT_DIR"), "/chuzz_embedded.rs"));

struct EmbeddedChuzzScriptFetcher;

impl blitz_script::ScriptFetcher for EmbeddedChuzzScriptFetcher {
    fn fetch(&self, url: &blitz_traits::net::Url) -> Result<String, blitz_script::FetchError> {
        if url.as_str() == CHUZZ_JS_URL {
            decompress_asset(CHUZZ_JS_BROTLI, CHUZZ_JS_LEN, "JavaScript")
                .map_err(blitz_script::FetchError::InvalidData)
        } else {
            blitz_script::ScriptFetcher::fetch(&blitz_script::DefaultScriptFetcher, url)
        }
    }
}

fn decompress_asset(compressed: &[u8], expected_len: usize, label: &str) -> Result<String, String> {
    let mut decoder = brotli::Decompressor::new(compressed, 4096);
    let mut decoded = String::with_capacity(expected_len);
    decoder
        .read_to_string(&mut decoded)
        .map_err(|error| format!("could not decompress embedded {label}: {error}"))?;
    Ok(decoded)
}

const CHROME_API_SHIM: &str = r#"
(function () {
  if (typeof globalThis.ResizeObserver === 'undefined') {
    globalThis.ResizeObserver = function () {
      this.observe = function () {};
      this.unobserve = function () {};
      this.disconnect = function () {};
    };
  }
})();
"#;

pub fn document(browser: Browser, url: &str) -> Result<blitz_script::ScriptDocument, String> {
    let css = decompress_asset(CHUZZ_CSS_BROTLI, CHUZZ_CSS_LEN, "CSS")?;
    let html = CHUZZ_SHELL_HTML.replacen(CHUZZ_CSS_MARKER, &css, 1);
    let config = blitz_dom::DocumentConfig {
        base_url: Some(url.to_owned()),
        ..Default::default()
    };
    let mut document = blitz_script::ScriptDocument::from_html(&html, config)
        .with_fetcher(EmbeddedChuzzScriptFetcher);
    document.eval(crate::document_loader::WEB_API_SHIM);
    document.eval(CHROME_API_SHIM);
    browser.install_document_lifecycle(&mut document);
    Ok(document)
}

/// A stand-in for the Tauri command bridge, for tests that have no Tauri app.
///
/// The interface bootstraps itself from `list_tabs`/`active_tab_id`/
/// `panel_state`/`status`, so without a bridge the store keeps its empty
/// defaults and renders no tabs at all. A test running against that empty shell
/// cannot see the page mount, which is precisely the thing most worth testing:
/// the mount is the rendezvous point between this document and the page
/// document, and when its identifier drifts the browser silently renders
/// nothing. Canned answers here put one tab on screen so the rendezvous is
/// exercised for real.
#[cfg(test)]
pub(crate) const TAURI_TEST_BRIDGE: &str = r#"
(function () {
  var tabs = [{
    id: 0,
    title: 'Example Domain',
    url: 'https://example.com/',
    status: 'complete',
    canGoBack: false,
    canGoForward: false
  }];
  var panel = {
    collapsed: true,
    sections: { page: true, history: true, network: false, console: false, debugging: true }
  };
  var status = {
    status: 'complete',
    url: 'https://example.com/',
    tabCount: 1,
    nodeCount: 0,
    transferred: '0 B'
  };
  var nextCallback = 1;
  globalThis.__TAURI_INTERNALS__ = {
    transformCallback: function (callback) {
      var id = nextCallback++;
      globalThis['_' + id] = callback;
      return id;
    },
    unregisterCallback: function () {},
    convertFileSrc: function (path) { return path; },
    invoke: function (cmd) {
      switch (cmd) {
        case 'list_tabs': return Promise.resolve(tabs);
        case 'active_tab_id': return Promise.resolve(0);
        case 'panel_state': return Promise.resolve(panel);
        case 'status': return Promise.resolve(status);
        case 'debug_log': return Promise.resolve([]);
        default: return Promise.resolve(null);
      }
    }
  };
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use blitz_dom::Document as _;
    use serde_json::json;

    #[test]
    fn embedded_ui_bundle_builds_the_solid_shell_in_boa() {
        std::thread::Builder::new()
            .name("chuzz-ui-bundle-test".to_owned())
            .stack_size(16 * 1024 * 1024)
            .spawn(|| {
                let mut document = document(Browser::new(None), "chuzz://ui/").unwrap();
                document.set_ipc_handler(|_| {});
                // Before the bundle runs: the interface reads the bridge during
                // its own module evaluation.
                document.eval(TAURI_TEST_BRIDGE);
                document.execute_scripts();
                for _ in 0..64 {
                    document.poll(None);
                }
                assert_eq!(
                    document
                        .eval_json("document.querySelectorAll('.app-shell').length")
                        .expect("query the rendered UI"),
                    json!(1)
                );
                assert_eq!(
                    document
                        .eval_json("document.querySelectorAll('#chuzz-page-0').length")
                        .expect("query the page mount"),
                    json!(1)
                );
                // The assertion that matters: the shell's own lookup, not a CSS
                // query standing in for it. `page_node` is what decides whether
                // a fetched page is ever attached, and it is what regressed.
                assert!(
                    crate::browser::page_node(&document.inner(), 0).is_some(),
                    "the shell could not find the page mount for tab 0"
                );
                let mut inner = document.inner_mut();
                inner.set_viewport(blitz_traits::shell::Viewport::new(
                    1440,
                    960,
                    1.0,
                    blitz_traits::shell::ColorScheme::Dark,
                ));
                inner.resolve(0.0);
                let shell = inner.query_selector(".app-shell").unwrap().unwrap();
                let layout = inner.get_node(shell).unwrap().final_layout();
                assert!(layout.size.width > 1000.0);
                assert!(layout.size.height > 700.0);

                // The mount has to be a real box, not just a node in the tree.
                // It sits behind two wrappers that exist to keep the tab list
                // reactive, and a wrapper that does not carry the size through
                // collapses the page to nothing: the document would be attached
                // and correct, and the window would still look empty.
                let mount = crate::browser::page_node(&inner, 0).expect("page mount");
                let mount_layout = inner.get_node(mount).unwrap().final_layout();
                assert!(
                    mount_layout.size.width > 1000.0 && mount_layout.size.height > 700.0,
                    "the page mount collapsed: {:?}",
                    mount_layout.size
                );
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
