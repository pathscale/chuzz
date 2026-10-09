//! `fetch` and `XMLHttpRequest`, built out of what the script engine already
//! exposes.
//!
//! Most of the web fetches its own content. A page that renders server-side is
//! the exception now, so an engine without these does not render a slightly
//! incomplete page: it renders the shell and stops. Measured over a
//! hundred-site corpus, `XMLHttpRequest` was missing on 6 sites and `fetch` on
//! 4 by name, and the pages laying out at 37% and 44% of a reference browser's
//! height are the same story counted a different way.
//!
//! `blitz-script` has no way to register a host function, so this is built from
//! the three things it does expose:
//!
//! - `window.ipc.postMessage(text)`, a string channel from JavaScript to here;
//! - `eval`, the same channel in reverse;
//! - `add_poll_hook`, work run on the document thread during polling.
//!
//! JavaScript posts a request and parks a promise. The handler here spawns the
//! real fetch on the page's own provider, so it obeys the same per-origin
//! connection cap as every other load. When it finishes, the result goes on a
//! queue, and the next poll hands it back by evaluating a call to the resolver
//! the shim installed. Nothing blocks: unlike a `<script src>`, which the HTML
//! spec makes synchronous, `fetch` is asynchronous by definition and this can
//! honour that.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use blitz_dom::Document as _;
use blitz_traits::net::{Method, Request};

use crate::decode::decode_body;
use crate::document_loader::NetProvider;

/// A finished request, waiting for the document thread to hand it back.
struct Delivery {
    id: u64,
    payload: serde_json::Value,
}

/// Requests in flight and results waiting to be delivered.
#[derive(Clone, Default)]
struct Mailbox(Arc<Mutex<Vec<Delivery>>>);

impl Mailbox {
    fn post(&self, delivery: Delivery) {
        // A poisoned lock here would mean a panic on the document thread while
        // holding it, and the page is already lost at that point; dropping the
        // delivery is better than a second panic on top of the first.
        if let Ok(mut queue) = self.0.lock() {
            queue.push(delivery);
        }
    }

    fn drain(&self) -> Vec<Delivery> {
        match self.0.lock() {
            Ok(mut queue) => std::mem::take(&mut *queue),
            Err(_) => Vec::new(),
        }
    }
}

/// Scroll operations posted by the JavaScript shim for the document thread.
#[derive(Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum ScrollMode {
    To,
    By,
}

#[derive(Clone, Copy, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ScrollCommand {
    mode: ScrollMode,
    #[serde(default)]
    left: Option<f64>,
    #[serde(default)]
    top: Option<f64>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ScrollEnvelope {
    #[serde(rename = "__chuzz_scroll")]
    command: ScrollCommand,
}

#[derive(Clone, Default)]
struct ScrollMailbox(Arc<Mutex<Vec<ScrollCommand>>>);

impl ScrollMailbox {
    fn post(&self, command: ScrollCommand) {
        if let Ok(mut queue) = self.0.lock() {
            queue.push(command);
        }
    }

    fn drain(&self) -> Vec<ScrollCommand> {
        match self.0.lock() {
            Ok(mut queue) => std::mem::take(&mut *queue),
            Err(_) => Vec::new(),
        }
    }
}

/// Apply one request to the native root scrollport, then expose that real
/// position and its resulting scroll events to JavaScript.
fn apply_viewport_scroll(
    document: &mut blitz_script::ScriptDocument,
    command: ScrollCommand,
) -> bool {
    let mut events = Vec::new();
    let changed = {
        let mut inner = document.inner_mut();
        let current = inner.viewport_scroll();
        let (left, top) = match command.mode {
            ScrollMode::To => (
                command.left.unwrap_or(current.x),
                command.top.unwrap_or(current.y),
            ),
            ScrollMode::By => (
                current.x + command.left.unwrap_or(0.0),
                current.y + command.top.unwrap_or(0.0),
            ),
        };
        // Blitz's scroll methods subtract their arguments from the native
        // offset, so current-minus-target expresses a target position.
        let dx = current.x - left;
        let dy = current.y - top;
        if let Some(root) = inner.try_root_element().map(|element| element.id) {
            inner.scroll_node_by_has_changed(root, dx, dy, |event| {
                events.push(event);
            })
        } else {
            inner.scroll_viewport_by_has_changed(dx, dy)
        }
    };

    if changed {
        refresh_script_scroll(document);
        for event in events {
            document.dispatch_dom_event(event);
        }
        // Scroll events on the root element are non-bubbling in Blitz. The
        // viewport's browser-facing event is targeted at Window, so dispatch
        // it separately after updating the getters from native state.
        document.eval(
            "if (typeof globalThis.dispatchEvent === 'function' && typeof globalThis.Event === 'function') globalThis.dispatchEvent(new Event('scroll'));",
        );
    }
    changed
}

fn refresh_script_scroll(document: &mut blitz_script::ScriptDocument) {
    let position = document.inner().viewport_scroll();
    let position = serde_json::json!({ "x": position.x, "y": position.y });
    document.eval(&format!(
        "if (globalThis.__chuzzScrollRefresh) globalThis.__chuzzScrollRefresh({position});"
    ));
}

/// What the shim posts when a page asks for a URL.
#[derive(serde::Deserialize)]
struct NetRequest {
    id: u64,
    url: String,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    body: Option<String>,
}

/// One `document.cookie = ...` assignment from the JavaScript shim.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CookieAssignment {
    cookie: String,
}

/// Give `document` a working `fetch` and `XMLHttpRequest`.
///
/// `deadline` bounds a single request. Nothing blocks on it, but a page that
/// waits forever for a server that never answers keeps the capture alive for
/// no reason, and a browser tab holds the connection.
pub fn install(
    document: &mut blitz_script::ScriptDocument,
    net: Arc<NetProvider>,
    deadline: Duration,
    base: Option<blitz_traits::net::Url>,
) {
    document.eval(NETWORK_API_SHIM);
    // After the web-API shim, so the real socket replaces the inert one that
    // shim installs when nothing better exists.
    document.eval(crate::ws_bridge::WEBSOCKET_SHIM);
    document.eval(crate::webgpu::WEBGPU_SHIM);

    let initial_cookie = base
        .as_ref()
        .map(|url| net.cookie_store().script_cookies(url))
        .unwrap_or_default();
    document.eval(&format!(
        "if (globalThis.__chuzzCookieRefresh) __chuzzCookieRefresh({});",
        serde_json::to_string(&initial_cookie).expect("a cookie header serializes as JSON")
    ));

    let mailbox = Mailbox::default();
    let handler_mailbox = mailbox.clone();
    let scroll_mailbox = ScrollMailbox::default();
    let handler_scroll_mailbox = scroll_mailbox.clone();
    let poll_scroll_mailbox = scroll_mailbox.clone();
    let sockets = crate::ws_bridge::Bridge::new();
    let handler_sockets = sockets.clone();
    let gpu = crate::webgpu::Bridge::default();
    let handler_gpu = gpu.clone();
    let handler_base = base.clone();
    let poll_base = base.clone();
    let poll_net = Arc::clone(&net);
    let mut last_cookie = initial_cookie;
    let initial_scroll = document.inner().viewport_scroll();
    let mut last_scroll = (initial_scroll.x, initial_scroll.y);

    document.set_ipc_handler(move |message| {
        // Setting an IPC handler replaces it rather than adding to it.
        // Each bridge claims its own messages through this shared handler.
        if handler_sockets.handle(&message) || handler_gpu.handle(&message) {
            return;
        }
        if let Ok(assignment) = serde_json::from_str::<CookieAssignment>(&message) {
            let Some(url) = handler_base.clone() else {
                return;
            };
            let _ = net
                .cookie_store()
                .set_script_cookie(&url, &assignment.cookie);
            return;
        }
        if let Ok(request) = serde_json::from_str::<ScrollEnvelope>(&message) {
            handler_scroll_mailbox.post(request.command);
            return;
        }
        let Ok(request) = serde_json::from_str::<NetRequest>(&message) else {
            // Not ours. The channel is shared, and a message this does not
            // recognise belongs to somebody else rather than being an error.
            return;
        };
        // Resolved against the page, not parsed on its own. `fetch("/v.json")`
        // is the ordinary way to ask for something on your own origin, and
        // parsing it without a base makes it a relative reference with no
        // scheme, which is a hard error. A page whose bootstrap asks that way
        // then fails at its first step and renders nothing.
        let Ok(url) = blitz_traits::net::Url::options()
            .base_url(handler_base.as_ref())
            .parse(&request.url)
        else {
            handler_mailbox.post(Delivery {
                id: request.id,
                payload: serde_json::json!({"ok": false, "error": "invalid URL"}),
            });
            return;
        };

        let method = match request.method.as_deref().map(str::to_ascii_uppercase) {
            Some(ref verb) if verb == "POST" => Method::POST,
            Some(ref verb) if verb == "PUT" => Method::PUT,
            Some(ref verb) if verb == "DELETE" => Method::DELETE,
            _ => Method::GET,
        };

        let mut blitz_request = Request::get(url);
        blitz_request.method = method;
        if let Some(body) = request.body {
            blitz_request.body = blitz_traits::net::Body::Bytes(body.into_bytes().into());
        }

        let net = Arc::clone(&net);
        let mailbox = handler_mailbox.clone();
        let id = request.id;

        // Spawned rather than blocked on: this runs on the document thread, and
        // holding it here would stop the very scripts waiting for the answer.
        let _ = nagoya::spawn(async move {
            let payload = match nagoya::timeout(deadline, net.fetch_async(blitz_request)).await {
                Ok(Ok((_, bytes))) => serde_json::json!({
                    "ok": true,
                    "status": 200,
                    "body": decode_body(&bytes),
                }),
                Ok(Err(error)) => serde_json::json!({"ok": false, "error": format!("{error:?}")}),
                Err(_) => serde_json::json!({"ok": false, "error": "timed out"}),
            };
            mailbox.post(Delivery { id, payload });
        });
    });

    document.add_poll_hook(move |document, _| {
        let current_cookie = poll_base
            .as_ref()
            .map(|url| poll_net.cookie_store().script_cookies(url))
            .unwrap_or_default();
        let cookie_changed = current_cookie != last_cookie;
        if cookie_changed {
            document.eval(&format!(
                "if (globalThis.__chuzzCookieRefresh) __chuzzCookieRefresh({});",
                serde_json::to_string(&current_cookie).expect("a cookie header serializes as JSON")
            ));
            last_cookie = current_cookie;
        }

        // Bridge events and fetch results share a pass, and any of them is
        // reason enough to draw a frame.
        let drew = sockets.drain_into(document) | gpu.drain_into(document);
        let mut scroll_changed = false;
        for command in poll_scroll_mailbox.drain() {
            if apply_viewport_scroll(document, command) {
                let position = document.inner().viewport_scroll();
                last_scroll = (position.x, position.y);
                scroll_changed = true;
            }
        }

        // Wheel, keyboard and automation actions can also move the native
        // viewport. Mirror those positions and notify Window listeners when
        // the movement did not originate in the shim queue above.
        let position = document.inner().viewport_scroll();
        if (position.x, position.y) != last_scroll {
            refresh_script_scroll(document);
            document.eval(
                "if (typeof globalThis.dispatchEvent === 'function' && typeof globalThis.Event === 'function') globalThis.dispatchEvent(new Event('scroll'));",
            );
            last_scroll = (position.x, position.y);
            scroll_changed = true;
        }
        let ready = mailbox.drain();
        if ready.is_empty() {
            return drew | cookie_changed | scroll_changed;
        }
        for delivery in ready {
            // `serde_json` renders a JavaScript-safe literal, so the body needs
            // no escaping of its own. Building this string by hand is how a
            // page's own quotes end up as a syntax error in the resolver.
            document.eval(&format!(
                "globalThis.__chuzzNetResolve({}, {});",
                delivery.id, delivery.payload
            ));
        }
        // The page has new data; whatever it renders from it needs a frame.
        true
    });
}

/// The JavaScript half: promises parked here, resolved from the host.
const NETWORK_API_SHIM: &str = r#"
(function () {
  var pending = Object.create(null);
  var nextId = 1;

  // Called by the host when a request finishes. Kept off the page's own names
  // so a site defining `fetch` helpers cannot collide with it.
  globalThis.__chuzzNetResolve = function (id, result) {
    var entry = pending[id];
    if (!entry) { return; }
    delete pending[id];
    try { entry(result); } catch (error) { /* the page's own handler threw */ }
  };

  function send(url, options, onResult) {
    var id = nextId++;
    pending[id] = onResult;
    try {
      window.ipc.postMessage(JSON.stringify({
        id: id,
        url: String(url),
        method: options && options.method ? String(options.method) : "GET",
        body: options && options.body != null ? String(options.body) : null
      }));
    } catch (error) {
      delete pending[id];
      onResult({ ok: false, error: String(error) });
    }
    return id;
  }

  // Stop listening for a request's answer.
  //
  // The request itself keeps running: the host has already spawned it and there
  // is no cancellation channel back. What this buys is the observable half of
  // aborting: the caller's promise settles now, and its handler does not run
  // later against a component that has been torn down.
  function forget(id) { delete pending[id]; }

  function Response(result) {
    var body = result.body == null ? "" : String(result.body);
    this.ok = !!result.ok;
    this.status = result.ok ? (result.status || 200) : 0;
    this.statusText = result.ok ? "OK" : String(result.error || "error");
    this.url = result.url || "";
    this.headers = {
      get: function () { return null; },
      has: function () { return false; },
      forEach: function () {}
    };
    this.text = function () { return Promise.resolve(body); };
    this.json = function () {
      try { return Promise.resolve(JSON.parse(body)); }
      catch (error) { return Promise.reject(error); }
    };
    this.clone = function () { return new Response(result); };
  }

  function abortError(signal) {
    var reason = signal && signal.reason;
    if (reason !== undefined && reason !== null) { return reason; }
    var error = new Error("The operation was aborted");
    error.name = "AbortError";
    return error;
  }

  if (typeof globalThis.fetch === "undefined") {
    globalThis.fetch = function (input, init) {
      var url = input && input.url ? input.url : input;
      var signal = init && init.signal;
      return new Promise(function (resolve, reject) {
        // An already-aborted signal rejects without touching the network, which
        // is what a caller reusing a controller across renders depends on.
        if (signal && signal.aborted) { reject(abortError(signal)); return; }
        var id = send(url, init, function (result) {
          if (result.ok) { resolve(new Response(result)); }
          else { reject(new TypeError("fetch failed: " + (result.error || "unknown"))); }
        });
        if (signal && typeof signal.addEventListener === "function") {
          signal.addEventListener("abort", function () {
            forget(id);
            reject(abortError(signal));
          });
        }
      });
    };
  }

  if (typeof globalThis.XMLHttpRequest === "undefined") {
    var XHR = function () {
      this.readyState = 0;
      this.status = 0;
      this.statusText = "";
      this.responseText = "";
      this.response = "";
      this.onreadystatechange = null;
      this.onload = null;
      this.onerror = null;
      this.onabort = null;
      this._method = "GET";
      this._url = "";
      // The id of the request in flight, so `abort` has something to drop.
      this._id = 0;
    };
    XHR.prototype.open = function (method, url) {
      this._method = method || "GET";
      this._url = url;
      this.readyState = 1;
      if (this.onreadystatechange) { this.onreadystatechange(); }
    };
    // Accepted and ignored: no header reaches the host yet, and throwing here
    // would break pages that only ever set an Accept.
    XHR.prototype.setRequestHeader = function () {};
    XHR.prototype.getAllResponseHeaders = function () { return ""; };
    XHR.prototype.getResponseHeader = function () { return null; };
    XHR.prototype.abort = function () {
      if (this._id) { forget(this._id); this._id = 0; }
      this.readyState = 0;
      this.status = 0;
      this.statusText = "";
      if (this.onabort) { this.onabort(); }
      if (this.onreadystatechange) { this.onreadystatechange(); }
    };
    XHR.prototype.send = function (body) {
      var self = this;
      this._id = send(this._url, { method: this._method, body: body }, function (result) {
        self._id = 0;
        self.readyState = 4;
        if (result.ok) {
          self.status = result.status || 200;
          self.statusText = "OK";
          self.responseText = result.body == null ? "" : String(result.body);
          self.response = self.responseText;
          if (self.onreadystatechange) { self.onreadystatechange(); }
          if (self.onload) { self.onload(); }
        } else {
          self.status = 0;
          self.statusText = String(result.error || "error");
          if (self.onreadystatechange) { self.onreadystatechange(); }
          if (self.onerror) { self.onerror(); }
        }
      });
    };
    globalThis.XMLHttpRequest = XHR;
  }
})();
"#;
