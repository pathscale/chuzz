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

/// Give `document` a working `fetch` and `XMLHttpRequest`.
///
/// `deadline` bounds a single request. Nothing blocks on it, but a page that
/// waits forever for a server that never answers keeps the capture alive for
/// no reason, and a browser tab holds the connection.
pub fn install(
    document: &mut blitz_script::ScriptDocument,
    net: Arc<NetProvider>,
    deadline: Duration,
) {
    document.eval(NETWORK_API_SHIM);

    let mailbox = Mailbox::default();
    let handler_mailbox = mailbox.clone();

    document.set_ipc_handler(move |message| {
        let Ok(request) = serde_json::from_str::<NetRequest>(&message) else {
            // Not ours. The channel is shared, and a message this does not
            // recognise belongs to somebody else rather than being an error.
            return;
        };
        let Ok(url) = blitz_traits::net::Url::parse(&request.url) else {
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
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            mailbox.post(Delivery {
                id,
                payload: serde_json::json!({"ok": false, "error": "no runtime"}),
            });
            return;
        };
        handle.spawn(async move {
            let payload = match tokio::time::timeout(deadline, net.fetch_async(blitz_request)).await
            {
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
        let ready = mailbox.drain();
        if ready.is_empty() {
            return false;
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
  }

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

  if (typeof globalThis.fetch === "undefined") {
    globalThis.fetch = function (input, init) {
      var url = input && input.url ? input.url : input;
      return new Promise(function (resolve, reject) {
        send(url, init, function (result) {
          if (result.ok) { resolve(new Response(result)); }
          else { reject(new TypeError("fetch failed: " + (result.error || "unknown"))); }
        });
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
      this._method = "GET";
      this._url = "";
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
    XHR.prototype.abort = function () {};
    XHR.prototype.send = function (body) {
      var self = this;
      send(this._url, { method: this._method, body: body }, function (result) {
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

#[cfg(test)]
mod tests {
    use super::*;
    // `poll` arrives through the `Document` trait, not inherently.
    use blitz_dom::Document;
    use std::io::{Read, Write};

    /// Serve one request and hand back a body, or give up.
    ///
    /// Non-blocking with a deadline: if the bridge regresses and never issues
    /// the request, a plain `accept` would hang the test rather than fail it.
    fn serve_once(body: &'static str) -> (u16, std::thread::JoinHandle<bool>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("loopback is available");
        let port = listener
            .local_addr()
            .expect("the socket has an address")
            .port();
        listener
            .set_nonblocking(true)
            .expect("the listener takes a nonblocking mode");
        let handle = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(ref error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if std::time::Instant::now() >= deadline {
                            return false;
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => return false,
                }
            };
            stream
                .set_nonblocking(false)
                .expect("the accepted stream returns to blocking reads");
            let mut seen = Vec::new();
            let mut byte = [0u8; 1];
            while !seen.ends_with(b"\r\n\r\n") {
                match stream.read(&mut byte) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => seen.push(byte[0]),
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
            true
        });
        (port, handle)
    }

    /// Pump until the page has recorded an answer, or the passes run out.
    fn pump_for(document: &mut blitz_script::ScriptDocument, probe: &str) -> serde_json::Value {
        for _ in 0..200 {
            document.poll(None);
            if let Ok(value) = document.eval_json(probe)
                && !value.is_null()
            {
                return value;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        serde_json::Value::Null
    }

    fn page() -> blitz_script::ScriptDocument {
        blitz_script::ScriptDocument::from_html(
            "<html><body><div id=out></div></body></html>",
            blitz_dom::DocumentConfig::default(),
        )
    }

    /// A page that fetches gets its body, through the promise it parked.
    ///
    /// This is the whole point of the bridge: `blitz-script` has no
    /// host-function registration, so it proves a real network round trip can
    /// be assembled out of `postMessage`, `eval` and a poll hook.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_page_can_fetch_and_read_the_body() {
        let (port, server) = serve_once(r#"{"answer":42}"#);
        let mut document = page();
        install(
            &mut document,
            Arc::new(NetProvider::new(None)),
            Duration::from_secs(10),
        );

        document.eval(&format!(
            "globalThis.__result = null;
             fetch('http://127.0.0.1:{port}/data.json')
               .then(function (r) {{ return r.json(); }})
               .then(function (v) {{ globalThis.__result = v.answer; }})
               .catch(function (e) {{ globalThis.__result = 'error: ' + e; }});"
        ));

        let result = pump_for(&mut document, "globalThis.__result");
        assert!(
            server.join().expect("the server thread finishes"),
            "the bridge never opened a connection"
        );
        assert_eq!(
            result,
            serde_json::json!(42),
            "the page should read the body"
        );
    }

    /// The same channel carries `XMLHttpRequest`, which older code still uses.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_page_can_use_xmlhttprequest() {
        let (port, server) = serve_once("hello from the server");
        let mut document = page();
        install(
            &mut document,
            Arc::new(NetProvider::new(None)),
            Duration::from_secs(10),
        );

        document.eval(&format!(
            "globalThis.__xhr = null;
             var x = new XMLHttpRequest();
             x.open('GET', 'http://127.0.0.1:{port}/text');
             x.onload = function () {{ globalThis.__xhr = x.responseText; }};
             x.send();"
        ));

        let result = pump_for(&mut document, "globalThis.__xhr");
        assert!(
            server.join().expect("the server thread finishes"),
            "the bridge never opened a connection"
        );
        assert_eq!(result, serde_json::json!("hello from the server"));
    }
}
