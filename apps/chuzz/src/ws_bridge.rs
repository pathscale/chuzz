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
//! a command on `window.ipc.postMessage`, work happens on nagoya workers,
//! and events come back through a mailbox that a poll hook drains and hands to
//! the page by `eval`. `net_bridge` owns the IPC handler, because setting one
//! replaces it rather than adding to it, so it delegates here first and keeps
//! anything this does not claim.
//!
//! Unlike a fetch, a socket outlives its command: the send direction needs the
//! connection to stay addressable, which is what `senders` is.

use std::collections::{HashMap, VecDeque};
use std::future::poll_fn;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Poll, Waker};

use nago_wss::client::{ClientOptions, Connected};
use nago_wss::conn::{Connection, Error};
use nago_wss::proto::message::{CloseFrame, Limits, Message};
use nago_wss::stream::{ByteStream, Errno, StreamExt};
use nagoya::net::TcpStream;
use nagoya::reactor::Reactor;

#[path = "ws_bridge_script.rs"]
mod script;

// The default 16 MiB frame cap disconnects pages that send a self-contained
// image-rich JSON result in one frame. Keep a bounded cap, with room for the
// 38 MiB OpenCaptchaWorld image-matching response and similarly sized pages.
const MAX_MESSAGE_BYTES: usize = 128 << 20;
const MAX_FRAME_BYTES: usize = 64 << 20;
const MAX_ACTIVE_SOCKETS: usize = 64;

// Every document bridge shares this driver. The static owns the reactor,
// because dropping it while connections still hold handles stops their I/O.
static REACTOR: OnceLock<Result<Reactor, String>> = OnceLock::new();

#[derive(Default)]
struct OutboxState {
    messages: VecDeque<Message>,
    reader: Option<Waker>,
}

#[derive(Default)]
struct Outbox {
    state: Mutex<OutboxState>,
    interruptible: AtomicBool,
}

impl Outbox {
    fn push(&self, message: Message) {
        let reader = match self.state.lock() {
            Ok(mut state) => {
                state.messages.push_back(message);
                state.reader.take()
            }
            Err(_) => return,
        };
        if let Some(reader) = reader {
            reader.wake();
        }
    }

    fn pop(&self) -> Option<Message> {
        self.state
            .lock()
            .ok()
            .and_then(|mut state| state.messages.pop_front())
    }
}

/// Interrupt only a TCP read, before it consumes any bytes.
///
/// Dropping a pending WebSocket read is unsafe for TLS: that read may be
/// flushing a key update, with ciphertext held inside the write future.
/// Returning EINTR at the underlying TCP read instead lets both protocol
/// layers finish their futures and retain their state before servicing sends.
/// Handshake reads remain uninterrupted until the upgrade has completed.
struct CommandStream {
    stream: TcpStream,
    outbox: Arc<Outbox>,
}

impl ByteStream for CommandStream {
    async fn read(&mut self, buffer: &mut [u8]) -> Result<usize, Errno> {
        poll_fn(|context| {
            if self.outbox.interruptible.load(Ordering::Acquire) {
                let mut state = match self.outbox.state.lock() {
                    Ok(state) => state,
                    Err(_) => return Poll::Ready(Err(Errno(5))),
                };
                if !state.messages.is_empty() {
                    return Poll::Ready(Err(Errno::INTERRUPTED));
                }
                // Checking the queue and registering share one lock, so a
                // command cannot arrive between them and lose its wake.
                state.reader = Some(context.waker().clone());
            }
            self.stream.poll_read(context, buffer)
        })
        .await
    }

    async fn write_all(&mut self, buffer: &[u8]) -> Result<(), Errno> {
        self.stream.write_all(buffer).await
    }
}

enum Transport {
    Plain(CommandStream),
    Secure(nago_wss::tls::TlsStream<CommandStream>),
}

impl ByteStream for Transport {
    async fn read(&mut self, buffer: &mut [u8]) -> Result<usize, Errno> {
        match self {
            Self::Plain(stream) => stream.read(buffer).await,
            Self::Secure(stream) => stream.read(buffer).await,
        }
    }

    async fn write_all(&mut self, buffer: &[u8]) -> Result<(), Errno> {
        match self {
            Self::Plain(stream) => stream.write_all(buffer).await,
            Self::Secure(stream) => stream.write_all(buffer).await,
        }
    }
}

impl StreamExt for Transport {}

#[derive(Debug)]
struct Event {
    id: u64,
    reply: SocketReply,
}

/// Native operations accepted by the socket subsystem.
pub enum SocketRequest {
    Connect {
        id: u64,
        url: Option<String>,
        protocols: Vec<String>,
    },
    Send {
        id: u64,
        data: Option<String>,
    },
    Close {
        id: u64,
        code: Option<u16>,
    },
}

/// Native replies produced by the socket subsystem.
#[derive(Debug)]
pub enum SocketReply {
    Open { protocol: String },
    Message { data: String },
    Error { message: String },
    Close { code: u16, reason: String },
}

/// Errors returned by native socket operations.
#[derive(Debug)]
pub enum SocketError {
    MissingUrl,
    RegistryPoisoned,
    ActiveLimitReached,
    BadCloseCode,
}

impl std::fmt::Display for SocketError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingUrl => formatter.write_str("no url"),
            Self::RegistryPoisoned => formatter.write_str("socket registry poisoned"),
            Self::ActiveLimitReached => formatter.write_str("active socket limit reached"),
            Self::BadCloseCode => formatter.write_str("bad close code"),
        }
    }
}

/// Open sockets and the events they have produced.
#[derive(Clone, Default)]
pub struct Bridge {
    events: Arc<Mutex<Vec<Event>>>,
    senders: Arc<Mutex<HashMap<u64, Arc<Outbox>>>>,
    wake: crate::net_bridge::PageWake,
    lifetime: Option<Arc<blitz_dom::release::Teardown>>,
    signal: blitz_traits::net::AbortSignal,
    initiator: blitz_traits::net::Initiator,
}

impl Bridge {
    pub(crate) fn with_lifetime(
        wake: crate::net_bridge::PageWake,
        lifetime: Arc<blitz_dom::release::Teardown>,
    ) -> Self {
        Self {
            wake,
            lifetime: Some(lifetime),
            ..Default::default()
        }
    }

    pub(crate) fn with_initiator(mut self, initiator: blitz_traits::net::Initiator) -> Self {
        self.initiator = initiator;
        self
    }

    pub(crate) fn retire(&self) {
        blitz_traits::net::AbortController {
            signal: self.signal.clone(),
        }
        .abort();
        self.senders.lock().unwrap().clear();
        self.events.lock().unwrap().clear();
    }

    fn post(&self, id: u64, reply: SocketReply) {
        if self.signal.aborted() {
            return;
        }
        crate::serve::verbose(&format!("socket {id} produced {reply:?}"));
        // A poisoned lock means a panic on the document thread while holding
        // it. The page is already lost; dropping the event beats panicking on
        // top of the first one.
        if let Ok(mut queue) = self.events.lock() {
            queue.push(Event { id, reply });
        }
        self.wake.wake();
    }

    fn forget(&self, id: u64) {
        if let Ok(mut open) = self.senders.lock() {
            open.remove(&id);
        }
    }

    fn failed(&self, id: u64, _reason: &str, _error: impl std::fmt::Display) {
        self.post(
            id,
            SocketReply::Error {
                message: "WebSocket connection failed".to_owned(),
            },
        );
        self.post(
            id,
            SocketReply::Close {
                code: 1006,
                reason: String::new(),
            },
        );
    }

    fn reject(&self, id: u64, error: SocketError) {
        if matches!(error, SocketError::BadCloseCode) {
            self.post(
                id,
                SocketReply::Error {
                    message: "bad close code".to_owned(),
                },
            );
        } else {
            self.failed(id, "connect", error);
        }
    }

    /// Apply one native socket operation.
    pub fn operate(&self, request: SocketRequest) -> Result<(), SocketError> {
        match request {
            SocketRequest::Connect { id, url, protocols } => self.connect(id, url, protocols),
            SocketRequest::Send { id, data } => self.send(id, data),
            SocketRequest::Close { id, code } => self.close(id, code),
        }
    }

    /// Take one IPC message if it is a socket command. Returns whether it was.
    pub fn handle(&self, message: &str) -> bool {
        script::handle(self, message)
    }

    fn connect(
        &self,
        id: u64,
        url: Option<String>,
        protocols: Vec<String>,
    ) -> Result<(), SocketError> {
        if self.signal.aborted() {
            return Ok(());
        }

        let url = url.ok_or(SocketError::MissingUrl)?;
        let outbox = Arc::new(Outbox::default());
        match self.senders.lock() {
            Ok(mut open) => {
                if self.signal.aborted() || open.contains_key(&id) {
                    return Ok(());
                }
                if open.len() >= MAX_ACTIVE_SOCKETS {
                    return Err(SocketError::ActiveLimitReached);
                }
                open.insert(id, Arc::clone(&outbox));
            }
            Err(_) => return Err(SocketError::RegistryPoisoned),
        }

        let bridge = self.clone();
        nagoya::spawn(async move {
            let _lease = bridge.lifetime.as_ref().map(|receipt| receipt.lease());
            let mut cancelled = Box::pin(bridge.signal.cancelled());
            let mut work = Box::pin(async {
                let connected =
                    match open_socket(&url, &protocols, &bridge.initiator, Arc::clone(&outbox))
                        .await
                    {
                        Ok(connected) => connected,
                        Err(error) => {
                            bridge.failed(id, "connect", error);
                            return;
                        }
                    };

                // The subprotocol the server actually chose, which is not always
                // the first one offered and is what the page reads back off
                // `socket.protocol`.
                bridge.post(
                    id,
                    SocketReply::Open {
                        protocol: connected.protocol.unwrap_or_default(),
                    },
                );
                outbox.interruptible.store(true, Ordering::Release);
                bridge
                    .run_connection(id, connected.connection, &outbox)
                    .await;
            });
            std::future::poll_fn(|cx| {
                use std::future::Future;
                if cancelled.as_mut().poll(cx).is_ready() {
                    return std::task::Poll::Ready(());
                }
                work.as_mut().poll(cx)
            })
            .await;
            bridge.forget(id);
        });
        Ok(())
    }

    async fn run_connection(
        &self,
        id: u64,
        mut connection: Connection<Transport>,
        outbox: &Outbox,
    ) {
        loop {
            if let Some(message) = outbox.pop() {
                let closing = match &message {
                    Message::Close(Some(frame)) => Some(u16::from(frame.code)),
                    Message::Close(None) => Some(1005),
                    _ => None,
                };
                if let Err(error) = connection.write(message).await {
                    self.failed(id, "send", error);
                    break;
                }
                if let Some(code) = closing {
                    self.post(
                        id,
                        SocketReply::Close {
                            code,
                            reason: String::new(),
                        },
                    );
                    break;
                }
                continue;
            }

            match connection.read().await {
                Ok(Some(Message::Text(bytes))) => self.post(
                    id,
                    SocketReply::Message {
                        data: String::from_utf8_lossy(&bytes).into_owned(),
                    },
                ),
                // Binary arrives as text here on purpose. The pages
                // this exists for speak JSON; handing them a string
                // keeps `onmessage` usable, and `binaryType` is a
                // promise this does not yet keep.
                Ok(Some(Message::Binary(bytes))) => self.post(
                    id,
                    SocketReply::Message {
                        data: String::from_utf8_lossy(&bytes).into_owned(),
                    },
                ),
                Ok(Some(Message::Close(frame))) => {
                    let (code, reason) = match &frame {
                        Some(frame) => (
                            u16::from(frame.code),
                            String::from_utf8_lossy(&frame.reason).into_owned(),
                        ),
                        None => (1005, String::new()),
                    };
                    // nago-wss exposes control frames to its caller.
                    let _ = connection.close(frame).await;
                    self.post(id, SocketReply::Close { code, reason });
                    break;
                }
                Ok(Some(Message::Ping(payload))) => {
                    if let Err(error) = connection.pong(payload).await {
                        self.failed(id, "pong", error);
                        break;
                    }
                }
                Ok(Some(Message::Pong(_))) => {}
                // An outbound command woke the TCP read. No TLS write or
                // partially assembled WebSocket message was discarded.
                Err(Error::Io(error)) if error.interrupted() => continue,
                Err(error) => {
                    self.failed(id, "stream", error);
                    break;
                }
                Ok(None) => {
                    self.post(
                        id,
                        SocketReply::Close {
                            code: 1006,
                            reason: String::new(),
                        },
                    );
                    break;
                }
            }
        }
    }

    fn send(&self, id: u64, data: Option<String>) -> Result<(), SocketError> {
        let Some(data) = data else {
            return Ok(());
        };
        let sender = self
            .senders
            .lock()
            .ok()
            .and_then(|open| open.get(&id).cloned());
        // A send on a socket that has closed is dropped rather than reported.
        // That is what a browser does: `send` after close is a no-op, and the
        // close event has already told the page.
        if let Some(sender) = sender {
            sender.push(Message::Text(data.into_bytes().into()));
        }
        Ok(())
    }

    fn close(&self, id: u64, code: Option<u16>) -> Result<(), SocketError> {
        let sender = self
            .senders
            .lock()
            .ok()
            .and_then(|open| open.get(&id).cloned());
        if let Some(sender) = sender {
            let frame = CloseFrame {
                code: code.unwrap_or(1000).into(),
                reason: Default::default(),
            };
            if !frame.code.is_sendable() {
                return Err(SocketError::BadCloseCode);
            }
            sender.push(Message::Close(Some(frame)));
        }
        Ok(())
    }

    /// Hand every event from the last poll to the page.
    pub fn drain_into(&self, document: &mut blitz_script::ScriptDocument) -> bool {
        script::drain_into(self, document)
    }
}

/// A handshake carrying the subprotocols the page asked for.
///
/// This is the whole reason the fleet needs a real socket rather than a
/// reachable one: honey.id's auth rides here, as `["0login", "1<token>"]`, so
/// a client that drops the header authenticates as nobody.
fn invalid_handshake_bytes(value: &str) -> bool {
    value
        .bytes()
        .any(|byte| matches!(byte, b'\r' | b'\n' | b'\0'))
}

async fn open_socket(
    url: &str,
    protocols: &[String],
    initiator: &blitz_traits::net::Initiator,
    outbox: Arc<Outbox>,
) -> Result<Connected<Transport>, String> {
    if invalid_handshake_bytes(url) {
        return Err("invalid WebSocket URL".to_owned());
    }
    let url = url::Url::parse(url).map_err(|error| error.to_string())?;
    let secure = match url.scheme() {
        "ws" => false,
        "wss" => true,
        _ => return Err("scheme is not ws or wss".to_owned()),
    };
    if url.fragment().is_some() || !url.username().is_empty() || url.password().is_some() {
        return Err("WebSocket URL cannot contain credentials or a fragment".to_owned());
    }
    let host = match url.host() {
        Some(url::Host::Domain(host)) => host.to_owned(),
        Some(url::Host::Ipv4(host)) => host.to_string(),
        Some(url::Host::Ipv6(host)) => host.to_string(),
        None => return Err("missing host".to_owned()),
    };
    let document_url = initiator
        .document_url()
        .ok_or_else(|| "document URL is unavailable".to_owned())?;
    let origin = match document_url.scheme() {
        "http" | "https" => document_url.origin().ascii_serialization(),
        _ => "null".to_owned(),
    };
    if invalid_handshake_bytes(&origin) {
        return Err("invalid document origin".to_owned());
    }
    for (index, protocol) in protocols.iter().enumerate() {
        if invalid_handshake_bytes(protocol) {
            return Err("bad subprotocol".to_owned());
        }
        let token = !protocol.is_empty()
            && protocol.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(
                        byte,
                        b'!' | b'#'
                            | b'$'
                            | b'%'
                            | b'&'
                            | b'\''
                            | b'*'
                            | b'+'
                            | b'-'
                            | b'.'
                            | b'^'
                            | b'_'
                            | b'`'
                            | b'|'
                            | b'~'
                    )
            });
        if !token || protocols[..index].contains(protocol) {
            return Err("bad subprotocol".to_owned());
        }
    }
    let offered: Vec<&str> = protocols.iter().map(String::as_str).collect();

    let mut policy_url = url.clone();
    policy_url
        .set_scheme(if secure { "https" } else { "http" })
        .map_err(|_| "invalid WebSocket policy URL".to_owned())?;
    initiator.begin(&policy_url)?;
    initiator.check_url(&policy_url)?;

    let mut entropy = [0u8; 16];
    crate::tls::crypto_provider()
        .secure_random
        .fill(&mut entropy)
        .map_err(|_| "WebSocket handshake entropy unavailable".to_owned())?;
    let tls = if secure {
        Some(crate::tls::client_config()?)
    } else {
        None
    };
    let headers = [("Origin", origin.as_str())];
    let options = ClientOptions {
        protocols: &offered,
        headers: &headers,
        entropy,
        limits: Limits {
            max_frame: MAX_FRAME_BYTES as u64,
            max_message: MAX_MESSAGE_BYTES,
        },
        tls,
    };

    let reactor = REACTOR
        .get_or_init(|| Reactor::start().map_err(|error| error.to_string()))
        .as_ref()
        .map_err(Clone::clone)?;
    let handle = reactor.handle();
    let port = url.port().unwrap_or(if secure { 443 } else { 80 });
    // This function runs on a nagoya worker. getaddrinfo must never run
    // on the reactor thread or the document thread.
    let addresses = nagoya::resolve::resolve(&host, port).map_err(|error| format!("{error:?}"))?;
    for address in &addresses {
        let ip = match address {
            nagoya::reactor::Addr::V4(octets, _) => {
                std::net::IpAddr::V4(std::net::Ipv4Addr::from(*octets))
            }
            nagoya::reactor::Addr::V6(octets, _)
            | nagoya::reactor::Addr::V6Scoped(octets, _, _) => {
                std::net::IpAddr::V6(std::net::Ipv6Addr::from(*octets))
            }
            nagoya::reactor::Addr::Path(_) => {
                return Err("WebSocket resolved to Unix path".to_owned());
            }
        };
        initiator.check_address(ip)?;
    }
    let stream = nagoya::resolve::connect_any(&addresses, &handle)
        .await
        .map_err(|error| error.to_string())?;
    let stream = CommandStream { stream, outbox };

    // The URL-level client owns its TcpStream, so use nago-wss's upgrade
    // entry point to put the command wake boundary below TLS.
    let stream = if secure {
        let config = options
            .tls
            .clone()
            .ok_or_else(|| "missing TLS configuration".to_owned())?;
        let name = nago_wss::tls::rustls_pki_types::ServerName::try_from(host)
            .map_err(|error| error.to_string())?;
        let session =
            rustls::ClientConnection::new(config, name).map_err(|error| error.to_string())?;
        let mut stream = nago_wss::tls::TlsStream::client(stream, session);
        stream
            .handshake()
            .await
            .map_err(|error| error.to_string())?;
        Transport::Secure(stream)
    } else {
        Transport::Plain(stream)
    };
    let path = &url[url::Position::BeforePath..url::Position::AfterQuery];
    let host_header = &url[url::Position::BeforeHost..url::Position::AfterPort];
    if invalid_handshake_bytes(path) || invalid_handshake_bytes(host_header) {
        return Err("invalid WebSocket handshake target".to_owned());
    }
    let (connection, protocol) = nago_wss::upgrade::connect(
        stream,
        path,
        host_header,
        options.protocols,
        options.headers,
        options.entropy,
        options.limits,
    )
    .await
    .map_err(|error| error.to_string())?;
    if let Some(selected) = &protocol
        && !protocols.contains(selected)
    {
        return Err("server selected an unoffered subprotocol".to_owned());
    }
    Ok(Connected {
        connection,
        protocol,
    })
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
      // A listener may remove itself while handling the event (the RPC adapter
      // does this when sending queued requests on open). Iterating the live
      // array skips the next request after that removal. New listeners belong
      // to the next dispatch; explicitly removed listeners must not run.
      var registered = listeners[type].slice();
      for (var i = 0; i < registered.length; i++) {
        if (listeners[type].indexOf(registered[i]) >= 0) {
          registered[i].call(socket, event);
        }
      }
    }

    this.__deliver = function (event) {
      if (event.type === "open") {
        socket.readyState = 1;
        socket.protocol = event.protocol || "";
        fire("open", { type: "open", target: socket });
      } else if (event.type === "message") {
        var message = new MessageEvent("message", {
          data: event.data,
          origin: new URL(socket.url, location.href).origin
        });
        Object.defineProperties(message, {
          target: { value: socket, configurable: true },
          currentTarget: { value: socket, configurable: true },
          eventPhase: { value: 2, configurable: true }
        });
        try { fire("message", message); }
        finally {
          Object.defineProperties(message, {
            currentTarget: { value: null, configurable: true },
            eventPhase: { value: 0, configurable: true }
          });
        }
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
      if (socket.readyState === 0) {
        throw new DOMException("WebSocket is connecting", "InvalidStateError");
      }
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
    post({ ws: "connect", id: id, url: this.url, protocols: offered, origin: (typeof location !== "undefined" && location.origin) || "null" });
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
