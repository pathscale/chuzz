//! Fetching a URL and turning the bytes into a renderable page document.
//!
//! Each tab owns one loader. A loader holds at most one in-flight request: a
//! new navigation aborts the previous one, so a slow page cannot land after
//! the user has already moved on.

use std::sync::{Arc, Mutex};

use blitz_dom::{DocumentConfig, FontContext};
use blitz_html::{HtmlDocument, HtmlProvider};
use blitz_traits::net::{AbortController, AbortSignal, Request};
use blitz_traits::shell::ShellProvider;
use dioxus_native::{SubDocumentAttr, prelude::*};

use crate::decode::decode_body;
use crate::history::{History, SyncStore, TabNavProvider};

#[cfg(feature = "javascript")]
use blitz_traits::net::Url;
#[cfg(feature = "javascript")]
use std::collections::HashMap;

pub type NetProvider = blitz_net::Provider;

/// Shown when a page cannot be fetched at all (DNS failure, refused
/// connection, TLS error). The message node is filled in from the real error.
const ERROR_HTML: &str = r#"<!doctype html>
<html>
  <head>
    <meta charset="utf-8">
    <title>Page not available</title>
    <style>
      body { font: 16px/1.5 system-ui, sans-serif; margin: 0; padding: 12vh 10vw; color: #202124; background: #fff; }
      h1 { font-size: 22px; font-weight: 600; margin: 0 0 12px; }
      p { margin: 0 0 8px; color: #5f6368; }
      #error { font-family: ui-monospace, monospace; font-size: 13px; color: #b3261e; word-break: break-word; }
    </style>
  </head>
  <body>
    <h1>This page could not be loaded</h1>
    <p>Chuzz could not reach the site.</p>
    <p id="error"></p>
  </body>
</html>
"#;

/// Shown when the server answered but sent nothing back.
const EMPTY_HTML: &str = r#"<!doctype html>
<html>
  <head><meta charset="utf-8"><title>Empty response</title></head>
  <body style="font: 16px/1.5 system-ui, sans-serif; margin: 0; padding: 12vh 10vw; color: #5f6368;">
    <h1 style="font-size: 22px; color: #202124;">Empty response</h1>
    <p>The server returned no content for this address.</p>
  </body>
</html>
"#;

/// Web APIs the script engine does not provide.
///
/// Boa is a JavaScript engine, not a browser: it supplies the language, and
/// everything `window`-shaped has to come from the embedder. A bundle that
/// touches `localStorage` on startup otherwise dies before it renders, which
/// looks exactly like a blank page.
///
/// This is in-memory and per-document on purpose. Real persistence is browser
/// policy: it needs an origin-keyed store on disk and a quota, and pretending
/// otherwise would silently lose a site's data on reload.
#[cfg(feature = "javascript")]
const WEB_API_SHIM: &str = r#"
(function () {
  function MemoryStorage() {
    var entries = Object.create(null);
    return {
      getItem: function (key) {
        var value = entries[String(key)];
        return value === undefined ? null : value;
      },
      setItem: function (key, value) { entries[String(key)] = String(value); },
      removeItem: function (key) { delete entries[String(key)]; },
      clear: function () { entries = Object.create(null); },
      key: function (index) {
        var names = Object.keys(entries);
        return index < names.length ? names[index] : null;
      },
      get length() { return Object.keys(entries).length; }
    };
  }
  if (typeof globalThis.localStorage === 'undefined') {
    globalThis.localStorage = MemoryStorage();
  }
  if (typeof globalThis.sessionStorage === 'undefined') {
    globalThis.sessionStorage = MemoryStorage();
  }
  if (typeof globalThis.URL === 'undefined') {
    // Enough of the URL interface for routing: parse, read the parts, and
    // resolve against a base. Not a WHATWG-conformant implementation.
    globalThis.URL = function (input, base) {
      var text = String(input);
      if (base !== undefined && !/^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(text)) {
        var root = String(base);
        if (text.charAt(0) === '/') {
          var origin = root.match(/^([a-zA-Z][a-zA-Z0-9+.-]*:\/\/[^\/?#]*)/);
          text = (origin ? origin[1] : root.replace(/[?#].*$/, '')) + text;
        } else {
          text = root.replace(/[?#].*$/, '').replace(/[^\/]*$/, '') + text;
        }
      }
      var parts = text.match(
        /^([a-zA-Z][a-zA-Z0-9+.-]*:)\/\/([^\/?#:]*)(?::(\d+))?([^?#]*)(\?[^#]*)?(#.*)?$/
      );
      this.href = text;
      this.protocol = parts ? parts[1] : '';
      this.hostname = parts ? parts[2] : '';
      this.port = parts && parts[3] ? parts[3] : '';
      this.pathname = parts && parts[4] ? parts[4] : '/';
      this.search = parts && parts[5] ? parts[5] : '';
      this.hash = parts && parts[6] ? parts[6] : '';
      this.host = this.hostname + (this.port ? ':' + this.port : '');
      this.origin = this.protocol + '//' + this.host;
      this.toString = function () { return this.href; };
    };
    // Object URLs: a page that creates one and revokes it on teardown would
    // otherwise throw on a missing static. Blobs are not backed here, so the
    // handle is a token rather than a readable resource.
    var objectUrls = Object.create(null);
    var objectUrlSeq = 0;
    globalThis.URL.createObjectURL = function (object) {
      var handle = 'blob:chuzz/' + (++objectUrlSeq);
      objectUrls[handle] = object;
      return handle;
    };
    globalThis.URL.revokeObjectURL = function (handle) {
      delete objectUrls[String(handle)];
    };
  }
  if (typeof globalThis.URLSearchParams === 'undefined') {
    globalThis.URLSearchParams = function (init) {
      var pairs = [];
      if (typeof init === 'string') {
        init.replace(/^\?/, '').split('&').forEach(function (part) {
          if (!part) return;
          var index = part.indexOf('=');
          var key = index < 0 ? part : part.slice(0, index);
          var value = index < 0 ? '' : part.slice(index + 1);
          pairs.push([decodeURIComponent(key), decodeURIComponent(value)]);
        });
      }
      this.get = function (key) {
        for (var i = 0; i < pairs.length; i++) {
          if (pairs[i][0] === key) return pairs[i][1];
        }
        return null;
      };
      this.has = function (key) { return this.get(key) !== null; };
      this.set = function (key, value) { pairs.push([key, String(value)]); };
      this.append = function (key, value) { pairs.push([key, String(value)]); };
      this.toString = function () {
        return pairs
          .map(function (pair) {
            return encodeURIComponent(pair[0]) + '=' + encodeURIComponent(pair[1]);
          })
          .join('&');
      };
    };
  }
  if (typeof globalThis.MutationObserver === 'undefined') {
    // Frameworks construct an observer at startup and only rely on callbacks
    // later. A constructor that records its target and never fires keeps that
    // startup path alive; it does not make mutations observable.
    globalThis.MutationObserver = function (callback) {
      this.callback = callback;
      this.observe = function () {};
      this.disconnect = function () {};
      this.takeRecords = function () { return []; };
    };
  }
  if (typeof globalThis.IntersectionObserver === 'undefined') {
    // Reports everything as visible, once, instead of never reporting at all.
    //
    // A no-op observer is the obvious shim and the wrong one: the common use is
    // a lazy loader that shows an image or a section when it scrolls into view,
    // and an observer that never fires leaves that content permanently hidden.
    // Answering "yes, visible" once is wrong for anything below the fold but
    // renders the page; answering nothing renders a skeleton.
    globalThis.IntersectionObserver = function (callback) {
      var self = this;
      this.root = null;
      this.rootMargin = '0px';
      this.thresholds = [0];
      this.observe = function (target) {
        setTimeout(function () {
          callback([{
            target: target,
            isIntersecting: true,
            intersectionRatio: 1,
            time: 0,
            boundingClientRect: null,
            intersectionRect: null,
            rootBounds: null
          }], self);
        }, 0);
      };
      this.unobserve = function () {};
      this.disconnect = function () {};
      this.takeRecords = function () { return []; };
    };
  }
  if (typeof globalThis.screen === 'undefined') {
    // Read from the window rather than invented, so a page branching on screen
    // size gets an answer consistent with the one it gets from window.innerWidth.
    globalThis.screen = {
      get width() { return globalThis.innerWidth || 1440; },
      get height() { return globalThis.innerHeight || 960; },
      get availWidth() { return globalThis.innerWidth || 1440; },
      get availHeight() { return globalThis.innerHeight || 960; },
      colorDepth: 24,
      pixelDepth: 24,
      orientation: { type: 'landscape-primary', angle: 0 }
    };
  }
  if (typeof globalThis.requestIdleCallback === 'undefined') {
    globalThis.requestIdleCallback = function (callback) {
      return setTimeout(function () {
        callback({ didTimeout: false, timeRemaining: function () { return 0; } });
      }, 1);
    };
    globalThis.cancelIdleCallback = function (handle) { clearTimeout(handle); };
  }
  if (typeof globalThis.matchMedia === 'undefined') {
    globalThis.matchMedia = function (query) {
      return {
        media: String(query),
        matches: false,
        addListener: function () {},
        removeListener: function () {},
        addEventListener: function () {},
        removeEventListener: function () {},
        dispatchEvent: function () { return false; }
      };
    };
  }
})();
"#;

/// A new tab: an empty document, not a failed load.
const BLANK_HTML: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title></title></head>
<body style="margin:0;background:#0f1622"></body></html>
"#;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LoadStatus {
    Loading,
    Idle,
}

/// A page that finished loading and is ready to be swapped into its tab.
#[derive(Clone)]
pub struct LoadedDocument {
    pub document: SubDocumentAttr,
    pub title: String,
}

pub struct DocumentLoader {
    pub font_ctx: FontContext,
    pub net_provider: Arc<NetProvider>,
    pub status: Signal<LoadStatus>,
    pub history: SyncStore<History>,
    /// Bumped by `reload`. The tab's load resource reads it, so reloading the
    /// same URL still re-runs the fetch.
    pub reload_generation: Signal<u64>,
    current_abort: Mutex<Option<AbortController>>,
}

impl DocumentLoader {
    pub fn new(net_provider: Arc<NetProvider>, history: SyncStore<History>) -> Self {
        Self {
            font_ctx: FontContext::default(),
            net_provider,
            status: Signal::new(LoadStatus::Idle),
            history,
            reload_generation: Signal::new(0),
            current_abort: Mutex::new(None),
        }
    }

    pub fn reload(&self) {
        self.abort_current();
        let mut generation = self.reload_generation;
        *generation.write() += 1;
    }

    pub fn reload_generation(&self) -> u64 {
        *self.reload_generation.read()
    }

    pub fn abort_current(&self) {
        // A poisoned lock here would mean a panic while swapping abort
        // handles; dropping the stale handle is still the right recovery.
        if let Ok(mut slot) = self.current_abort.lock()
            && let Some(controller) = slot.take()
        {
            controller.abort();
        }
    }

    fn install_abort(&self) -> AbortSignal {
        let controller = AbortController::default();
        let signal = controller.signal.clone();
        if let Ok(mut slot) = self.current_abort.lock() {
            if let Some(previous) = slot.take() {
                previous.abort();
            }
            *slot = Some(controller);
        }
        signal
    }

    fn doc_config(&self, base_url: Option<String>, signal: AbortSignal) -> DocumentConfig {
        DocumentConfig {
            base_url,
            net_provider: Some(Arc::clone(&self.net_provider) as _),
            navigation_provider: Some(Arc::new(TabNavProvider {
                history: self.history,
            })),
            shell_provider: Some(consume_context::<Arc<dyn ShellProvider>>()),
            html_parser_provider: Some(Arc::new(HtmlProvider)),
            font_ctx: Some(self.font_ctx.clone()),
            abort_signal: Some(signal),
            ..Default::default()
        }
    }

    pub async fn load(&self, req: Request) -> LoadedDocument {
        if req.url.scheme() == "about" {
            let signal = self.install_abort();
            let config = self.doc_config(None, signal);
            let document = HtmlDocument::from_html(BLANK_HTML, config).into_inner();
            return LoadedDocument {
                document: SubDocumentAttr::new(document),
                title: String::new(),
            };
        }

        let signal = self.install_abort();
        let response = self
            .net_provider
            .fetch_async(req.signal(signal.clone()))
            .await;

        match response {
            Ok((resolved_url, bytes)) => {
                let html = if bytes.is_empty() {
                    EMPTY_HTML.to_string()
                } else {
                    decode_body(&bytes)
                };
                let config = self.doc_config(Some(resolved_url), signal.clone());
                self.build_page(&html, config, &signal).await
            }
            Err(error) => {
                let config = self.doc_config(None, signal);
                let mut document = HtmlDocument::from_html(ERROR_HTML, config).into_inner();
                if let Some(text_node) = document
                    .get_element_by_id("error")
                    .and_then(|id| document.get_node(id))
                    .and_then(|node| node.children.first().copied())
                {
                    document
                        .mutate()
                        .set_node_text(text_node, &format!("{error:?}"));
                }
                LoadedDocument {
                    document: SubDocumentAttr::new(document),
                    title: String::from("Page not available"),
                }
            }
        }
    }
}

impl DocumentLoader {
    /// Parse a page. Without JavaScript the parsed DOM is the final DOM.
    #[cfg(not(feature = "javascript"))]
    async fn build_page(
        &self,
        html: &str,
        config: DocumentConfig,
        _signal: &AbortSignal,
    ) -> LoadedDocument {
        let document = HtmlDocument::from_html(html, config).into_inner();
        let title = document
            .find_title_node()
            .map(|node| node.text_content())
            .unwrap_or_default();
        LoadedDocument {
            document: SubDocumentAttr::new(document),
            title,
        }
    }

    /// Parse a page and run its scripts.
    ///
    /// Most of the web ships an empty body and builds the DOM in JavaScript, so
    /// without this step such a page paints nothing at all. The script fetcher
    /// is synchronous, so external sources are prefetched through the browser's
    /// own net provider (keeping cookies and caching) and then served from
    /// memory.
    #[cfg(feature = "javascript")]
    async fn build_page(
        &self,
        html: &str,
        config: DocumentConfig,
        signal: &AbortSignal,
    ) -> LoadedDocument {
        use blitz_dom::Document as _;

        let document = blitz_script::ScriptDocument::from_html(html, config);

        let mut scripts: HashMap<Url, String> = HashMap::new();
        for url in document.external_script_urls() {
            if scripts.contains_key(&url) {
                continue;
            }
            let request = Request::get(url.clone()).signal(signal.clone());
            if let Ok((_, bytes)) = self.net_provider.fetch_async(request).await {
                scripts.insert(url, decode_body(&bytes));
            }
        }

        let mut document = document.with_fetcher(PrefetchedScripts { scripts });
        // Installed before the page's own scripts, which read these at startup.
        document.eval(WEB_API_SHIM);
        document.execute_scripts();

        // Scripts keep working after the first pass: timers, microtasks and
        // fetches all land later. Drive the document until it settles, or the
        // page swaps in as the empty mount point the server actually sent.
        for _ in 0..24 {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            document.eval("void 0");
            document.poll(None);
            // Settle once the page has built something worth painting.
            //
            // Checking `body > *` was wrong: a script-rendered page ships an
            // empty mount point, so that matches on the first iteration and
            // the loop exits before any script has run. A framework mounts
            // *into* that element, so the test is whether the tree has grown
            // below it.
            let built = document
                .inner()
                .query_selector("body > * > *")
                .ok()
                .flatten()
                .is_some();
            if built {
                break;
            }
        }

        // Resolve style and layout before handing the document over.
        // AgencyZero's blitz-preview does the same after its poll loop: without
        // it the tree is built but unmeasured, so anything whose size comes
        // from layout (an inline SVG sized `w-auto` from its viewBox, a flex
        // child) is swapped in at the wrong size or not painted at all.
        document.inner_mut().resolve(0.0);

        let title = {
            let inner = document.inner();
            inner
                .find_title_node()
                .map(|node| node.text_content())
                .unwrap_or_default()
        };

        LoadedDocument {
            document: SubDocumentAttr::new(document),
            title,
        }
    }
}

/// Serves the sources prefetched above, falling back to the default fetcher
/// for `file:` and `data:` URLs.
#[cfg(feature = "javascript")]
struct PrefetchedScripts {
    scripts: HashMap<Url, String>,
}

#[cfg(feature = "javascript")]
impl blitz_script::ScriptFetcher for PrefetchedScripts {
    fn fetch(&self, url: &Url) -> Result<String, blitz_script::FetchError> {
        if let Some(source) = self.scripts.get(url) {
            return Ok(source.clone());
        }
        blitz_script::DefaultScriptFetcher.fetch(url)
    }
}

/// Load a page outside the browser, for headless capture.
///
/// Shares the fetch, decompression, script execution and web-API shims with
/// the browser's own loader, so a capture shows what a tab would show. It
/// cannot reuse `DocumentLoader` itself, which is built around Dioxus signals
/// that only exist inside a running UI.
#[cfg(feature = "capture")]
pub async fn load_for_capture(
    request: Request,
    net_provider: Arc<NetProvider>,
) -> Result<CapturedDocument, Box<dyn std::error::Error>> {
    use blitz_dom::Document as _;

    let (resolved_url, bytes) = net_provider
        .fetch_async(request)
        .await
        .map_err(|error| format!("{error:?}"))?;
    let html = decode_body(&bytes);

    let config = DocumentConfig {
        base_url: Some(resolved_url),
        net_provider: Some(Arc::clone(&net_provider) as _),
        html_parser_provider: Some(Arc::new(HtmlProvider)),
        ..Default::default()
    };

    #[cfg(feature = "javascript")]
    {
        let document = blitz_script::ScriptDocument::from_html(&html, config);
        let mut scripts: HashMap<Url, String> = HashMap::new();
        for url in document.external_script_urls() {
            if scripts.contains_key(&url) {
                continue;
            }
            if let Ok((_, bytes)) = net_provider.fetch_async(Request::get(url.clone())).await {
                scripts.insert(url, decode_body(&bytes));
            }
        }
        let mut document = document.with_fetcher(PrefetchedScripts { scripts });
        document.eval(WEB_API_SHIM);
        document.execute_scripts();
        // Pump the script runtime until the page has built its DOM, then keep
        // pumping for a few more passes.
        //
        // Breaking out the moment `body > * > *` matches is wrong: parsing the
        // HTML already issued the <img> fetches, and those responses are
        // delivered on the document's channel. Returning early drops the
        // document that owns the receiver while requests are still in flight,
        // so `respond` sends into a closed channel and the decoded image is
        // discarded, which looks exactly like an image that failed to load.
        //
        // Note the selector also matches static markup that simply has a
        // wrapper element, so for most pages this loop used to exit on the
        // first pass regardless of whether anything was pending.
        let mut passes_since_built = 0;
        for _ in 0..24 {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            document.eval("void 0");
            document.poll(None);
            let built = document
                .inner()
                .query_selector("body > * > *")
                .ok()
                .flatten()
                .is_some();
            if built {
                passes_since_built += 1;
                if passes_since_built >= 8 {
                    break;
                }
            }
        }
        Ok(CapturedDocument::Script(Box::new(document)))
    }

    #[cfg(not(feature = "javascript"))]
    Ok(CapturedDocument::Html(
        HtmlDocument::from_html(&html, config).into_inner(),
    ))
}

/// A loaded page held for capture. Which variant it is depends on whether the
/// build runs scripts; both expose the same document underneath.
#[cfg(feature = "capture")]
pub enum CapturedDocument {
    // Boxed: a ScriptDocument is far larger than a bare one, and the enum
    // would otherwise cost the bigger variant on every use.
    #[cfg(feature = "javascript")]
    Script(Box<blitz_script::ScriptDocument>),
    #[allow(dead_code)]
    Html(Box<blitz_dom::BaseDocument>),
}

#[cfg(feature = "capture")]
impl CapturedDocument {
    pub fn with_document<R>(
        &mut self,
        callback: impl FnOnce(&mut blitz_dom::BaseDocument) -> R,
    ) -> R {
        #[cfg(feature = "javascript")]
        use blitz_dom::Document as _;
        match self {
            #[cfg(feature = "javascript")]
            Self::Script(document) => callback(&mut document.inner_mut()),
            Self::Html(document) => callback(document),
        }
    }
}

impl Drop for DocumentLoader {
    fn drop(&mut self) {
        self.abort_current();
    }
}
