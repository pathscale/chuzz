use std::io::Read;

use crate::browser::Browser;

include!(concat!(env!("OUT_DIR"), "/chuzz_embedded.rs"));

fn decompress_asset(compressed: &[u8], expected_len: usize, label: &str) -> Result<String, String> {
    let mut decoder = brotli::Decompressor::new(compressed, 4096);
    let mut decoded = String::with_capacity(expected_len);
    decoder
        .read_to_string(&mut decoded)
        .map_err(|error| format!("could not decompress embedded {label}: {error}"))?;
    Ok(decoded)
}

pub fn document(browser: Browser, url: &str) -> Result<crate::page_document::PageDocument, String> {
    document_with_prelude(browser, url, None)
}

fn document_with_prelude(
    browser: Browser,
    url: &str,
    prelude: Option<&str>,
) -> Result<crate::page_document::PageDocument, String> {
    let css = decompress_asset(CHUZZ_CSS_BROTLI, CHUZZ_CSS_LEN, "CSS")?;
    let javascript = decompress_asset(CHUZZ_JS_BROTLI, CHUZZ_JS_LEN, "JavaScript")?;
    let html = CHUZZ_SHELL_HTML
        .replacen(CHUZZ_CSS_MARKER, &css, 1)
        .replace(&format!("<script src=\"{CHUZZ_JS_URL}\"></script>"), "");
    let base = blitz_traits::net::Url::parse(url).map_err(|error| error.to_string())?;
    let net = browser.net();
    let navigation = net.browser_initiator(base.clone());
    let lifetime = blitz_dom::release::Teardown::new();
    let signal = blitz_traits::net::AbortSignal::default();
    let page_net = std::sync::Arc::new(crate::tab_resources::PageNet {
        net: std::sync::Arc::clone(&net),
        lifetime: std::sync::Arc::clone(&lifetime),
    });
    let config = blitz_dom::DocumentConfig {
        base_url: Some(url.to_owned()),
        net_provider: Some(page_net as _),
        teardown: Some(std::sync::Arc::clone(&lifetime)),
        abort_signal: Some(signal.clone()),
        html_parser_provider: Some(std::sync::Arc::new(blitz_html::HtmlProvider)),
        ..Default::default()
    };
    let host_config = crate::script_host::HostInstallContext {
        net: std::sync::Arc::new(net.for_document(base.clone(), &navigation)),
        base: Some(base),
        deadline: std::time::Duration::from_secs(15),
        wake: crate::net_bridge::PageWake::default(),
        lifetime,
        signal,
        session_backing: None,
    };
    let mut document = crate::page_document::PageDocument::from_html(
        std::sync::Arc::<str>::from(html),
        config,
        host_config,
    )?;
    browser.install_document_lifecycle(&mut document);
    document.eval("globalThis.__CHUZZ_BLITZ__ = true")?;
    if let Some(prelude) = prelude {
        document.eval(prelude)?;
    }
    document.eval_named(&javascript, CHUZZ_JS_URL)?;
    document.execute_scripts()?;
    Ok(document)
}

#[cfg(test)]
mod tests {
    use super::*;
    use blitz_dom::Document as _;
    use serde_json::json;

    const TEST_BRIDGE: &str = r#"
(function () {
  var tabs = [{ id: 0, title: 'Sample page', url: 'https://fixture.invalid/', status: 'complete', canGoBack: false, canGoForward: false }];
  var panel = { collapsed: true, sections: { page: true, history: true, network: false, console: false, debugging: true } };
  var status = { status: 'complete', url: 'https://fixture.invalid/', tabCount: 1, nodeCount: 0, transferred: '0 B' };
  globalThis.__IZUMO__ = {
    listen: function () { return Promise.resolve(function () {}); },
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

    #[test]
    fn embedded_ui_bundle_builds_the_shell_and_page_mount() {
        let mut document = document_with_prelude(
            Browser::new(None),
            "https://fixture.invalid/",
            Some(TEST_BRIDGE),
        )
        .expect("construct the UI with the registered page runtime");
        for _ in 0..64 {
            document.poll(None);
        }
        assert_eq!(
            document
                .eval_json("document.querySelectorAll('.app-shell').length")
                .expect("query the rendered shell"),
            json!(1)
        );
        assert_eq!(
            document
                .eval_json("document.querySelectorAll('[id$=\"page-0\"]').length")
                .expect("query the page mount"),
            json!(1)
        );
        assert!(
            crate::browser::page_node(&document.inner(), 0).is_some(),
            "the shell could not find its page mount"
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
        let mount = crate::browser::page_node(&inner, 0).expect("page mount");
        let mount_layout = inner.get_node(mount).unwrap().final_layout();
        assert!(
            mount_layout.size.width > 1000.0 && mount_layout.size.height > 700.0,
            "the page mount collapsed: {:?}",
            mount_layout.size
        );
    }
}
