//! `WebSocket`, built on the same seam as `fetch`.
//!
//! The engine had a constructor that reported a refused connection without
//! ever opening one. That was deliberate and, for its original purpose, right:
//! a dev server's live-reload client runs at bundle top level, and a missing
//! constructor takes the whole bundle down, so an inert socket is the shape
//! those clients already survive.
//!
//! It stops being right the moment a page's own data rides on a socket. Eight
//! of the thirteen sites in this fleet authenticate over the WebSocket
//! handshake, passing the token in `Sec-WebSocket-Protocol`, so with an inert
//! socket no sign-in can complete and every check stops at what the page
//! decides for itself.
//!
//! The transport is the same one `net_bridge` uses, for the same reason:
//! `blitz-script` has no way to register a host function, so JavaScript posts
//! a command on `window.ipc.postMessage`, work happens on the tokio runtime,
//! and events come back through a mailbox that a poll hook drains and hands to
//! the page by `eval`. `net_bridge` owns the IPC handler, because setting one
//! replaces it rather than adding to it, so it delegates here first and keeps
//! anything this does not claim.
//!
//! Unlike a fetch, a socket outlives its command: the send direction needs the
//! connection to stay addressable, which is what `senders` is.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::Message;

/// One thing that happened to one socket, waiting for the document thread.
struct Event {
    id: u64,
    payload: serde_json::Value,
}

/// What JavaScript asked for.
#[derive(serde::Deserialize)]
struct Command {
    /// Present only on this bridge's messages. The IPC channel is shared, so
    /// this is what distinguishes a socket command from a fetch.
    ws: String,
    id: u64,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    protocols: Vec<String>,
    #[serde(default)]
    data: Option<String>,
    #[serde(default)]
    code: Option<u16>,
}

/// Open sockets and the events they have produced.
#[derive(Clone, Default)]
pub struct Bridge {
    events: Arc<Mutex<Vec<Event>>>,
    senders: Arc<Mutex<HashMap<u64, mpsc::UnboundedSender<Message>>>>,
}

impl Bridge {
    pub fn new() -> Self {
        Self::default()
    }

    fn post(&self, id: u64, payload: serde_json::Value) {
        // A poisoned lock means a panic on the document thread while holding
        // it. The page is already lost; dropping the event beats panicking on
        // top of the first one.
        if let Ok(mut queue) = self.events.lock() {
            queue.push(Event { id, payload });
        }
    }

    fn forget(&self, id: u64) {
        if let Ok(mut open) = self.senders.lock() {
            open.remove(&id);
        }
    }

    /// Take one IPC message if it is a socket command. Returns whether it was.
    pub fn handle(&self, message: &str) -> bool {
        let Ok(command) = serde_json::from_str::<Command>(message) else {
            return false;
        };
        match command.ws.as_str() {
            "connect" => self.connect(command),
            "send" => self.send(command),
            "close" => self.close(command),
            // A command shape we know, naming an operation we do not. Claimed
            // rather than passed on: it is addressed to this bridge.
            _ => {}
        }
        true
    }

    fn connect(&self, command: Command) {
        let id = command.id;
        let Some(url) = command.url else {
            self.post(
                id,
                serde_json::json!({"type": "error", "message": "no url"}),
            );
            return;
        };

        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            self.post(
                id,
                serde_json::json!({"type": "error", "message": "no runtime"}),
            );
            return;
        };

        let (tx, mut rx) = mpsc::unbounded_channel();
        if let Ok(mut open) = self.senders.lock() {
            open.insert(id, tx);
        }

        let bridge = self.clone();
        handle.spawn(async move {
            let request = match build_request(&url, &command.protocols) {
                Ok(request) => request,
                Err(why) => {
                    bridge.post(id, serde_json::json!({"type": "error", "message": why}));
                    bridge.post(id, closed(1006, "handshake"));
                    bridge.forget(id);
                    return;
                }
            };

            let (stream, response) = match tokio_tungstenite::connect_async(request).await {
                Ok(pair) => pair,
                Err(error) => {
                    // The page is told the same thing a browser tells it: the
                    // connection failed. The detail goes in the error event
                    // rather than being swallowed, because "it did not
                    // connect" is the least useful sentence in networking.
                    bridge.post(
                        id,
                        serde_json::json!({"type": "error", "message": format!("{error}")}),
                    );
                    bridge.post(id, closed(1006, "connect"));
                    bridge.forget(id);
                    return;
                }
            };

            // The subprotocol the server actually chose, which is not always
            // the first one offered and is what the page reads back off
            // `socket.protocol`.
            let negotiated = response
                .headers()
                .get("sec-websocket-protocol")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned();
            bridge.post(
                id,
                serde_json::json!({"type": "open", "protocol": negotiated}),
            );

            let (mut sink, mut source) = stream.split();
            loop {
                tokio::select! {
                    outgoing = rx.recv() => match outgoing {
                        Some(message) => {
                            let closing = matches!(message, Message::Close(_));
                            if sink.send(message).await.is_err() {
                                bridge.post(id, closed(1006, "send"));
                                break;
                            }
                            if closing {
                                bridge.post(id, closed(1000, ""));
                                break;
                            }
                        }
                        // Every sender dropped, so nothing can ask for more.
                        None => break,
                    },
                    incoming = source.next() => match incoming {
                        Some(Ok(Message::Text(text))) => bridge.post(
                            id,
                            serde_json::json!({"type": "message", "data": text.as_str()}),
                        ),
                        // Binary arrives as text here on purpose. The pages
                        // this exists for speak JSON; handing them a string
                        // keeps `onmessage` usable, and `binaryType` is a
                        // promise this does not yet keep.
                        Some(Ok(Message::Binary(bytes))) => bridge.post(
                            id,
                            serde_json::json!({
                                "type": "message",
                                "data": String::from_utf8_lossy(&bytes),
                            }),
                        ),
                        Some(Ok(Message::Close(frame))) => {
                            let (code, reason) = match frame {
                                Some(frame) => (u16::from(frame.code), frame.reason.to_string()),
                                None => (1005, String::new()),
                            };
                            bridge.post(id, closed(code, &reason));
                            break;
                        }
                        // Ping and pong are answered by the library.
                        Some(Ok(_)) => {}
                        Some(Err(error)) => {
                            bridge.post(
                                id,
                                serde_json::json!({"type": "error", "message": format!("{error}")}),
                            );
                            bridge.post(id, closed(1006, "stream"));
                            break;
                        }
                        None => {
                            bridge.post(id, closed(1006, "eof"));
                            break;
                        }
                    },
                }
            }
            bridge.forget(id);
        });
    }

    fn send(&self, command: Command) {
        let Some(data) = command.data else { return };
        let sender = self
            .senders
            .lock()
            .ok()
            .and_then(|open| open.get(&command.id).cloned());
        // A send on a socket that has closed is dropped rather than reported.
        // That is what a browser does: `send` after close is a no-op, and the
        // close event has already told the page.
        if let Some(sender) = sender {
            let _ = sender.send(Message::text(data));
        }
    }

    fn close(&self, command: Command) {
        let sender = self
            .senders
            .lock()
            .ok()
            .and_then(|open| open.get(&command.id).cloned());
        if let Some(sender) = sender {
            let frame = tokio_tungstenite::tungstenite::protocol::CloseFrame {
                code: command.code.unwrap_or(1000).into(),
                reason: Default::default(),
            };
            let _ = sender.send(Message::Close(Some(frame)));
        }
    }

    /// Hand every event since the last poll to the page.
    pub fn drain_into(&self, document: &mut blitz_script::ScriptDocument) -> bool {
        let ready = match self.events.lock() {
            Ok(mut queue) => std::mem::take(&mut *queue),
            Err(_) => Vec::new(),
        };
        if ready.is_empty() {
            return false;
        }
        for event in ready {
            // `serde_json` renders a JavaScript-safe literal, so a message
            // carrying quotes cannot become a syntax error in the receiver.
            document.eval(&format!(
                "globalThis.__chuzzWsEvent({}, {});",
                event.id, event.payload
            ));
        }
        true
    }
}

fn closed(code: u16, reason: &str) -> serde_json::Value {
    serde_json::json!({"type": "close", "code": code, "reason": reason})
}

/// A handshake request carrying the subprotocols the page asked for.
///
/// This is the whole reason the fleet needs a real socket rather than a
/// reachable one: honey.id's auth rides here, as `["0login", "1<token>"]`, so
/// a client that drops the header authenticates as nobody.
fn build_request(
    url: &str,
    protocols: &[String],
) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request, String> {
    let mut request = url
        .into_client_request()
        .map_err(|error| format!("{error}"))?;
    if !protocols.is_empty() {
        let value = protocols.join(", ");
        let header = value.parse().map_err(|_| "bad subprotocol".to_owned())?;
        request
            .headers_mut()
            .insert("sec-websocket-protocol", header);
    }
    Ok(request)
}

/// The JavaScript half: a `WebSocket` that talks to the bridge.
///
/// Installed after `WEB_API_SHIM`, so it replaces the inert constructor that
/// shim puts in when nothing better exists.
pub const WEBSOCKET_SHIM: &str = r#"
(function () {
  var sockets = Object.create(null);
  var nextId = 1;

  globalThis.__chuzzWsEvent = function (id, event) {
    var socket = sockets[id];
    if (!socket) { return; }
    socket.__deliver(event);
  };

  function post(payload) {
    try { window.ipc.postMessage(JSON.stringify(payload)); } catch (e) {}
  }

  function WebSocketShim(url, protocols) {
    var socket = this;
    var id = nextId++;
    sockets[id] = this;

    this.url = String(url);
    this.protocol = "";
    this.extensions = "";
    this.bufferedAmount = 0;
    this.binaryType = "blob";
    this.readyState = 0;
    this.onopen = null;
    this.onmessage = null;
    this.onerror = null;
    this.onclose = null;

    var listeners = { open: [], message: [], error: [], close: [] };
    this.addEventListener = function (type, handler) {
      if (listeners[type] && typeof handler === "function") { listeners[type].push(handler); }
    };
    this.removeEventListener = function (type, handler) {
      if (!listeners[type]) { return; }
      var i = listeners[type].indexOf(handler);
      if (i >= 0) { listeners[type].splice(i, 1); }
    };
    this.dispatchEvent = function () { return false; };

    function fire(type, event) {
      var direct = socket["on" + type];
      if (typeof direct === "function") { direct.call(socket, event); }
      var registered = listeners[type];
      for (var i = 0; i < registered.length; i++) { registered[i].call(socket, event); }
    }

    this.__deliver = function (event) {
      if (event.type === "open") {
        socket.readyState = 1;
        socket.protocol = event.protocol || "";
        fire("open", { type: "open", target: socket });
      } else if (event.type === "message") {
        fire("message", { type: "message", data: event.data, target: socket });
      } else if (event.type === "error") {
        fire("error", { type: "error", target: socket, message: event.message });
      } else if (event.type === "close") {
        socket.readyState = 3;
        delete sockets[id];
        fire("close", {
          type: "close",
          code: event.code,
          reason: event.reason || "",
          wasClean: event.code === 1000,
          target: socket,
        });
      }
    };

    this.send = function (data) {
      if (socket.readyState !== 1) { return; }
      post({ ws: "send", id: id, data: String(data) });
    };

    this.close = function (code) {
      if (socket.readyState === 3) { return; }
      socket.readyState = 2;
      post({ ws: "close", id: id, code: typeof code === "number" ? code : 1000 });
    };

    var offered = [];
    if (typeof protocols === "string") { offered = [protocols]; }
    else if (protocols && protocols.length) {
      for (var i = 0; i < protocols.length; i++) { offered.push(String(protocols[i])); }
    }
    post({ ws: "connect", id: id, url: this.url, protocols: offered });
  }

  WebSocketShim.CONNECTING = 0;
  WebSocketShim.OPEN = 1;
  WebSocketShim.CLOSING = 2;
  WebSocketShim.CLOSED = 3;
  WebSocketShim.prototype.CONNECTING = 0;
  WebSocketShim.prototype.OPEN = 1;
  WebSocketShim.prototype.CLOSING = 2;
  WebSocketShim.prototype.CLOSED = 3;

  globalThis.WebSocket = WebSocketShim;
})();
"#;
