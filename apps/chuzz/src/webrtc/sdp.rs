//! Browser slot metadata and the deliberately narrow initial SDP profile.
//!
//! str0m parses and negotiates SDP. This module validates browser invariants
//! before that mutable negotiation and extracts identity, not packet state.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use str0m::media::Direction;

pub(super) const SLOT_LIMIT: usize = 32;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(super) struct Description {
    pub r#type: String,
    pub sdp: String,
}

#[derive(Clone, Debug)]
pub(super) struct AudioCodec {
    pub payload_type: u8,
    pub opus: bool,
    pub h264: bool,
    pub stereo: bool,
    pub fec: bool,
    pub cbr: bool,
    pub bitrate: Option<u32>,
    pub fmtp: String,
}

impl AudioCodec {
    pub(super) fn name(&self) -> &'static str {
        if self.h264 {
            "h264"
        } else if self.opus {
            "opus"
        } else {
            "pcmu"
        }
    }

    pub(super) fn parameters(&self) -> Value {
        if self.h264 {
            return json!({
                "payloadType": self.payload_type,
                "mimeType": "video/H264",
                "clockRate": 90000,
                "sdpFmtpLine": self.fmtp
            });
        }
        json!({
            "payloadType": self.payload_type,
            "mimeType": if self.opus { "audio/opus" } else { "audio/PCMU" },
            "clockRate": if self.opus { 48000 } else { 8000 },
            "channels": if self.opus { 2 } else { 1 },
            "sdpFmtpLine": self.fmtp
        })
    }
}

#[derive(Clone, Debug)]
pub(super) struct Section {
    pub kind: &'static str,
    pub mid: String,
    pub direction: String,
    pub stream: Option<String>,
    pub track: Option<String>,
    pub ufrag: String,
    pub password: String,
    pub fingerprint: String,
    pub setup: String,
    pub codecs: Vec<AudioCodec>,
}

#[derive(Default)]
struct Mapping {
    payloads: Vec<u8>,
    names: BTreeMap<u8, String>,
    formats: BTreeMap<u8, String>,
}

fn payload(value: &str) -> Result<u8, String> {
    value
        .parse::<u8>()
        .ok()
        .filter(|value| *value <= 127)
        .ok_or_else(|| "OperationError: invalid audio payload type".into())
}

fn mapping_attribute(mapping: &mut Mapping, line: &str) -> Result<(), String> {
    let (value, formats) = if let Some(value) = line.strip_prefix("a=rtpmap:") {
        (value, false)
    } else if let Some(value) = line.strip_prefix("a=fmtp:") {
        (value, true)
    } else {
        return Ok(());
    };
    let (number, value) = value
        .split_once(' ')
        .ok_or("OperationError: incomplete audio codec attribute")?;
    let number = payload(number)?;
    let target = if formats {
        &mut mapping.formats
    } else {
        &mut mapping.names
    };
    if target.insert(number, value.trim().into()).is_some() {
        return Err("OperationError: duplicate audio codec attribute".into());
    }
    Ok(())
}

fn audio_codecs(mapping: Mapping) -> Result<Vec<AudioCodec>, String> {
    let mut codecs = Vec::new();
    for number in mapping.payloads {
        let name = mapping.names.get(&number).map(String::as_str);
        let pcmu = number == 0
            && name.is_none_or(|name| {
                name.eq_ignore_ascii_case("PCMU/8000") || name.eq_ignore_ascii_case("PCMU/8000/1")
            });
        if number == 0 && !pcmu {
            return Err("OperationError: payload type zero must be mono PCMU at 8000 Hz".into());
        }
        let opus = name.is_some_and(|name| name.eq_ignore_ascii_case("opus/48000/2"));
        if !pcmu && !opus {
            continue;
        }
        let fmtp = mapping.formats.get(&number).cloned().unwrap_or_default();
        let mut codec = AudioCodec {
            payload_type: number,
            opus,
            h264: false,
            stereo: false,
            fec: false,
            cbr: false,
            bitrate: None,
            fmtp,
        };
        if opus {
            let mut keys = BTreeSet::new();
            for entry in codec
                .fmtp
                .split(';')
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
            {
                let (key, value) = entry
                    .split_once('=')
                    .ok_or("OperationError: incomplete Opus format parameter")?;
                let key = key.trim().to_ascii_lowercase();
                let value = value.trim();
                if !keys.insert(key.clone()) {
                    return Err("OperationError: duplicate Opus format parameter".into());
                }
                match key.as_str() {
                    "stereo" | "useinbandfec" | "cbr" => {
                        let enabled = match value {
                            "0" => false,
                            "1" => true,
                            _ => {
                                return Err("OperationError: invalid Opus boolean parameter".into());
                            }
                        };
                        match key.as_str() {
                            "stereo" => codec.stereo = enabled,
                            "useinbandfec" => codec.fec = enabled,
                            _ => codec.cbr = enabled,
                        }
                    }
                    "maxaveragebitrate" => {
                        codec.bitrate = Some(
                            value
                                .parse::<u32>()
                                .ok()
                                .filter(|rate| (6000..=510_000).contains(rate))
                                .ok_or("OperationError: invalid Opus bitrate")?,
                        );
                    }
                    "minptime" => {
                        let minimum = value
                            .parse::<u32>()
                            .map_err(|_| "OperationError: invalid Opus minptime")?;
                        if minimum > 20 {
                            return Err("NotSupportedError: Opus sender uses 20 ms packets".into());
                        }
                    }
                    _ => {}
                }
            }
        }
        codecs.push(codec);
    }
    if codecs.is_empty() {
        return Err("NotSupportedError: audio must offer Opus at 48000 Hz or mono PCMU".into());
    }
    Ok(codecs)
}

fn video_codecs(mapping: Mapping) -> Result<Vec<AudioCodec>, String> {
    let mut codecs = Vec::new();
    for number in mapping.payloads {
        if !mapping
            .names
            .get(&number)
            .is_some_and(|name| name.eq_ignore_ascii_case("H264/90000"))
        {
            continue;
        }
        let fmtp = mapping.formats.get(&number).cloned().unwrap_or_default();
        let mut parameters = BTreeMap::new();
        for entry in fmtp
            .split(';')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
        {
            let (key, value) = entry
                .split_once('=')
                .ok_or("OperationError: incomplete H.264 format parameter")?;
            if parameters
                .insert(key.trim().to_ascii_lowercase(), value.trim().to_owned())
                .is_some()
            {
                return Err("OperationError: duplicate H.264 format parameter".into());
            }
        }
        if parameters.get("packetization-mode").map(String::as_str) != Some("1")
            || !parameters
                .get("profile-level-id")
                .is_some_and(|profile| profile.eq_ignore_ascii_case(super::encode::PROFILE))
        {
            continue;
        }
        if parameters
            .get("level-asymmetry-allowed")
            .is_some_and(|value| !matches!(value.as_str(), "0" | "1"))
        {
            return Err("OperationError: invalid H.264 level asymmetry parameter".into());
        }
        codecs.push(AudioCodec {
            payload_type: number,
            opus: false,
            h264: true,
            stereo: false,
            fec: false,
            cbr: false,
            bitrate: None,
            fmtp,
        });
    }
    if codecs.is_empty() {
        return Err(
            "NotSupportedError: video requires H.264 mode 1 constrained baseline level 3.1".into(),
        );
    }
    Ok(codecs)
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
            return Err("NotSupportedError: one MSID per media section is supported".into());
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
        kind: "audio",
        mid: String::new(),
        direction: "sendrecv".into(),
        stream: None,
        track: None,
        ufrag: String::new(),
        password: String::new(),
        fingerprint: String::new(),
        setup: String::new(),
        codecs: Vec::new(),
    };
    let mut sections: Vec<Section> = Vec::new();
    let mut mappings: Vec<Mapping> = Vec::new();
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
                || !matches!(fields[0], "m=audio" | "m=video")
                || fields[1] == "0"
                || fields[2] != "UDP/TLS/RTP/SAVPF"
            {
                return Err(
                    "NotSupportedError: active DTLS audio or video sections are required".into(),
                );
            }
            if sections.len() == SLOT_LIMIT {
                return Err("OperationError: too many SDP sections".into());
            }
            let payloads = fields[3..]
                .iter()
                .map(|value| payload(value))
                .collect::<Result<Vec<_>, _>>()?;
            if payloads.iter().copied().collect::<BTreeSet<_>>().len() != payloads.len() {
                return Err("OperationError: duplicate m-line payload type".into());
            }
            mappings.push(Mapping {
                payloads,
                ..Mapping::default()
            });
            let mut section = defaults.clone();
            section.kind = if fields[0] == "m=video" {
                "video"
            } else {
                "audio"
            };
            sections.push(section);
        } else {
            attributes(sections.last_mut().unwrap_or(&mut defaults), line)?;
            if let Some(mapping) = mappings.last_mut() {
                mapping_attribute(mapping, line)?;
            }
            if line.starts_with("a=sctp-") || line.starts_with("a=simulcast:") {
                return Err("NotSupportedError: SCTP and simulcast are not enabled".into());
            }
        }
    }

    if sections.is_empty() {
        return Err("OperationError: SDP has no media sections".into());
    }
    for (section, mapping) in sections.iter_mut().zip(mappings) {
        section.codecs = if section.kind == "video" {
            video_codecs(mapping)?
        } else {
            audio_codecs(mapping)?
        };
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
        return Err(
            "OperationError: BUNDLE must preserve all MID identities and m-line order".into(),
        );
    }
    Ok(sections)
}

/// Apply local codec policy to str0m's generated descriptions. Payload numbers
/// retain their negotiated identity; removed codecs lose their attributes.
pub(super) fn local(
    mut description: Description,
    preferences: &[Vec<String>],
) -> Result<Description, String> {
    let sections = sections(&description)?;
    if sections.len() != preferences.len() {
        return Err("OperationError: missing local codec policy".into());
    }
    let selected: Vec<Vec<&AudioCodec>> = sections
        .iter()
        .zip(preferences)
        .map(|(section, preferences)| {
            preferences
                .iter()
                .flat_map(|name| {
                    section
                        .codecs
                        .iter()
                        .filter(move |codec| codec.name() == name)
                })
                .collect()
        })
        .collect();
    if selected.iter().any(Vec::is_empty) {
        return Err("NotSupportedError: codec preferences have no negotiated media codec".into());
    }
    let mut result = String::with_capacity(description.sdp.len() + sections.len() * 64);
    let mut index = None;
    for line in description.sdp.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with("m=") {
            index = Some(index.map_or(0, |value: usize| value + 1));
            let fields: Vec<_> = line.split_whitespace().collect();
            result.push_str(&fields[..3].join(" "));
            for codec in &selected[index.expect("media section")] {
                result.push(' ');
                result.push_str(&codec.payload_type.to_string());
            }
            result.push_str("\r\n");
            continue;
        }
        if let Some(index) = index {
            let attribute = line
                .strip_prefix("a=rtpmap:")
                .or_else(|| line.strip_prefix("a=fmtp:"))
                .or_else(|| line.strip_prefix("a=rtcp-fb:"));
            if let Some(attribute) = attribute {
                let number = attribute.split_whitespace().next().unwrap_or("");
                if number != "*" {
                    let number = payload(number)?;
                    let Some(codec) = selected[index]
                        .iter()
                        .find(|codec| codec.payload_type == number)
                    else {
                        continue;
                    };
                    if codec.opus && line.starts_with("a=fmtp:") {
                        continue;
                    }
                    if codec.opus && line.starts_with("a=rtpmap:") {
                        result.push_str(line);
                        result.push_str("\r\n");
                        result.push_str(&format!(
                            "a=fmtp:{number} minptime=10;useinbandfec=1;stereo=1;cbr=0\r\n"
                        ));
                        continue;
                    }
                }
            }
        }
        result.push_str(line);
        result.push_str("\r\n");
    }
    description.sdp = result;
    self::sections(&description)?;
    Ok(description)
}

/// Set answer directions and outgoing track identity after str0m accepts an
/// offer. Codec, ICE, DTLS, SSRC, extension and payload mappings are preserved.
pub(super) fn answer(original: &str, entries: &[(&str, Option<&str>, Option<&str>)]) -> String {
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
