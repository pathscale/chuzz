//! Built-in diagnostics and agent-control surface for Chuzz.
//!
//! This is an in-process interface, not a server: no socket, no listener, no
//! network transport. The browser calls into these types directly to let an
//! agent inspect the live page, drive input, and read diagnostics.
//!
//! The command and payload shapes deliberately match AgencyZero's Blitz
//! control surface, so tooling written against that surface keeps working.

use serde::{Deserialize, Serialize};

pub const CONTROL_PROTOCOL_VERSION: u32 = 1;
pub const AGENT_CONTROL_TOOL: &str = "blitz.agent.control";
pub const DIAGNOSTICS_TOOL: &str = "blitz.diagnostics";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", content = "params", rename_all = "camelCase")]
pub enum AgentControlRequest {
    Inspect { root: Option<u64>, max_depth: u32 },
    Act(AgentAction),
    Relaunch,
    Quit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", content = "params", rename_all = "camelCase")]
pub enum AgentAction {
    Click { node_id: u64 },
    SetValue { node_id: u64, value: String },
    ScrollIntoView { node_id: u64 },
    Input(InputCommand),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "input", rename_all = "camelCase")]
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
