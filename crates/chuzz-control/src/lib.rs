//! Built-in diagnostics and agent-control surface for Chuzz.
//!
//! This is an in-process interface, not a server: no socket, no listener, no
//! network transport. The browser calls into these types directly to let an
//! agent inspect the live page, drive input, and read diagnostics.
//!
//! The command and payload shapes deliberately match AgencyZero's Blitz
//! control surface, so tooling written against that surface keeps working.

use serde::{Deserialize, Serialize};

#[cfg(feature = "server")]
pub mod server;

pub const CONTROL_PROTOCOL_VERSION: u32 = 1;
pub const AGENT_CONTROL_TOOL: &str = "blitz.agent.control";
pub const DIAGNOSTICS_TOOL: &str = "blitz.diagnostics";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", content = "params", rename_all = "camelCase")]
#[serde(rename_all_fields = "camelCase")]
pub enum AgentControlRequest {
    Inspect { root: Option<u64>, max_depth: u32 },
    Act(AgentAction),
    Relaunch,
    Quit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", content = "params", rename_all = "camelCase")]
#[serde(rename_all_fields = "camelCase")]
pub enum AgentAction {
    Click { node_id: u64 },
    SetValue { node_id: u64, value: String },
    ScrollIntoView { node_id: u64 },
    Input(InputCommand),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "input", rename_all = "camelCase")]
#[serde(rename_all_fields = "camelCase")]
pub enum InputCommand {
    Key {
        phase: KeyPhase,
        key: String,
        code: String,
        modifiers: Modifiers,
    },
    Pointer {
        phase: PointerPhase,
        x: f64,
        y: f64,
        button: u16,
        modifiers: Modifiers,
    },
    Wheel {
        delta_x: f64,
        delta_y: f64,
        phase: WheelPhase,
        modifiers: Modifiers,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub meta: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeyPhase {
    Down,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PointerPhase {
    Move,
    Down,
    Up,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WheelPhase {
    Started,
    Moved,
    Ended,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", content = "params", rename_all = "camelCase")]
pub enum DiagnosticsRequest {
    Observe { streams: Vec<DebugStream> },
    Snapshot(SnapshotRequest),
    Metrics,
    WaitForIdle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DebugStream {
    Snapshots,
    Metrics,
    Console,
    RuntimeErrors,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotRequest {
    pub include_dom: bool,
    pub include_layout: bool,
    pub include_computed_style: bool,
}

/// One node as an agent sees it: identity, role, text, state and where it is
/// on screen. Mirrors AgencyZero's `SemanticNode`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticNode {
    pub id: u64,
    pub parent: Option<u64>,
    pub role: String,
    pub name: String,
    pub value: Option<String>,
    pub enabled: bool,
    pub visible: bool,
    /// `[x, y, width, height]` in CSS pixels, absent when the node has no box.
    pub bounds: Option<[f64; 4]>,
    /// The element's XML namespace, when it is not plain HTML. Inline SVG that
    /// arrives in the HTML namespace is never treated as SVG by the engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// For an image element: what the engine actually holds after fetching.
    /// A laid-out `<img>` with `None` here downloaded but never decoded, which
    /// on screen is indistinguishable from a missing file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<ImageStatus>,
}

/// What an image element resolved to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageStatus {
    /// `raster`, `svg`, or `none` when the slot is empty.
    pub kind: String,
    /// Intrinsic pixel size, when decoded.
    pub intrinsic: Option<[u32; 2]>,
}

/// A settled view of the document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSnapshot {
    pub revision: u64,
    pub url: Option<String>,
    pub title: Option<String>,
    pub focused_node: Option<u64>,
    pub nodes: Vec<SemanticNode>,
}

/// What the browser answers with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", content = "value", rename_all = "camelCase")]
pub enum ControlResponse {
    Ok,
    Snapshot(AgentSnapshot),
    Error(ControlError),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlError {
    pub code: String,
    pub message: String,
}

impl ControlError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_match_the_existing_blitz_surface() {
        assert_eq!(AGENT_CONTROL_TOOL, "blitz.agent.control");
        assert_eq!(DIAGNOSTICS_TOOL, "blitz.diagnostics");
    }

    #[test]
    fn typed_key_input_uses_the_compatible_tagged_shape() {
        let request = AgentControlRequest::Act(AgentAction::Input(InputCommand::Key {
            phase: KeyPhase::Down,
            key: "k".into(),
            code: "KeyK".into(),
            modifiers: Modifiers {
                meta: true,
                ..Default::default()
            },
        }));

        let encoded = serde_json::to_value(&request).unwrap();
        assert_eq!(encoded["command"], "act");
        assert_eq!(encoded["params"]["action"], "input");
        assert_eq!(encoded["params"]["params"]["input"], "key");
        assert_eq!(
            serde_json::from_value::<AgentControlRequest>(encoded).unwrap(),
            request
        );
    }

    #[test]
    fn request_fields_are_camel_case_like_every_other_field() {
        let encoded = serde_json::to_value(AgentControlRequest::Inspect {
            root: None,
            max_depth: 3,
        })
        .unwrap();
        assert_eq!(encoded["params"]["maxDepth"], 3);
        assert!(
            encoded["params"].get("max_depth").is_none(),
            "snake_case leaked into the wire format"
        );
    }

    #[test]
    fn a_snapshot_response_round_trips() {
        let response = ControlResponse::Snapshot(AgentSnapshot {
            revision: 7,
            url: Some("https://24x.ai/".into()),
            title: Some("24x.ai".into()),
            focused_node: None,
            nodes: vec![SemanticNode {
                id: 1,
                parent: None,
                role: "svg".into(),
                namespace: Some("http://www.w3.org/2000/svg".into()),
                image: Some(ImageStatus {
                    kind: "svg".into(),
                    intrinsic: Some([744, 221]),
                }),
                name: String::new(),
                value: None,
                enabled: true,
                visible: true,
                bounds: Some([16.0, 12.0, 210.0, 44.0]),
            }],
        });
        let encoded = serde_json::to_value(&response).unwrap();
        assert_eq!(encoded["result"], "snapshot");
        assert_eq!(encoded["value"]["nodes"][0]["bounds"][2], 210.0);
        assert_eq!(
            serde_json::from_value::<ControlResponse>(encoded).unwrap(),
            response
        );
    }

    #[test]
    fn an_error_response_carries_a_code() {
        let response = ControlResponse::Error(ControlError::new("no_such_node", "node 42 is gone"));
        let encoded = serde_json::to_value(&response).unwrap();
        assert_eq!(encoded["result"], "error");
        assert_eq!(encoded["value"]["code"], "no_such_node");
        assert_eq!(
            serde_json::from_value::<ControlResponse>(encoded).unwrap(),
            response
        );
    }

    #[test]
    fn diagnostic_snapshot_request_round_trips() {
        let request = DiagnosticsRequest::Snapshot(SnapshotRequest {
            include_dom: true,
            include_layout: true,
            include_computed_style: false,
        });
        let encoded = serde_json::to_value(&request).unwrap();
        assert_eq!(encoded["command"], "snapshot");
        assert_eq!(
            serde_json::from_value::<DiagnosticsRequest>(encoded).unwrap(),
            request
        );
    }
}
