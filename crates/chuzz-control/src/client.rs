//! Talking to a running browser over its control socket.
//!
//! The server side of this lives in `izumo`, which speaks MCP
//! JSON-RPC over endpoint-libs' length-delimited framing. Nothing in this repo
//! could speak it, which meant the one interface built for checking the window
//! without looking at it had no caller, so UI work was being reported on the
//! strength of log lines instead.
//!
//! Requests are built as JSON here rather than reusing this crate's own request
//! types, because `AttrScope` in [`crate`] is a chuzz-side extension the
//! running runtime does not have. Field names are camelCase, as protocol 2
//! defines them; version 1 hosts read only snake_case.

use std::io;
use std::path::{Path, PathBuf};

use blitz_control_protocol::{NagoyaStream, framed_json_neutral};
use endpoint_libs::libs::ws::transport::TransportStream;
use endpoint_libs::libs::ws::{MessageStream, WireMessage};
use nagoya::reactor::{Addr, Reactor, TcpStream};
use serde_json::{Value, json};

pub const AGENT_CONTROL_TOOL: &str = "blitz.agent.control";
/// The other half of the surface: DOM and layout snapshots, renderer metrics,
/// and the console and runtime-error streams. Advertised by `tools/list` on
/// any build that can collect them, and unreachable from here until now.
pub const DIAGNOSTICS_TOOL: &str = "blitz.diagnostics";

/// Where a running browser publishes its socket.
///
/// `TAURI_BLITZ_CONTROL_DESCRIPTOR` names one file; otherwise the runtime
/// writes into a per-instance file under the temp directory, and the most
/// recently written one is the browser that is still up.
pub fn descriptor_dir() -> PathBuf {
    match std::env::var_os("TAURI_BLITZ_CONTROL_DESCRIPTOR") {
        Some(path) => PathBuf::from(path)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(std::env::temp_dir),
        None => std::env::temp_dir().join("tauri-blitz-agent"),
    }
}

/// The newest descriptor, which is the browser most recently started.
///
/// Stale files outlive a killed process, so newest-wins is the rule rather
/// than first-found. A descriptor whose socket refuses the connection means
/// that browser is gone; say so with the path, because the caller then knows
/// to remove it.
pub fn newest_descriptor() -> io::Result<PathBuf> {
    let directory = descriptor_dir();
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(&directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let modified = entry.metadata()?.modified()?;
        if newest.as_ref().is_none_or(|(seen, _)| modified > *seen) {
            newest = Some((modified, path));
        }
    }
    newest.map(|(_, path)| path).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "no control descriptor in {}. Start the browser with CHUZZ_CONTROL=1, \
                 or turn inspection on in Settings -> Diagnostics.",
                directory.display()
            ),
        )
    })
}

pub struct Client {
    // Boxed rather than generic: `framed_json` returns an `impl Transport` that
    // cannot be named, so a `TransportStream<T>` field would force every caller
    // to be generic over a type it can never write down.
    stream: Box<dyn MessageStream>,
    next_id: i64,
    target: Option<Value>,
    identity: Option<Value>,
    explicit_identity: bool,
    notifications: std::collections::VecDeque<Value>,
    lost_notifications: u64,
}

/// The reactor every control connection lives on.
///
/// A nagoya socket makes progress only while the reactor it was created on is
/// polled, and this is a library: it does not own the caller's thread. One for
/// the process rather than one per connection, because a thread per connection
/// would be a thread per command.
fn client_reactor() -> &'static nagoya::reactor::Handle {
    static REACTOR: std::sync::OnceLock<(Reactor, nagoya::reactor::Handle)> =
        std::sync::OnceLock::new();
    let (_, handle) = REACTOR.get_or_init(|| {
        let reactor = Reactor::start().expect("control reactor could not start");
        let handle = reactor.handle();
        (reactor, handle)
    });
    handle
}

impl Client {
    /// Connect to the browser named by a descriptor file.
    pub async fn connect(descriptor: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let descriptor: Value = serde_json::from_slice(&std::fs::read(descriptor)?)?;
        let address = descriptor["address"]
            .as_str()
            .ok_or("the descriptor has no address")?;
        let path = address.strip_prefix("unix://").unwrap_or(address);
        let addr = Addr::path(path.as_bytes())
            .map_err(|_| std::io::Error::other("socket path is not a valid unix address"))?;
        let stream = TcpStream::connect(addr, client_reactor()).await?;
        Ok(Self {
            stream: Box::new(TransportStream::new(framed_json_neutral(
                NagoyaStream::new(stream),
            ))),
            next_id: 1,
            target: None,
            identity: None,
            explicit_identity: false,
            notifications: std::collections::VecDeque::new(),
            lost_notifications: 0,
        })
    }

    pub fn select_tab(&mut self, id: u64) {
        self.target = Some(json!({"tabId": id}));
        self.identity = None;
        self.explicit_identity = false;
    }

    pub fn select_chrome(&mut self) {
        self.target = Some(json!("chrome"));
        self.identity = None;
        self.explicit_identity = false;
    }

    pub fn set_expected_identity(&mut self, generation: u64, document_id: u64) {
        self.identity = Some(json!({"generation": generation, "documentId": document_id}));
        self.explicit_identity = true;
    }

    pub fn identity(&self) -> Option<&Value> {
        self.identity.as_ref()
    }

    pub async fn targets(&mut self) -> Result<Value, Box<dyn std::error::Error>> {
        self.call(json!({"command": "listTargets"})).await
    }

    fn queue_notification(&mut self, value: Value) {
        const CAPACITY: usize = 1024;
        if self.notifications.len() == CAPACITY {
            self.notifications.pop_front();
            self.lost_notifications += 1;
        }
        self.notifications.push_back(value);
    }

    pub async fn next_notification(&mut self) -> Result<Value, Box<dyn std::error::Error>> {
        if self.lost_notifications > 0 {
            return Ok(
                json!({"method": "notifications/blitz/diagnostics", "params": {
                    "event": "clientNotificationOverflow", "value": {"dropped": std::mem::take(&mut self.lost_notifications)}
                }}),
            );
        }
        if let Some(value) = self.notifications.pop_front() {
            return Ok(value);
        }
        loop {
            let message = self
                .stream
                .recv()
                .await
                .ok_or("the browser closed the socket")?
                .map_err(|error| format!("transport: {error}"))?;
            match message {
                WireMessage::Text(text) => {
                    let value: Value = serde_json::from_str(&text)?;
                    if value.get("method").is_some() && value.get("id").is_none() {
                        return Ok(value);
                    }
                }
                WireMessage::Close(_) => return Err("the browser closed the socket".into()),
                _ => {}
            }
        }
    }

    /// One `tools/call`, unwrapped to the browser's own response value.
    ///
    /// The MCP envelope puts the typed payload in `structuredContent`; the
    /// `content` array beside it is a one-line human summary and not the data.
    /// A caller that read `content` would get "semantic snapshot with 334
    /// nodes" and nothing to measure.
    pub async fn call(&mut self, request: Value) -> Result<Value, Box<dyn std::error::Error>> {
        self.call_tool(AGENT_CONTROL_TOOL, request).await
    }

    /// The same round trip against the diagnostics tool.
    pub async fn diagnostics(
        &mut self,
        request: Value,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        self.call_tool(DIAGNOSTICS_TOOL, request).await
    }

    async fn call_tool(
        &mut self,
        tool: &str,
        mut request: Value,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        let arguments = request
            .as_object_mut()
            .ok_or("tool arguments must be an object")?;
        if !arguments.contains_key("target")
            && arguments.get("allTargets") != Some(&Value::Bool(true))
            && let Some(target) = &self.target
        {
            arguments.insert("target".into(), target.clone());
        }
        let cached_guard = match arguments.get("command").and_then(Value::as_str) {
            Some("act") => true,
            Some("inspect") => arguments
                .get("params")
                .and_then(|params| params.get("root"))
                .and_then(Value::as_u64)
                .is_some(),
            Some("snapshot") => arguments
                .get("params")
                .and_then(|params| params.get("nodeIds"))
                .and_then(Value::as_array)
                .is_some_and(|nodes| !nodes.is_empty()),
            Some("capture") => arguments
                .get("params")
                .and_then(|params| params.get("nodeId"))
                .and_then(Value::as_u64)
                .is_some(),
            _ => false,
        };
        if arguments.get("target") == self.target.as_ref()
            && self.target.is_some()
            && (cached_guard || self.explicit_identity)
            && let Some(identity) = &self.identity
        {
            arguments
                .entry("expectedGeneration")
                .or_insert_with(|| identity["generation"].clone());
            arguments
                .entry("expectedDocumentId")
                .or_insert_with(|| identity["documentId"].clone());
        }
        let id = self.next_id;
        self.next_id += 1;
        let envelope = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {"name": tool, "arguments": request},
        });
        self.stream
            .send(WireMessage::Text(serde_json::to_string(&envelope)?.into()))
            .await
            .map_err(|error| format!("could not send: {error}"))?;

        loop {
            let message = self
                .stream
                .recv()
                .await
                .ok_or("the browser closed the socket")?
                .map_err(|error| format!("transport: {error}"))?;
            let text = match message {
                WireMessage::Text(text) => text,
                // Notifications and transport bookkeeping share the socket
                // with replies. Skipping them rather than failing is what lets
                // a client stay connected while the browser is streaming
                // events at it.
                WireMessage::Close(_) => return Err("the browser closed the socket".into()),
                _ => continue,
            };
            let reply: Value = serde_json::from_str(&text)?;
            if reply.get("id").and_then(Value::as_i64) != Some(id) {
                if reply.get("method").is_some() && reply.get("id").is_none() {
                    self.queue_notification(reply);
                }
                continue;
            }
            if let Some(context) = reply.pointer("/result/structuredContent/context") {
                let selected_id = self.target.as_ref().map_or_else(
                    || json!("chrome"),
                    |target| {
                        if target == "chrome" {
                            target.clone()
                        } else {
                            target["tabId"].clone()
                        }
                    },
                );
                if context.get("targetId") == Some(&selected_id) && !self.explicit_identity {
                    self.identity = Some(context.clone());
                }
            }
            if let Some(error) = reply.get("error") {
                return Err(format!("JSON-RPC error: {error}").into());
            }
            return reply
                .get("result")
                .and_then(|result| result.get("structuredContent"))
                .cloned()
                .ok_or_else(|| format!("no structuredContent in {reply}").into());
        }
    }

    /// The semantic tree, resolved and settled by the browser before it answers.
    pub async fn inspect(
        &mut self,
        root: Option<u64>,
        max_depth: u32,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        // `maxDepth` under protocol 2, with `max_depth` as the version 1 alias.
        // Getting this wrong is answered with a
        // JSON-RPC -32600 naming the missing field, which is at least a legible
        // way to be told.
        let response = self
            .call(json!({
                "command": "inspect",
                "params": {"root": root, "maxDepth": max_depth},
            }))
            .await?;
        if response["result"] == "error" {
            return Err(format!("inspect failed: {}", response["value"]).into());
        }
        let mut snapshot = response["value"].clone();
        if let Some(context) = response.get("context") {
            snapshot["context"] = context.clone();
        }
        Ok(snapshot)
    }

    /// Committed top-level TLS status from chuzz-headless, without collecting DOM nodes.
    pub async fn tls_status(&mut self) -> Result<Value, Box<dyn std::error::Error>> {
        let response = self
            .diagnostics(json!({
                "command": "snapshot",
                "params": {
                    "nodeIds": [],
                    "includeDom": false,
                    "includeLayout": false,
                    "includeComputedStyle": false
                }
            }))
            .await?;
        if response["result"] == "error" {
            return Err(format!("TLS status failed: {}", response["value"]).into());
        }
        Ok(response["value"]["dom"].clone())
    }

    /// Synthesise a click on a node.
    ///
    /// The runtime resolves layout, takes the node's centre and dispatches a
    /// real pointer move, down and up at those coordinates, so this goes
    /// through hit testing rather than calling a handler directly. A control
    /// covered by something else fails here exactly as it fails under a mouse,
    /// which is what makes it a test.
    pub async fn click(&mut self, node_id: u64) -> Result<Value, Box<dyn std::error::Error>> {
        self.call(json!({
            "command": "act",
            "params": {"action": "click", "params": {"nodeId": node_id}},
        }))
        .await
    }

    /// Replace a text input's contents.
    ///
    /// The runtime focuses the field, selects all and commits the text as an
    /// IME event. It does not submit: a field whose value has changed and a
    /// field whose value has been entered are different states, and the second
    /// one is [`Self::key`] with Enter.
    pub async fn set_value(
        &mut self,
        node_id: u64,
        value: &str,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        self.call(json!({
            "command": "act",
            "params": {
                "action": "setValue",
                "params": {"nodeId": node_id, "value": value},
            },
        }))
        .await
    }

    /// One key, down then up.
    pub async fn key(&mut self, key: &str, code: &str) -> Result<(), Box<dyn std::error::Error>> {
        let modifiers = json!({"shift": false, "control": false, "alt": false, "meta": false});
        for phase in ["down", "up"] {
            self.call(json!({
                "command": "act",
                "params": {
                    "action": "input",
                    "params": {
                        "input": "key",
                        "phase": phase,
                        "key": key,
                        "code": code,
                        "modifiers": modifiers,
                    },
                },
            }))
            .await?;
        }
        Ok(())
    }

    /// Type text one key at a time into whatever holds focus.
    ///
    /// Slower than [`Self::set_value`] and the only one that can be trusted.
    /// `set_value` asks the runtime to select-all before committing, and on
    /// this engine the select-all arrives as a literal `a` in the field: a
    /// URL typed that way becomes `ahttps://example.com`, which parses, fetches
    /// nothing and renders the browser's own error page. Two navigation
    /// "failures" were diagnosed from that before the field was read back.
    pub async fn type_text(&mut self, text: &str) -> Result<(), Box<dyn std::error::Error>> {
        for character in text.chars() {
            let key = character.to_string();
            let code = match character {
                'a'..='z' => format!("Key{}", character.to_ascii_uppercase()),
                'A'..='Z' => format!("Key{character}"),
                '0'..='9' => format!("Digit{character}"),
                '.' => "Period".to_owned(),
                '/' => "Slash".to_owned(),
                ':' => "Semicolon".to_owned(),
                '-' => "Minus".to_owned(),
                _ => "Unidentified".to_owned(),
            };
            self.key(&key, &code).await?;
        }
        Ok(())
    }

    /// A pointer press at window coordinates, hitting whatever is on top there.
    ///
    /// [`Self::click`] cannot answer "what is actually at this point": it
    /// starts from a node. When a control does not respond, the question is
    /// which element is receiving the press instead, and only a coordinate can
    /// ask that.
    pub async fn press(&mut self, x: f64, y: f64) -> Result<(), Box<dyn std::error::Error>> {
        let modifiers = json!({"shift": false, "control": false, "alt": false, "meta": false});
        for (phase, buttons) in [("move", 0), ("down", 1), ("up", 0)] {
            self.call(json!({
                "command": "act",
                "params": {
                    "action": "input",
                    "params": {
                        "input": "pointer",
                        "phase": phase,
                        "x": x,
                        "y": y,
                        "button": buttons,
                        "modifiers": modifiers,
                    },
                },
            }))
            .await?;
        }
        Ok(())
    }
}

/// A node's box as `[x, y, width, height]`, when it has one.
pub fn bounds(node: &Value) -> Option<[f64; 4]> {
    let values = node.get("bounds")?.as_array()?;
    let mut box_ = [0.0; 4];
    for (slot, value) in box_.iter_mut().zip(values) {
        *slot = value.as_f64()?;
    }
    Some(box_)
}

/// Whether two boxes intersect.
///
/// Touching edges are not an overlap: a control butted up against the next one
/// shares an edge coordinate and is laid out correctly.
pub fn overlaps(first: [f64; 4], second: [f64; 4]) -> bool {
    let [ax, ay, aw, ah] = first;
    let [bx, by, bw, bh] = second;
    ax < bx + bw && bx < ax + aw && ay < by + bh && by < ay + ah
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touching_boxes_are_not_overlapping() {
        // Bug 3 is decided by this predicate, so the boundary case has to be
        // the one a laid-out row actually produces: a title ending exactly
        // where the close button begins is correct, not a defect.
        assert!(!overlaps([0.0, 0.0, 10.0, 10.0], [10.0, 0.0, 10.0, 10.0]));
        assert!(overlaps([0.0, 0.0, 10.0, 10.0], [9.9, 0.0, 10.0, 10.0]));
        assert!(!overlaps([0.0, 0.0, 10.0, 10.0], [0.0, 10.0, 10.0, 10.0]));
    }

    #[test]
    fn bounds_survives_a_node_without_a_box() {
        assert_eq!(
            bounds(&json!({"bounds": [1.0, 2.0, 3.0, 4.0]})),
            Some([1.0, 2.0, 3.0, 4.0])
        );
        assert_eq!(bounds(&json!({"bounds": null})), None);
        assert_eq!(bounds(&json!({})), None);
    }
}
