//! The shared parts of turning a URL into a renderable document.
//!
//! Tab loading itself lives in `browser.rs`, next to the state it mutates.
//! What stays here is what more than one caller needs: the net provider type,
//! the web-API shim Boa cannot supply for itself, and the headless capture
//! path, which has to load a page the same way a tab does without a window to
//! do it in.

// Everything but the shim and the provider alias belongs to `load_for_capture`,
// which is off by default: it pulls in the CPU rasteriser the windowed browser
// never needs.
#[cfg(feature = "capture")]
use std::sync::Arc;

#[cfg(feature = "capture")]
use blitz_dom::DocumentConfig;
#[cfg(feature = "capture")]
use blitz_html::HtmlProvider;
#[cfg(feature = "capture")]
use blitz_traits::net::Request;

#[cfg(feature = "capture")]
use crate::decode::decode_body;

#[cfg(all(feature = "capture", feature = "javascript"))]
use blitz_traits::net::Url;
#[cfg(all(feature = "capture", feature = "javascript"))]
use std::collections::HashMap;

pub type NetProvider = blitz_net::Provider;

/// How long a capture waits for a script the page asked for while running.
///
/// Longer than the window's, because nobody is watching a capture and a
/// dropped script costs the very fidelity the capture exists to measure.
#[cfg(all(feature = "capture", feature = "javascript"))]
const CAPTURE_SCRIPT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

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
pub(crate) const WEB_API_SHIM: &str = r#"
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
  // A dev server's live-reload client is the first script on the page, and it
  // runs at bundle top-level rather than behind an event. Anything it throws
  // takes the whole bundle down with it, so an app served by `rsbuild`, Vite or
  // `webpack-dev-server` renders as an empty mount point while the same app
  // built for production renders fine. That asymmetry is the symptom to
  // recognise: the page is not at fault, its reload client is.
  //
  // Both shims below are deliberately inert rather than functional. Live reload
  // needs a socket the engine does not have; what it must not do is prevent the
  // page from rendering once. A constructor that reports a failed connection is
  // the shape these clients already handle, because a dev server that has gone
  // away is an ordinary thing for them to survive.
  if (typeof globalThis.WebSocket === 'undefined') {
    globalThis.WebSocket = function (url, protocols) {
      var socket = this;
      this.url = String(url);
      this.protocol = '';
      this.extensions = '';
      this.bufferedAmount = 0;
      this.binaryType = 'blob';
      // CLOSED, not CONNECTING: a client that reads readyState synchronously
      // should see a socket that is already finished, not one it will wait on.
      this.readyState = 3;
      this.onopen = null;
      this.onmessage = null;
      this.onerror = null;
      this.onclose = null;
      this.send = function () {};
      this.close = function () {};
      this.addEventListener = function (type, handler) {
        if (type === 'error' || type === 'close') { listeners.push([type, handler]); }
      };
      this.removeEventListener = function () {};
      this.dispatchEvent = function () { return false; };
      var listeners = [];
      // Report the failure asynchronously, the way a real refused connection
      // arrives. Firing during construction would reach a handler the caller
      // has not attached yet.
      setTimeout(function () {
        var error = { type: 'error', target: socket };
        if (typeof socket.onerror === 'function') { socket.onerror(error); }
        var close = { type: 'close', target: socket, code: 1006, reason: '', wasClean: false };
        if (typeof socket.onclose === 'function') { socket.onclose(close); }
        for (var i = 0; i < listeners.length; i++) {
          listeners[i][1](listeners[i][0] === 'error' ? error : close);
        }
      }, 0);
    };
    globalThis.WebSocket.CONNECTING = 0;
    globalThis.WebSocket.OPEN = 1;
    globalThis.WebSocket.CLOSING = 2;
    globalThis.WebSocket.CLOSED = 3;
  }
  // `location.port` is absent rather than empty on a document the engine built,
  // and a reload client reads it to work out where to reconnect. Reading a
  // missing property is not itself fatal, but it puts `undefined` into a URL
  // the client then parses, so fill it in from the href. Defined only when
  // missing, so a real port keeps whatever the engine reported.
  if (typeof globalThis.location === 'object' && globalThis.location !== null
      && globalThis.location.port === undefined) {
    var located = String(globalThis.location.href || '').match(
      /^[a-zA-Z][a-zA-Z0-9+.-]*:\/\/[^\/?#:]*:(\d+)/
    );
    try {
      globalThis.location.port = located ? located[1] : '';
    } catch (e) {
      // A frozen location is fine to leave alone; the read above is what matters.
    }
  }
  // Boa supplies a `URL` constructor, so the fuller one above never installs.
  // What it leaves out is `searchParams`, and the omission is not survivable:
  // `url.searchParams.append(...)` is a property access on undefined, which is
  // a TypeError at bundle top-level rather than a missing query string. Attach
  // one to the prototype instead of replacing `URL`, so the engine's parsing
  // stays authoritative and only the gap is filled.
  if (typeof globalThis.URL === 'function' && globalThis.URL.prototype
      && !('searchParams' in globalThis.URL.prototype)) {
    Object.defineProperty(globalThis.URL.prototype, 'searchParams', {
      configurable: true,
      get: function () {
        // Rebuilt per read from the current search, because the engine's setters
        // may have moved it since. Mutating the returned object updates `search`
        // here; it does not write back through to `href`, which this cannot do
        // without reimplementing serialisation.
        var url = this;
        var params = new globalThis.URLSearchParams(String(url.search || ''));
        var write = function () {
          try { url.search = '?' + params.toString(); } catch (e) {}
        };
        var set = params.set;
        var append = params.append;
        params.set = function (key, value) { set.call(params, key, value); write(); };
        params.append = function (key, value) { append.call(params, key, value); write(); };
        return params;
      }
    });
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
/// Load a page outside the browser, for headless capture.
///
/// Shares the fetch, decompression, script execution and web-API shim with the
/// browser's own loading, so a capture shows what a tab would show. It is a
/// separate path rather than the same one because `browser.rs` loads into a
/// live window: it attaches the result to a `<web-view>` and emits events, and
/// there is no window here to attach to. **A capture therefore proves the
/// engine renders and does not prove the shell's mount rendezvous.**
#[cfg(feature = "capture")]
pub async fn load_for_capture(
    request: Request,
    net_provider: Arc<NetProvider>,
) -> Result<CapturedDocument, Box<dyn std::error::Error>> {
    use blitz_dom::Document as _;

    // `view-source:` is the browser's, and a capture that could not take it was
    // the one address a tab could show and a PNG could not. The scheme is not a
    // fetchable one, so this has to come before the net provider sees it: the
    // inner URL is what gets fetched, and the bytes are escaped rather than
    // parsed.
    //
    // `browser::source_html` rather than a second copy of the escaping. What
    // the capture writes has to be byte-for-byte what the tab shows, or the PNG
    // stops being evidence about the browser and becomes evidence about this
    // function.
    //
    // Nothing below applies to a source document: it has no scripts to run and
    // no images to wait for, so it returns here rather than falling through to
    // the script pump.
    if request.url.scheme() == "view-source" {
        let inner = request.url.path().to_owned();
        let url = Url::parse(&inner).map_err(|error| format!("{inner} is not a URL: {error}"))?;
        let (_, bytes) = net_provider
            .fetch_async(Request::get(url))
            .await
            .map_err(|error| format!("could not fetch {inner}: {error:?}"))?;
        let html = crate::browser::source_html(&decode_body(&bytes));
        return Ok(CapturedDocument::Html(Box::new(
            blitz_html::HtmlDocument::from_html(
                &html,
                DocumentConfig {
                    html_parser_provider: Some(Arc::new(HtmlProvider)),
                    ..Default::default()
                },
            )
            .into_inner(),
        )));
    }

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
        let mut document = document.with_fetcher(crate::script_fetch::PageScripts::new(
            scripts,
            Arc::clone(&net_provider),
            CAPTURE_SCRIPT_DEADLINE,
        ));
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
