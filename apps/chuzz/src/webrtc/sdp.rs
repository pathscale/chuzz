//! Browser slot metadata and the deliberately narrow initial SDP profile.
//!
//! str0m parses and negotiates SDP. This module validates browser invariants
//! before that mutable negotiation and extracts identity, not packet state.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use str0m::media::Direction;

pub(super) const SLOT_LIMIT: usize = 32;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(super) struct Description {
    pub r#type: String,
    pub sdp: String,
}

#[derive(Clone, Debug)]
pub(super) struct Section {
    pub mid: String,
    pub direction: String,
    pub stream: Option<String>,
    pub track: Option<String>,
    pub ufrag: String,
    pub password: String,
    pub fingerprint: String,
    pub setup: String,
}

pub(super) fn direction(value: &str) -> Result<Direction, String> {
    match value {
        "sendrecv" => Ok(Direction::SendRecv),
        "sendonly" => Ok(Direction::SendOnly),
        "recvonly" => Ok(Direction::RecvOnly),
        "inactive" => Ok(Direction::Inactive),
        _ => Err("TypeError: invalid transceiver direction".into()),
    }
}

pub(super) fn name(value: Direction) -> &'static str {
    match value {
        Direction::SendRecv => "sendrecv",
        Direction::SendOnly => "sendonly",
        Direction::RecvOnly => "recvonly",
        Direction::Inactive => "inactive",
    }
}

pub(super) fn sends(value: &str) -> bool {
    matches!(value, "sendrecv" | "sendonly")
}

pub(super) fn receives(value: &str) -> bool {
    matches!(value, "sendrecv" | "recvonly")
}

pub(super) fn intersect(local: &str, remote: &str) -> &'static str {
    match (
        sends(local) && receives(remote),
        receives(local) && sends(remote),
    ) {
        (true, true) => "sendrecv",
        (true, false) => "sendonly",
        (false, true) => "recvonly",
        (false, false) => "inactive",
    }
}

fn attributes(section: &mut Section, line: &str) -> Result<(), String> {
    if let Some(value) = line.strip_prefix("a=mid:") {
        if !section.mid.is_empty() {
            return Err("OperationError: duplicate MID attribute".into());
        }
        section.mid = value.into();
    } else if let Some(value) = line.strip_prefix("a=ice-ufrag:") {
        section.ufrag = value.into();
    } else if let Some(value) = line.strip_prefix("a=ice-pwd:") {
        section.password = value.into();
    } else if let Some(value) = line.strip_prefix("a=fingerprint:") {
        section.fingerprint = value.into();
    } else if let Some(value) = line.strip_prefix("a=setup:") {
        section.setup = value.into();
    } else if let Some(value) = line.strip_prefix("a=msid:") {
        let fields: Vec<_> = value.split_whitespace().collect();
        if fields.len() != 2 || section.track.is_some() {
            return Err("NotSupportedError: one MSID per audio section is supported".into());
        }
        section.stream = (fields[0] != "-").then(|| fields[0].to_owned());
        section.track = Some(fields[1].to_owned());
    } else if let Some(value) = line.strip_prefix("a=")
        && matches!(value, "sendrecv" | "sendonly" | "recvonly" | "inactive")
    {
        section.direction = value.into();
    }
    Ok(())
}

pub(super) fn sections(description: &Description) -> Result<Vec<Section>, String> {
    if !matches!(description.r#type.as_str(), "offer" | "answer") {
        return Err("NotSupportedError: only offer and answer descriptions are supported".into());
    }
    if description.sdp.len() > 128 * 1024 {
        return Err("OperationError: SDP exceeds 128 KiB".into());
    }

    let mut defaults = Section {
        mid: String::new(),
        direction: "sendrecv".into(),
        stream: None,
        track: None,
        ufrag: String::new(),
        password: String::new(),
        fingerprint: String::new(),
        setup: String::new(),
    };
    let mut sections: Vec<Section> = Vec::new();
    let mut bundle: Option<Vec<String>> = None;

    for line in description.sdp.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(value) = line.strip_prefix("a=group:BUNDLE ") {
            if bundle.is_some() {
                return Err("OperationError: duplicate BUNDLE group".into());
            }
            bundle = Some(value.split_whitespace().map(str::to_owned).collect());
        } else if line.starts_with("m=") {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() < 4
                || fields[0] != "m=audio"
                || fields[1] == "0"
                || fields[2] != "UDP/TLS/RTP/SAVPF"
                || !fields[3..].contains(&"0")
            {
                return Err(
                    "NotSupportedError: W07 supports active DTLS audio sections offering PCMU"
                        .into(),
                );
            }
            if sections.len() == SLOT_LIMIT {
                return Err("OperationError: too many SDP sections".into());
            }
            sections.push(defaults.clone());
        } else {
            attributes(sections.last_mut().unwrap_or(&mut defaults), line)?;
            if line.starts_with("a=rtpmap:0 ")
                && !matches!(
                    line.strip_prefix("a=rtpmap:0 "),
                    Some("PCMU/8000" | "PCMU/8000/1")
                )
            {
                return Err("OperationError: payload type zero must be mono PCMU at 8000 Hz".into());
            }
            if line.starts_with("a=sctp-") || line.starts_with("a=simulcast:") {
                return Err("NotSupportedError: SCTP and simulcast are not enabled".into());
            }
        }
    }

    if sections.is_empty() {
        return Err("OperationError: SDP has no audio sections".into());
    }
    let mut mids = BTreeSet::new();
    for section in &sections {
        if section.mid.is_empty()
            || section.mid.len() > 16
            || !section.mid.bytes().all(|byte| byte.is_ascii_alphanumeric())
            || !mids.insert(section.mid.clone())
        {
            return Err("OperationError: missing, duplicate or unsupported MID".into());
        }
        if section.ufrag.len() < 4
            || section.password.len() < 22
            || !section
                .ufrag
                .bytes()
                .chain(section.password.bytes())
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/'))
        {
            return Err("OperationError: invalid ICE credentials".into());
        }
        let Some(fingerprint) = section.fingerprint.strip_prefix("sha-256 ") else {
            return Err("NotSupportedError: SHA-256 DTLS fingerprints are required".into());
        };
        let bytes: Vec<_> = fingerprint.split(':').collect();
        if bytes.len() != 32
            || bytes
                .iter()
                .any(|byte| byte.len() != 2 || u8::from_str_radix(byte, 16).is_err())
        {
            return Err("OperationError: invalid DTLS fingerprint".into());
        }
        let setup_valid = if description.r#type == "offer" {
            matches!(section.setup.as_str(), "actpass" | "active" | "passive")
        } else {
            matches!(section.setup.as_str(), "active" | "passive")
        };
        if !setup_valid {
            return Err("OperationError: invalid DTLS setup role".into());
        }
        if section.ufrag != sections[0].ufrag
            || section.password != sections[0].password
            || !section
                .fingerprint
                .eq_ignore_ascii_case(&sections[0].fingerprint)
        {
            return Err("NotSupportedError: all sections must share one BUNDLE transport".into());
        }
    }
    let expected: Vec<_> = sections.iter().map(|section| section.mid.clone()).collect();
    if bundle.as_ref() != Some(&expected) {
        return Err("OperationError: BUNDLE must preserve all MID identities and m-line order".into());
    }
    Ok(sections)
}

/// Set answer directions and outgoing track identity after str0m accepts an
/// offer. Codec, ICE, DTLS, SSRC, extension and payload mappings are preserved.
pub(super) fn answer(
    original: &str,
    entries: &[(&str, Option<&str>, Option<&str>)],
) -> String {
    let mut result = String::new();
    let mut index = None;
    for line in original.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with("m=") {
            index = Some(index.map_or(0, |value: usize| value + 1));
        }
        if let Some(index) = index {
            let (direction, stream, track) = entries[index];
            if matches!(
                line,
                "a=sendrecv" | "a=sendonly" | "a=recvonly" | "a=inactive"
            ) {
                result.push_str("a=");
                result.push_str(direction);
                result.push_str("\r\n");
                if sends(direction)
                    && let Some(track) = track
                {
                    result.push_str("a=msid:");
                    result.push_str(stream.unwrap_or("-"));
                    result.push(' ');
                    result.push_str(track);
                    result.push_str("\r\n");
                }
                continue;
            }
            if line.starts_with("a=msid:") {
                continue;
            }
        }
        result.push_str(line);
        result.push_str("\r\n");
    }
    result
}

