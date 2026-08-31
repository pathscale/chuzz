//! Talking to a running browser over its control socket.
//!
//! The server side of this lives in `tauri-runtime-blitz`, which speaks MCP
//! JSON-RPC over endpoint-libs' length-delimited framing. Nothing in this repo
//! could speak it, which meant the one interface built for checking the window
//! without looking at it had no caller, so UI work was being reported on the
//! strength of log lines instead.
//!
//! Requests are built as JSON here rather than reusing this crate's own request
//! types. They are not the same shape: the wire enum's `rename_all =
//! "camelCase"` renames variants and leaves fields alone, so `max_depth` stays
//! snake_case on the wire while `AttrScope` in [`crate`] is a chuzz-side
//! extension the running runtime does not have. Encoding by hand keeps this
//! honest about what the browser actually accepts.

use std::io;
use std::path::{Path, PathBuf};

use endpoint_libs::libs::ws::transport::{TransportStream, framed_json};
use endpoint_libs::libs::ws::{MessageStream, WireMessage};
use serde_json::{Value, json};
use tokio::net::UnixStream;

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
}

impl Client {
    /// Connect to the browser named by a descriptor file.
    pub async fn connect(descriptor: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let descriptor: Value = serde_json::from_slice(&std::fs::read(descriptor)?)?;
        let address = descriptor["address"]
            .as_str()
            .ok_or("the descriptor has no address")?;
        let path = address.strip_prefix("unix://").unwrap_or(address);
        let stream = UnixStream::connect(path).await?;
        Ok(Self {
            stream: Box::new(TransportStream::new(framed_json(stream))),
            next_id: 1,
        })
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
        request: Value,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        let id = self.next_id;
        self.next_id += 1;
        let envelope = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {"name": tool, "arguments": request},
        });
        self.stream
            .send(WireMessage::Text(serde_json::to_string(&envelope)?))
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
                continue;
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
        // `max_depth`, not `maxDepth`. Getting this wrong is answered with a
        // JSON-RPC -32600 naming the missing field, which is at least a legible
        // way to be told.
        let response = self
            .call(json!({
                "command": "inspect",
                "params": {"root": root, "max_depth": max_depth},
            }))
            .await?;
        if response["result"] == "error" {
            return Err(format!("inspect failed: {}", response["value"]).into());
        }
        Ok(response["value"].clone())
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
            "params": {"action": "click", "params": {"node_id": node_id}},
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
                "params": {"node_id": node_id, "value": value},
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
