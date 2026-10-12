//! One task owns one Rtc, its sockets, pacing and protocol deadlines.
//!
//! Opus is preferred, with PCMU fallback. Codec calls and bounded receive
//! lookahead run synchronously here; adaptive playout remains a later wave.

use std::collections::VecDeque;
use std::future::{Future, poll_fn};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::task::Poll;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use str0m::change::{SdpAnswer, SdpOffer, SdpPendingOffer};
use str0m::crypto::dtls::{DtlsCert, ProtocolVersion};
use str0m::format::Codec;
use str0m::media::{MediaKind, MediaTime, Mid};
use str0m::net::{Protocol, Receive, Transmit};
use str0m::{Candidate, Event, Input, Output, Rtc};

use crate::audio::pcm::Track as AudioTrack;
use crate::media::frame::Cancellation;
use crate::media::pcm::Pcm;
use crate::media::track::Track;

use super::gather::{self, Config, Socket};
use super::host::{Job, Peer};
use super::sdp::{self, Description, Section};
use super::{encode, opus, pcmu, receive};

const AUDIO_QUANTUM: Duration = Duration::from_millis(20);
const OUTPUTS: usize = 128;
const TRANSMITS: usize = 256;

struct Slot {
    id: u64,
    kind: &'static str,
    mid: Option<Mid>,
    direction: String,
    current: Option<String>,
    stream: Option<String>,
    track: Option<Arc<Track>>,
    input: Option<Arc<AudioTrack>>,
    output: Option<Arc<AudioTrack>>,
    receiver: Option<Arc<Track>>,
    remote: Option<Section>,
    active: bool,
    rx_next: u64,
    silence_run: u64,
    preferences: Vec<String>,
    codecs: Vec<sdp::AudioCodec>,
    send: opus::Sender,
    receive: Option<Box<opus::Receiver>>,
    video_send: encode::Sender,
    video_receive: receive::Receiver,
}

struct Sink<'a> {
    output: &'a AudioTrack,
    receiver: &'a Track,
    next: &'a mut u64,
    silence_run: &'a mut u64,
    generation: crate::media::frame::Generation,
    peer: &'a Peer,
}

impl Sink<'_> {
    fn push(&mut self, decoded: opus::Decoded) -> Result<(), String> {
        let pcm = Arc::new(
            Pcm::new(
                self.receiver.source.id,
                opus::RATE,
                2,
                decoded.position,
                self.generation.clone(),
                decoded.samples,
            )
            .map_err(|error| format!("{}: {}", error.name, error.message))?,
        );
        let gap = if *self.next == 0 {
            0
        } else {
            decoded.position.saturating_sub(*self.next)
        };
        let mut silence_max = *self.silence_run;
        for frame in pcm.samples().as_chunks::<2>().0 {
            if frame.iter().all(|sample| sample.abs() < 1.0e-6) {
                *self.silence_run += 1;
                silence_max = silence_max.max(*self.silence_run);
            } else {
                *self.silence_run = 0;
            }
        }
        let frames = pcm.frames() as u64;
        let energy = pcm.energy();
        self.output.push(pcm)?;
        *self.next = decoded.position + frames;
        self.receiver.source.muted.store(false, Ordering::Release);
        self.peer.metrics(|metrics| {
            metrics.samples_received += frames;
            metrics.received_energy += energy;
            metrics.concealed_frames += decoded.concealed_frames;
            metrics.fec_attempts += decoded.fec_attempts;
            if decoded.fec_attempts == 0 {
                metrics.plc_frames += decoded.concealed_frames;
            }
            metrics.codec_errors += decoded.errors;
            metrics.max_silence_frames = metrics.max_silence_frames.max(silence_max);
            metrics.max_playout_gap_frames = metrics.max_playout_gap_frames.max(gap);
        });
        Ok(())
    }
}

struct Engine {
    rtc: Rtc,
    certificate: DtlsCert,
    candidates: Vec<Candidate>,
    slots: Vec<Slot>,
    pending: Option<SdpPendingOffer>,
    generated_offer: Option<Description>,
    generated_answer: Option<Description>,
    pending_local: Option<Description>,
    pending_remote: Option<Description>,
    current_local: Option<Description>,
    current_remote: Option<Description>,
    signaling: &'static str,
    restart: bool,
    connected: bool,
    epoch: Instant,
    audio_deadline: Instant,
    audio_drop: u64,
    video_drop: bool,
    video_lost: Option<(u32, Instant)>,
    video_recovery: bool,
    cancellation: Cancellation,
}

fn rtc(certificate: DtlsCert) -> Rtc {
    super::crypto::config()
        .set_dtls_cert(certificate)
        .clear_codecs()
        .enable_opus(true, false)
        .enable_pcmu(true, false)
        .enable_h264(true)
        .set_reordering_size_video(0)
        // Receiver owns timestamp reordering and the 80 ms playout budget.
        // str0m's default 15-packet wait after loss exceeds that budget
        // and releases more packets than the 12-packet lookahead can hold.
        .set_reordering_size_audio(0)
        .build(Instant::now())
}

fn slot_id(data: &Value) -> Result<u64, String> {
    data["slot"]
        .as_u64()
        .filter(|id| *id > 0 && *id <= sdp::SLOT_LIMIT as u64)
        .ok_or_else(|| "TypeError: invalid transceiver slot".into())
}

fn description(data: &Value) -> Result<Description, String> {
    serde_json::from_value(data["description"].clone())
        .map_err(|_| "TypeError: invalid session description".into())
}

impl Engine {
    fn local_sdp(&self, description: Description) -> Result<Description, String> {
        let sections = sdp::sections(&description)?;
        let preferences = sections
            .iter()
            .map(|section| {
                self.slots
                    .iter()
                    .find(|slot| slot.mid.is_some_and(|mid| mid.to_string() == section.mid))
                    .map(|slot| slot.preferences.clone())
                    .ok_or_else(|| "OperationError: missing local codec slot".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
        sdp::local(description, &preferences)
    }

    fn commit_codecs(&mut self) -> Result<(), String> {
        let local = self
            .current_local
            .as_ref()
            .ok_or("OperationError: missing local SDP")?;
        let remote = self
            .current_remote
            .as_ref()
            .ok_or("OperationError: missing remote SDP")?;
        let answer = if local.r#type == "answer" {
            local
        } else {
            remote
        };
        let answers = sdp::sections(answer)?;
        let remotes = sdp::sections(remote)?;
        for section in answers {
            let remote = remotes
                .iter()
                .find(|remote| remote.mid == section.mid)
                .ok_or("OperationError: missing remote codec section")?;
            let slot = self
                .slots
                .iter_mut()
                .find(|slot| slot.mid.is_some_and(|mid| mid.to_string() == section.mid))
                .ok_or("OperationError: missing negotiated codec slot")?;
            let old_opus = slot
                .codecs
                .iter()
                .find(|codec| codec.opus)
                .map(|codec| codec.payload_type);
            slot.codecs = section
                .codecs
                .into_iter()
                .map(|codec| {
                    // Opus fmtp describes the remote receiver's preferences.
                    // Preserve the answer's payload order and identity.
                    remote
                        .codecs
                        .iter()
                        .find(|remote| {
                            remote.payload_type == codec.payload_type
                                && remote.name() == codec.name()
                        })
                        .cloned()
                        .unwrap_or(codec)
                })
                .collect();
            let new_opus = slot
                .codecs
                .iter()
                .find(|codec| codec.opus)
                .map(|codec| codec.payload_type);
            if old_opus != new_opus {
                slot.receive = None;
            }
        }
        Ok(())
    }

    fn slot(&mut self, id: u64) -> Result<&mut Slot, String> {
        self.slots
            .iter_mut()
            .find(|slot| slot.id == id)
            .ok_or_else(|| "InvalidAccessError: unknown transceiver".into())
    }

    fn snapshot(&self) -> Value {
        let slots: Vec<_> = self
            .slots
            .iter()
            .map(|slot| {
                let remote = slot.remote.as_ref().filter(|remote| {
                    sdp::receives(sdp::intersect(&slot.direction, &remote.direction))
                });
                json!({
                    "slot": slot.id,
                    "kind": slot.kind,
                    "mid": slot.mid.map(|mid| mid.to_string()),
                    "direction": slot.direction,
                    "currentDirection": slot.current,
                    "codecs": slot.codecs.iter().map(sdp::AudioCodec::parameters).collect::<Vec<_>>(),
                    "remote": remote.map(|remote| json!({
                        "stream": remote.stream,
                        "track": remote.track
                    }))
                })
            })
            .collect();
        json!({
            "signalingState": self.signaling,
            "localDescription": self.pending_local.as_ref().or(self.current_local.as_ref()),
            "remoteDescription": self.pending_remote.as_ref().or(self.current_remote.as_ref()),
            "pendingLocalDescription": self.pending_local,
            "pendingRemoteDescription": self.pending_remote,
            "currentLocalDescription": self.current_local,
            "currentRemoteDescription": self.current_remote,
            "slots": slots
        })
    }

    fn local_candidates(&self, peer: &Peer, description: &Description) -> Result<(), String> {
        let sections = sdp::sections(description)?;
        let first = &sections[0];
        peer.post(json!({"type": "gathering", "state": "gathering"}));
        for candidate in &self.candidates {
            peer.post(json!({
                "type": "candidate",
                "candidate": {
                    "candidate": candidate.to_sdp_string(),
                    "sdpMid": first.mid,
                    "sdpMLineIndex": 0,
                    "usernameFragment": first.ufrag
                }
            }));
        }
        peer.post(json!({"type": "gathering", "state": "complete"}));
        peer.post(json!({"type": "candidate", "candidate": null}));
        Ok(())
    }

    /// Validate the supported SDP profile against a disposable protocol core
    /// before giving it to the live mutable Rtc.
    fn preflight(&self, description: &Description) -> Result<Vec<Section>, String> {
        let sections = sdp::sections(description)?;
        let offer = SdpOffer::from_sdp_string(&description.sdp)
            .map_err(|error| format!("OperationError: {error}"))?;
        rtc(self.certificate.clone())
            .sdp_api()
            .accept_offer(offer)
            .map_err(|error| format!("OperationError: {error}"))?;

        if let Some(current) = &self.current_remote {
            let previous = sdp::sections(current)?;
            if sections.len() < previous.len()
                || previous
                    .iter()
                    .zip(&sections)
                    .any(|(old, new)| old.mid != new.mid || old.kind != new.kind)
            {
                return Err(
                    "OperationError: established MID identity and m-line order are immutable"
                        .into(),
                );
            }
            if !previous[0]
                .fingerprint
                .eq_ignore_ascii_case(&sections[0].fingerprint)
            {
                return Err("OperationError: established DTLS fingerprint changed".into());
            }
            if description.r#type == "answer"
                && !self.restart
                && (previous[0].ufrag != sections[0].ufrag
                    || previous[0].password != sections[0].password)
            {
                return Err("OperationError: unsolicited ICE restart in answer".into());
            }
        }
        Ok(sections)
    }

    fn offer(&mut self) -> Result<Description, String> {
        if !matches!(self.signaling, "stable" | "have-local-offer") {
            return Err("InvalidStateError: cannot create an offer in this signaling state".into());
        }
        if self.slots.is_empty() {
            return Err("OperationError: add a media transceiver before creating an offer".into());
        }
        let mut changes = self.rtc.sdp_api();
        if let Some(pending) = self.pending.take() {
            changes.merge(pending);
        }
        for slot in &mut self.slots {
            let direction = sdp::direction(&slot.direction)?;
            if let Some(mid) = slot.mid {
                changes.set_direction(mid, direction);
            } else {
                slot.mid = Some(
                    changes.add_media(
                        if slot.kind == "video" {
                            MediaKind::Video
                        } else {
                            MediaKind::Audio
                        },
                        direction,
                        slot.stream.clone(),
                        slot.track
                            .as_ref()
                            .map(|track| format!("chuzz-track-{}", track.id)),
                        None,
                    ),
                );
            }
        }
        if self.restart {
            changes.ice_restart(true);
        }
        if !changes.has_changes() {
            let slot = &self.slots[0];
            let mid = slot.mid.ok_or("OperationError: missing negotiated MID")?;
            let desired = sdp::direction(&slot.direction)?;
            let temporary = if slot.direction == "inactive" {
                sdp::direction("sendrecv")?
            } else {
                sdp::direction("inactive")?
            };
            changes.set_direction(mid, temporary);
            changes.set_direction(mid, desired);
        }
        let (offer, pending) = changes
            .apply()
            .ok_or("OperationError: str0m did not produce an offer")?;
        let description = self.local_sdp(Description {
            r#type: "offer".into(),
            sdp: offer.to_sdp_string(),
        })?;
        sdp::sections(&description)?;
        self.pending = Some(pending);
        self.generated_offer = Some(description.clone());
        Ok(description)
    }

    fn answer(&mut self) -> Result<Description, String> {
        if self.signaling != "have-remote-offer" {
            return Err("InvalidStateError: createAnswer requires a remote offer".into());
        }
        let generated = self
            .generated_answer
            .as_ref()
            .ok_or("OperationError: missing generated answer")?;
        let sections = sdp::sections(generated)?;
        let mut identities = Vec::new();
        for section in &sections {
            let slot = self
                .slots
                .iter()
                .find(|slot| slot.mid.is_some_and(|mid| mid.to_string() == section.mid))
                .ok_or("OperationError: answer section has no slot")?;
            let remote = slot
                .remote
                .as_ref()
                .ok_or("OperationError: missing remote slot")?;
            identities.push((
                sdp::intersect(&slot.direction, &remote.direction).to_owned(),
                slot.stream.clone(),
                slot.track
                    .as_ref()
                    .map(|track| format!("chuzz-track-{}", track.id)),
            ));
        }
        {
            let mut changes = self.rtc.sdp_api();
            for (section, (direction, _, _)) in sections.iter().zip(&identities) {
                let slot = self
                    .slots
                    .iter()
                    .find(|slot| slot.mid.is_some_and(|mid| mid.to_string() == section.mid))
                    .ok_or("OperationError: missing answer MID")?;
                changes.set_direction(
                    slot.mid.ok_or("OperationError: missing answer MID")?,
                    sdp::direction(direction)?,
                );
            }
        }
        let entries: Vec<_> = identities
            .iter()
            .map(|(direction, stream, track)| {
                (direction.as_str(), stream.as_deref(), track.as_deref())
            })
            .collect();
        let description = self.local_sdp(Description {
            r#type: "answer".into(),
            sdp: sdp::answer(&generated.sdp, &entries),
        })?;
        sdp::sections(&description)?;
        SdpAnswer::from_sdp_string(&description.sdp)
            .map_err(|error| format!("OperationError: {error}"))?;
        self.generated_answer = Some(description.clone());
        Ok(description)
    }

    fn set_local(&mut self, value: Description, peer: &Peer) -> Result<Value, String> {
        match value.r#type.as_str() {
            "offer" if matches!(self.signaling, "stable" | "have-local-offer") => {
                if self.generated_offer.as_ref() != Some(&value) || self.pending.is_none() {
                    return Err(
                        "OperationError: local offer must be the current generated offer".into(),
                    );
                }
                self.pending_local = Some(value.clone());
                self.signaling = "have-local-offer";
            }
            "answer" if self.signaling == "have-remote-offer" => {
                if self.generated_answer.as_ref() != Some(&value) {
                    return Err(
                        "OperationError: local answer must be the current generated answer".into(),
                    );
                }
                self.current_local = Some(value.clone());
                self.current_remote = self.pending_remote.take();
                self.pending_local = None;
                self.signaling = "stable";
                for slot in &mut self.slots {
                    if let Some(remote) = &slot.remote {
                        slot.current =
                            Some(sdp::intersect(&slot.direction, &remote.direction).into());
                    }
                }
            }
            _ => return Err("InvalidStateError: invalid local-description transition".into()),
        }
        if self.signaling == "stable" {
            self.commit_codecs()?;
        }
        let snapshot = self.snapshot();
        // The reply is posted by the caller before gathering notifications.
        peer.post(json!({"type": "snapshot", "snapshot": snapshot}));
        self.local_candidates(peer, &value)?;
        Ok(json!({"snapshot": self.snapshot()}))
    }

    fn set_remote(&mut self, value: Description, peer: &Peer) -> Result<Value, String> {
        match value.r#type.as_str() {
            "offer" if self.signaling == "stable" => {
                let sections = self.preflight(&value)?;
                let mut assignments = Vec::new();
                let mut used = Vec::new();
                for section in &sections {
                    let existing = self
                        .slots
                        .iter()
                        .find(|slot| slot.mid.is_some_and(|mid| mid.to_string() == section.mid))
                        .or_else(|| {
                            self.slots.iter().find(|slot| {
                                slot.mid.is_none()
                                    && slot.kind == section.kind
                                    && !used.contains(&slot.id)
                            })
                        });
                    let id = existing.map(|slot| slot.id).unwrap_or_else(|| {
                        self.slots.len() as u64
                            + assignments
                                .iter()
                                .filter(|(_, id): &&(Section, u64)| {
                                    !self.slots.iter().any(|slot| slot.id == *id)
                                })
                                .count() as u64
                            + 1
                    });
                    if id > sdp::SLOT_LIMIT as u64 || used.contains(&id) {
                        return Err("OperationError: remote transceiver limit reached".into());
                    }
                    used.push(id);
                    assignments.push((section.clone(), id));
                }
                let offer = SdpOffer::from_sdp_string(&value.sdp)
                    .map_err(|error| format!("OperationError: {error}"))?;
                let answer = self
                    .rtc
                    .sdp_api()
                    .accept_offer(offer)
                    .map_err(|error| format!("OperationError: {error}"))?;
                // Receiver identity is committed only after protocol acceptance.
                for (section, id) in assignments {
                    let mid = section.mid.as_str().into();
                    if !self.slots.iter().any(|slot| slot.id == id) {
                        let kind = section.kind;
                        self.slots.push(Slot {
                            id,
                            kind,
                            mid: Some(mid),
                            direction: "recvonly".into(),
                            current: None,
                            stream: None,
                            track: None,
                            input: None,
                            output: None,
                            receiver: None,
                            remote: Some(section),
                            active: true,
                            rx_next: 0,
                            silence_run: 0,
                            preferences: if kind == "video" {
                                vec!["h264".into()]
                            } else {
                                vec!["opus".into(), "pcmu".into()]
                            },
                            codecs: Vec::new(),
                            send: opus::Sender::new(),
                            receive: None,
                            video_send: encode::Sender::new(),
                            video_receive: receive::Receiver::new(),
                        });
                    } else {
                        let slot = self.slot(id)?;
                        slot.mid = Some(mid);
                        slot.remote = Some(section);
                    }
                }
                self.generated_answer = Some(Description {
                    r#type: "answer".into(),
                    sdp: answer.to_sdp_string(),
                });
                self.pending_remote = Some(value);
                self.signaling = "have-remote-offer";
            }
            "answer" if self.signaling == "have-local-offer" => {
                let sections = self.preflight(&value)?;
                let offered = sdp::sections(
                    self.pending_local
                        .as_ref()
                        .ok_or("OperationError: missing local offer")?,
                )?;
                if sections.len() != offered.len()
                    || sections.iter().zip(&offered).any(|(answer, offer)| {
                        answer.mid != offer.mid
                            || answer.kind != offer.kind
                            || answer.codecs.iter().any(|codec| {
                                !offer.codecs.iter().any(|offered| {
                                    offered.payload_type == codec.payload_type
                                        && offered.name() == codec.name()
                                })
                            })
                            || (sdp::sends(&answer.direction) && !sdp::receives(&offer.direction))
                            || (sdp::receives(&answer.direction) && !sdp::sends(&offer.direction))
                    })
                {
                    return Err(
                        "OperationError: answer does not match the offered slot table".into(),
                    );
                }
                let answer = SdpAnswer::from_sdp_string(&value.sdp)
                    .map_err(|error| format!("OperationError: {error}"))?;
                let pending = self
                    .pending
                    .take()
                    .ok_or("OperationError: missing pending str0m offer")?;
                self.rtc
                    .sdp_api()
                    .accept_answer(pending, answer)
                    .map_err(|error| format!("OperationError: {error}"))?;
                for section in sections {
                    let slot = self
                        .slots
                        .iter_mut()
                        .find(|slot| slot.mid.is_some_and(|mid| mid.to_string() == section.mid))
                        .ok_or("OperationError: missing answer slot")?;
                    slot.current = Some(sdp::intersect(&slot.direction, &section.direction).into());
                    slot.remote = Some(section);
                }
                self.current_local = self.pending_local.take();
                self.current_remote = Some(value);
                self.pending_remote = None;
                self.generated_offer = None;
                self.restart = false;
                self.signaling = "stable";
            }
            _ => return Err("InvalidStateError: invalid remote-description transition".into()),
        }
        if self.signaling == "stable" {
            self.commit_codecs()?;
        }
        if !self.connected {
            peer.post(json!({"type": "connection", "state": "connecting"}));
        }
        Ok(json!({"snapshot": self.snapshot()}))
    }

    fn command(&mut self, job: Job, peer: &Peer) -> Result<Value, String> {
        match job.data["op"].as_str().unwrap_or("") {
            "add" => {
                let id = slot_id(&job.data)?;
                if self.slots.iter().any(|slot| slot.id == id)
                    || self.slots.len() >= sdp::SLOT_LIMIT
                {
                    return Err("InvalidAccessError: duplicate or excessive transceiver".into());
                }
                let kind = match job.data["kind"].as_str() {
                    Some("audio") => "audio",
                    Some("video") => "video",
                    _ => return Err("TypeError: invalid transceiver kind".into()),
                };
                self.slots.push(Slot {
                    id,
                    kind,
                    mid: None,
                    direction: job.data["direction"].as_str().unwrap_or("sendrecv").into(),
                    current: None,
                    stream: job.data["stream"].as_str().map(str::to_owned),
                    track: job.track,
                    input: job.input,
                    output: job.output,
                    receiver: job.receiver,
                    remote: None,
                    active: true,
                    rx_next: 0,
                    silence_run: 0,
                    preferences: if kind == "video" {
                        vec!["h264".into()]
                    } else {
                        vec!["opus".into(), "pcmu".into()]
                    },
                    codecs: Vec::new(),
                    send: opus::Sender::new(),
                    receive: None,
                    video_send: encode::Sender::new(),
                    video_receive: receive::Receiver::new(),
                });
                Ok(Value::Null)
            }
            "bind" => {
                let slot = self.slot(slot_id(&job.data)?)?;
                if job
                    .receiver
                    .as_ref()
                    .is_none_or(|receiver| receiver.source.kind.name() != slot.kind)
                {
                    return Err("InvalidAccessError: receiver kind does not match its slot".into());
                }
                slot.output = job.output;
                slot.receiver = job.receiver;
                Ok(Value::Null)
            }
            "direction" => {
                self.slot(slot_id(&job.data)?)?.direction = job.data["direction"]
                    .as_str()
                    .ok_or("TypeError: direction is required")?
                    .into();
                Ok(Value::Null)
            }
            "replace" => {
                let slot = self.slot(slot_id(&job.data)?)?;
                if job
                    .track
                    .as_ref()
                    .is_some_and(|track| track.source.kind.name() != slot.kind)
                {
                    return Err("InvalidModificationError: replacement track kind changed".into());
                }
                slot.video_send.clear();
                slot.track = job.track;
                slot.input = job.input;
                Ok(Value::Null)
            }
            "active" => {
                self.slot(slot_id(&job.data)?)?.active = job.data["active"]
                    .as_bool()
                    .ok_or("TypeError: active must be boolean")?;
                Ok(Value::Null)
            }
            "parameters" => {
                let slot = self.slot(slot_id(&job.data)?)?;
                let active = job.data["active"]
                    .as_bool()
                    .ok_or("TypeError: active must be boolean")?;
                if slot.kind == "video" {
                    let bitrate = job.data["maxBitrate"]
                        .as_u64()
                        .filter(|bitrate| (6000..=1_000_000).contains(bitrate))
                        .ok_or("RangeError: video bitrate must be between 6000 and 1000000")?;
                    let frame_rate = job.data["maxFramerate"]
                        .as_f64()
                        .filter(|rate| rate.is_finite() && (0.25..=30.0).contains(rate))
                        .ok_or("RangeError: video frame rate must be between 0.25 and 30")?;
                    let scale = job.data["scaleResolutionDownBy"]
                        .as_f64()
                        .filter(|scale| scale.is_finite() && (1.0..=32.0).contains(scale))
                        .ok_or("RangeError: video scale must be between 1 and 32")?;
                    slot.video_send.bitrate = bitrate as u32;
                    slot.video_send.max_frame_rate = frame_rate;
                    slot.video_send.scale = scale;
                    slot.video_send.clear();
                } else {
                    let bitrate = job.data["maxBitrate"]
                        .as_u64()
                        .filter(|bitrate| (6000..=510_000).contains(bitrate))
                        .ok_or("RangeError: Opus bitrate must be between 6000 and 510000")?;
                    let cbr = job.data["cbr"]
                        .as_bool()
                        .ok_or("TypeError: cbr must be boolean")?;
                    slot.send.bitrate = bitrate as u32;
                    slot.send.cbr = cbr;
                }
                slot.active = active;
                Ok(Value::Null)
            }
            "preferences" => {
                if self.signaling != "stable" {
                    return Err(
                        "InvalidStateError: change codec preferences in stable signaling".into(),
                    );
                }
                let values = job.data["codecs"]
                    .as_array()
                    .ok_or("TypeError: codec preferences must be an array")?;
                let kind = self.slot(slot_id(&job.data)?)?.kind;
                let mut preferences = Vec::new();
                for value in values {
                    let name = value
                        .as_str()
                        .ok_or("TypeError: invalid codec preference")?;
                    let supported = if kind == "video" {
                        name == "h264"
                    } else {
                        matches!(name, "opus" | "pcmu")
                    };
                    if !supported || preferences.iter().any(|value| value == name) {
                        return Err("NotSupportedError: invalid media codec preference".into());
                    }
                    preferences.push(name.to_owned());
                }
                if preferences.is_empty() {
                    preferences = if kind == "video" {
                        vec!["h264".into()]
                    } else {
                        vec!["opus".into(), "pcmu".into()]
                    };
                }
                self.slot(slot_id(&job.data)?)?.preferences = preferences;
                Ok(Value::Null)
            }
            "dropVideo" => {
                self.video_drop = true;
                self.video_lost = None;
                Ok(Value::Null)
            }
            "dropAudio" => {
                self.audio_drop = job.data["count"]
                    .as_u64()
                    .filter(|count| (1..=5).contains(count))
                    .ok_or("RangeError: drop between one and five RTP packets")?;
                Ok(Value::Null)
            }
            "createOffer" => Ok(json!({"description": self.offer()?})),
            "createAnswer" => Ok(json!({"description": self.answer()?})),
            "setLocal" => self.set_local(description(&job.data)?, peer),
            "setRemote" => self.set_remote(description(&job.data)?, peer),
            "restart" => {
                self.restart = true;
                Ok(Value::Null)
            }
            "candidate" => {
                let value = &job.data["candidate"];
                if value.is_null() || value["candidate"] == "" {
                    return Ok(Value::Null);
                }
                let remote = self
                    .pending_remote
                    .as_ref()
                    .or(self.current_remote.as_ref())
                    .ok_or(
                        "InvalidStateError: set a remote description before adding candidates",
                    )?;
                let sections = sdp::sections(remote)?;
                let mid = value["sdpMid"].as_str();
                let index = value["sdpMLineIndex"].as_u64();
                let section = if let Some(mid) = mid {
                    sections.iter().position(|section| section.mid == mid)
                } else {
                    index.and_then(|index| usize::try_from(index).ok())
                }
                .and_then(|index| sections.get(index))
                .ok_or("OperationError: candidate does not identify a remote section")?;
                if index.is_some_and(|index| {
                    sections
                        .get(index as usize)
                        .is_none_or(|indexed| indexed.mid != section.mid)
                }) || value["usernameFragment"]
                    .as_str()
                    .is_some_and(|ufrag| ufrag != section.ufrag)
                {
                    return Err(
                        "OperationError: candidate belongs to another ICE generation or MID".into(),
                    );
                }
                let candidate = Candidate::from_sdp_string(
                    value["candidate"]
                        .as_str()
                        .ok_or("TypeError: candidate text is required")?,
                )
                .map_err(|error| format!("OperationError: {error}"))?;
                self.rtc.add_remote_candidate(candidate);
                Ok(Value::Null)
            }
            _ => Err("TypeError: unknown peer operation".into()),
        }
    }

    fn playout(&mut self, peer: &Peer, now: Instant) -> Result<Option<Instant>, String> {
        if !self.connected {
            return Ok(None);
        }
        let generation = self.cancellation.generation();
        let mut deadline = None;
        for slot in &mut self.slots {
            if slot.current.as_deref().is_some_and(sdp::receives)
                && let (Some(receive), Some(output), Some(receiver)) =
                    (&mut slot.receive, &slot.output, &slot.receiver)
                && !receiver.is_stopped()
            {
                let mut sink = Sink {
                    output,
                    receiver,
                    next: &mut slot.rx_next,
                    silence_run: &mut slot.silence_run,
                    generation: generation.clone(),
                    peer,
                };
                receive.render(now, self.epoch, |decoded| sink.push(decoded))?;
                if let Some(due) = receive.deadline() {
                    deadline = Some(deadline.map_or(due, |value: Instant| value.min(due)));
                }
            }
        }
        Ok(deadline)
    }

    fn audio(&mut self, peer: &Peer, now: Instant) -> Result<(), String> {
        if !self.connected || self.signaling != "stable" || now < self.audio_deadline {
            return Ok(());
        }
        let elapsed = now.saturating_duration_since(self.epoch);
        // Capture and RTP use the same integer 20 ms clock. Scheduler jitter
        // must not create fractional-frame gaps in the receiver's RTP clock.
        let start = Duration::from_millis((elapsed.as_millis() / 20) as u64 * 20);
        // Skip genuine capture debt, but keep ordinary wakes on the epoch's
        // clock. Resetting to now + 20 ms accumulates scheduler delay and
        // creates artificial missing RTP frames.
        self.audio_deadline = self.epoch + start + AUDIO_QUANTUM;
        for slot in &mut self.slots {
            if slot.kind != "audio"
                || !slot.active
                || !slot.current.as_deref().is_some_and(sdp::sends)
                || slot.track.as_ref().is_none_or(|track| track.is_stopped())
            {
                continue;
            }
            let (Some(input), Some(mid), Some(codec)) =
                (&slot.input, slot.mid, slot.codecs.first())
            else {
                continue;
            };
            let Some(writer) = self.rtc.writer(mid) else {
                continue;
            };
            let Some(pt) = writer
                .payload_params()
                .find(|params| {
                    params.pt() == codec.payload_type.into()
                        && params.spec().codec == if codec.opus { Codec::Opus } else { Codec::PCMU }
                })
                .map(|params| params.pt())
            else {
                continue;
            };
            let bytes = if codec.opus {
                slot.send.encode(input, start, codec)?
            } else {
                pcmu::encode(&input.read(start, 160, 8000, 1))
            };
            let length = bytes.len();
            // Preserve the integer capture clock through RTP rebasing.
            // Floating-point seconds can truncate a microsecond, placing
            // a packet one sample before Receiver's expected timestamp.
            writer
                .write(
                    pt,
                    now,
                    MediaTime::from_millis(start.as_millis() as u64),
                    bytes,
                )
                .map_err(|error| format!("OperationError: {error}"))?;
            peer.metrics(|metrics| {
                metrics.tx_codec = Some(codec.name());
                metrics.samples_sent += if codec.opus { opus::FRAME as u64 } else { 160 };
                if codec.opus {
                    metrics.opus_packets_sent += 1;
                    metrics.opus_bytes_sent += length as u64;
                    metrics.opus_min_packet_bytes = Some(
                        metrics
                            .opus_min_packet_bytes
                            .map_or(length, |value| value.min(length)),
                    );
                    metrics.opus_max_packet_bytes = Some(
                        metrics
                            .opus_max_packet_bytes
                            .map_or(length, |value| value.max(length)),
                    );
                }
            });
        }
        Ok(())
    }

    fn video(&mut self, peer: &Peer, now: Instant) -> Result<Option<Instant>, String> {
        if !self.connected || self.signaling != "stable" {
            return Ok(None);
        }
        let mut deadline = None;
        for slot in &mut self.slots {
            if slot.kind != "video"
                || !slot.active
                || !slot.current.as_deref().is_some_and(sdp::sends)
            {
                slot.video_send.clear();
                continue;
            }
            let (Some(track), Some(mid), Some(codec)) =
                (&slot.track, slot.mid, slot.codecs.first())
            else {
                continue;
            };
            if track.is_stopped() || !track.source.video_ready() || !codec.h264 {
                slot.video_send.clear();
                continue;
            }
            let Some(writer) = self.rtc.writer(mid) else {
                continue;
            };
            let Some(pt) = writer
                .payload_params()
                .find(|params| {
                    params.pt() == codec.payload_type.into() && params.spec().codec == Codec::H264
                })
                .map(|params| params.pt())
            else {
                continue;
            };
            let (encoded, due, dropped) = slot.video_send.tick(track, self.epoch, now)?;
            deadline = Some(deadline.map_or(due, |value: Instant| value.min(due)));
            peer.metrics(|metrics| metrics.capture_frames_dropped += dropped);
            if let Some(encoded) = encoded {
                let length = encoded.bytes.len();
                writer
                    .write(
                        pt,
                        now,
                        MediaTime::from_millis(encoded.timestamp_ms),
                        encoded.bytes,
                    )
                    .map_err(|error| format!("OperationError: {error}"))?;
                peer.metrics(|metrics| {
                    metrics.frames_encoded += 1;
                    metrics.keyframes_sent += 1;
                    metrics.video_access_unit_bytes += length as u64;
                    metrics.video_width = encoded.width;
                    metrics.video_height = encoded.height;
                    metrics.video_profile = Some(encode::PROFILE);
                });
            }
        }
        Ok(deadline)
    }

    fn is_video_rtp(&self, bytes: &[u8]) -> bool {
        bytes.len() >= 12
            && bytes[0] & 0xc0 == 0x80
            && !(192..=223).contains(&bytes[1])
            && self.slots.iter().any(|slot| {
                slot.kind == "video"
                    && slot
                        .codecs
                        .iter()
                        .any(|codec| codec.h264 && codec.payload_type == bytes[1] & 0x7f)
            })
    }

    fn event(&mut self, event: Event, peer: &Peer) -> Result<(), String> {
        match event {
            Event::Connected => {
                if self.rtc.direct_api().dtls_protocol_version() != Some(ProtocolVersion::DTLS1_2) {
                    return Err("OperationError: unexpected negotiated DTLS version".into());
                }
                self.connected = true;
                let fingerprint = self
                    .rtc
                    .direct_api()
                    .remote_dtls_fingerprint()
                    .map(ToString::to_string);
                peer.metrics(|metrics| {
                    metrics.dtls_connected = true;
                    metrics.dtls_version = Some("DTLS1.2");
                    metrics.remote_fingerprint = fingerprint;
                });
                peer.post(json!({"type": "connection", "state": "connected"}));
            }
            Event::IceConnectionStateChange(state) => {
                let state = format!("{state:?}").to_ascii_lowercase();
                peer.post(json!({"type": "ice", "state": state}));
                if state == "disconnected" {
                    peer.post(json!({"type": "connection", "state": "disconnected"}));
                } else if self.connected && matches!(state.as_str(), "connected" | "completed") {
                    peer.post(json!({"type": "connection", "state": "connected"}));
                }
            }
            Event::MediaData(data) if data.params.spec().codec == Codec::H264 => {
                let Some(slot) = self
                    .slots
                    .iter_mut()
                    .find(|slot| slot.mid == Some(data.mid) && slot.kind == "video")
                else {
                    return Ok(());
                };
                if !slot.current.as_deref().is_some_and(sdp::receives)
                    || !slot
                        .codecs
                        .iter()
                        .any(|codec| codec.h264 && data.params.pt() == codec.payload_type.into())
                {
                    return Ok(());
                }
                let Some(receiver) = &slot.receiver else {
                    return Ok(());
                };
                if receiver.is_stopped() {
                    return Ok(());
                }
                let Some(video) = receiver.source.remote_video() else {
                    return Ok(());
                };
                let timestamp = data
                    .time
                    .rebase(str0m::media::Frequency::NINETY_KHZ)
                    .numer();
                let decoded = match slot.video_receive.decode(
                    &data.data,
                    receiver.source.id,
                    timestamp,
                    self.cancellation.generation(),
                ) {
                    Ok(decoded) => decoded,
                    Err(error) => {
                        eprintln!("{error}");
                        slot.video_receive = receive::Receiver::new();
                        peer.metrics(|metrics| metrics.video_decode_errors += 1);
                        return Ok(());
                    }
                };
                for frame in decoded.frames {
                    let width = frame.width;
                    let height = frame.height;
                    video
                        .publish(frame)
                        .map_err(|error| format!("{}: {}", error.name, error.message))?;
                    receiver.source.muted.store(false, Ordering::Release);
                    let recovered = decoded.keyframe
                        && self.video_recovery
                        && self.video_lost.is_some_and(|(lost, _)| {
                            let distance = (timestamp as u32).wrapping_sub(lost);
                            distance > 0 && distance < 0x8000_0000
                        });
                    if recovered {
                        self.video_recovery = false;
                    }
                    peer.metrics(|metrics| {
                        metrics.frames_decoded += 1;
                        metrics.keyframes_decoded += u64::from(decoded.keyframe);
                        metrics.video_recoveries += u64::from(recovered);
                        metrics.last_video_timestamp = Some(timestamp as u32);
                        metrics.video_width = width;
                        metrics.video_height = height;
                        metrics.video_profile = Some(encode::PROFILE);
                    });
                }
            }
            Event::MediaData(data)
                if matches!(data.params.spec().codec, Codec::Opus | Codec::PCMU) =>
            {
                let Some(slot) = self
                    .slots
                    .iter_mut()
                    .find(|slot| slot.mid == Some(data.mid))
                else {
                    return Ok(());
                };
                if !slot.current.as_deref().is_some_and(sdp::receives) {
                    return Ok(());
                }
                let Some(codec) = slot.codecs.iter().find(|codec| {
                    data.params.pt() == codec.payload_type.into()
                        && codec.opus == (data.params.spec().codec == Codec::Opus)
                }) else {
                    return Ok(());
                };
                let (Some(output), Some(receiver)) = (&slot.output, &slot.receiver) else {
                    return Ok(());
                };
                if receiver.is_stopped() {
                    return Ok(());
                }
                peer.metrics(|metrics| metrics.rx_codec = Some(codec.name()));
                if codec.opus {
                    if slot.receive.is_none() {
                        slot.receive = Some(Box::new(opus::Receiver::new(true, slot.rx_next)?));
                    }
                    let receive = slot
                        .receive
                        .as_mut()
                        .ok_or("OperationError: missing Opus receiver")?;
                    let timestamp = data
                        .time
                        .rebase(str0m::media::Frequency::FORTY_EIGHT_KHZ)
                        .numer();
                    if let Err(error) = receive.enqueue(timestamp, data.data) {
                        eprintln!(
                            "WebRTC Opus enqueue failed at RTP timestamp {timestamp}: {error}"
                        );
                        peer.metrics(|metrics| metrics.codec_errors += 1);
                    }
                } else {
                    if data.data.is_empty() || data.data.len() > 2000 {
                        peer.metrics(|metrics| metrics.codec_errors += 1);
                        return Ok(());
                    }
                    let pcm = Pcm::new(
                        receiver.source.id,
                        8000,
                        1,
                        0,
                        self.cancellation.generation(),
                        pcmu::decode(&data.data),
                    )
                    .map_err(|error| format!("{}: {}", error.name, error.message))?;
                    let samples = crate::audio::pcm::resample_block(&pcm, opus::RATE, 2);
                    let position = ((data.network_time.saturating_duration_since(self.epoch)
                        + opus::PLAYOUT)
                        .as_secs_f64()
                        * f64::from(opus::RATE)) as u64;
                    let position = position.max(slot.rx_next);
                    let mut sink = Sink {
                        output,
                        receiver,
                        next: &mut slot.rx_next,
                        silence_run: &mut slot.silence_run,
                        generation: self.cancellation.generation(),
                        peer,
                    };
                    sink.push(opus::Decoded {
                        position,
                        samples,
                        concealed_frames: 0,
                        fec_attempts: 0,
                        errors: 0,
                    })?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

fn dtls(bytes: &[u8]) -> bool {
    bytes.first().is_some_and(|byte| (20..64).contains(byte))
}

enum Wake {
    Commands,
    Sent,
    Received(usize, usize, nagoya::reactor::Addr),
    Timeout,
    Closed,
}

async fn drive(peer: &Peer, config: Config, epoch: Instant) -> Result<(), String> {
    let certificate = super::crypto::provider()
        .dtls_provider
        .generate_certificate()
        .ok_or("OperationError: ephemeral DTLS certificate generation failed")?;
    let (sockets, candidates) = gather::gather(config, peer, &certificate.certificate).await?;
    let mut rtc = rtc(certificate.clone());
    for candidate in &candidates {
        rtc.add_local_candidate(candidate.clone());
    }
    let mut engine = Engine {
        rtc,
        certificate,
        candidates,
        slots: Vec::new(),
        pending: None,
        generated_offer: None,
        generated_answer: None,
        pending_local: None,
        pending_remote: None,
        current_local: None,
        current_remote: None,
        signaling: "stable",
        restart: false,
        connected: false,
        epoch,
        audio_deadline: Instant::now(),
        audio_drop: 0,
        video_drop: false,
        video_lost: None,
        video_recovery: false,
        cancellation: Cancellation::default(),
    };
    let mut transmits: VecDeque<Transmit> = VecDeque::new();
    // The largest valid UDP payload fits. Truncation cannot create a valid
    // authenticated DTLS/SRTP packet from a larger IP datagram.
    let mut buffer = vec![0; 65_535];
    let mut receive_cursor = 0;

    loop {
        if peer.closed() {
            engine.cancellation.cancel();
            engine.rtc.disconnect();
            return Ok(());
        }
        for _ in 0..16 {
            let Some(job) = peer.next() else {
                break;
            };
            let token = job.token;
            let result = engine.command(job, peer);
            peer.reply(token, result);
            if peer.closed() {
                break;
            }
        }
        if peer.closed() {
            continue;
        }
        let now = Instant::now();
        engine.audio(peer, now)?;
        let video_deadline = engine.video(peer, now)?;
        engine
            .rtc
            .handle_input(Input::Timeout(now))
            .map_err(|error| format!("OperationError: {error}"))?;

        let mut protocol_deadline = None;
        for _ in 0..OUTPUTS {
            match engine
                .rtc
                .poll_output()
                .map_err(|error| format!("OperationError: {error}"))?
            {
                Output::Timeout(deadline) => {
                    protocol_deadline = Some(deadline);
                    break;
                }
                Output::Transmit(packet) => {
                    if packet.proto != Protocol::Udp || transmits.len() == TRANSMITS {
                        return Err(
                            "OperationError: unsupported transport or transmit backlog".into()
                        );
                    }
                    transmits.push_back(packet);
                }
                Output::Event(event) => engine.event(event, peer)?,
            }
        }
        // Ingest available MediaData before deciding a packet missed its
        // decode deadline. Receive playout has its own reactive wakeup and
        // must not wait for the next capture tick.
        let playout_deadline = engine.playout(peer, Instant::now())?;
        let Some(mut deadline) = protocol_deadline else {
            nagoya::yield_now().await;
            continue;
        };
        if engine.connected {
            if engine.signaling == "stable" {
                deadline = deadline.min(engine.audio_deadline);
                if let Some(video_deadline) = video_deadline {
                    deadline = deadline.min(video_deadline);
                }
            }
            if let Some(playout_deadline) = playout_deadline {
                deadline = deadline.min(playout_deadline);
            }
        }
        let mut sleep = Box::pin(nagoya::sleep(
            deadline.saturating_duration_since(Instant::now()),
        ));
        let wake = poll_fn(|context| {
            if !peer.register(context.waker()) {
                return Poll::Ready(Ok(Wake::Closed));
            }
            if peer.has_jobs() {
                return Poll::Ready(Ok(Wake::Commands));
            }
            if let Some(packet) = transmits.front() {
                let Some(Socket { socket, .. }) =
                    sockets.iter().find(|socket| socket.local == packet.source)
                else {
                    return Poll::Ready(Err(
                        "OperationError: str0m selected an unowned UDP source".into(),
                    ));
                };
                match socket.poll_send_to(
                    context,
                    &packet.contents,
                    gather::address(packet.destination),
                ) {
                    Poll::Ready(Ok(length)) if length == packet.contents.len() => {
                        let video = engine.is_video_rtp(&packet.contents);
                        peer.metrics(|metrics| {
                            metrics.udp_sent += 1;
                            metrics.bytes_sent += length as u64;
                            metrics.dtls_sent += u64::from(dtls(&packet.contents));
                            if video {
                                metrics.video_packets_sent += 1;
                                metrics.video_bytes_sent += length as u64;
                                metrics.video_max_datagram_bytes =
                                    metrics.video_max_datagram_bytes.max(length);
                            }
                        });
                        transmits.pop_front();
                        return Poll::Ready(Ok(Wake::Sent));
                    }
                    Poll::Ready(Ok(_)) => {
                        return Poll::Ready(
                            Err("OperationError: partial UDP datagram send".into()),
                        );
                    }
                    Poll::Ready(Err(error)) => {
                        return Poll::Ready(Err(format!(
                            "OperationError: UDP send failed: {error}"
                        )));
                    }
                    Poll::Pending => {}
                }
            }
            for offset in 0..sockets.len() {
                let index = (receive_cursor + offset) % sockets.len();
                match sockets[index].socket.poll_recv_from(context, &mut buffer) {
                    Poll::Ready(Ok((length, source))) => {
                        receive_cursor = (index + 1) % sockets.len();
                        return Poll::Ready(Ok(Wake::Received(index, length, source)));
                    }
                    Poll::Ready(Err(error)) => {
                        return Poll::Ready(Err(format!(
                            "OperationError: UDP receive failed: {error}"
                        )));
                    }
                    Poll::Pending => {}
                }
            }
            if sleep.as_mut().poll(context).is_ready() {
                return Poll::Ready(Ok(Wake::Timeout));
            }
            Poll::Pending
        })
        .await?;
        match wake {
            Wake::Closed => {
                engine.cancellation.cancel();
                engine.rtc.disconnect();
                return Ok(());
            }
            Wake::Received(index, length, source) => {
                peer.metrics(|metrics| {
                    metrics.udp_received += 1;
                    metrics.bytes_received += length as u64;
                    metrics.dtls_received += u64::from(dtls(&buffer[..length]));
                });
                // Video loss removes one entire timestamp before SRTP.
                // Repeated packets with that timestamp remain suppressed for
                // two seconds; recovery must use a newer keyframe.
                let video = engine.is_video_rtp(&buffer[..length]);
                if video && engine.connected {
                    let timestamp =
                        u32::from_be_bytes([buffer[4], buffer[5], buffer[6], buffer[7]]);
                    let now = Instant::now();
                    if engine.video_drop {
                        engine.video_drop = false;
                        engine.video_lost = Some((timestamp, now + Duration::from_secs(2)));
                        engine.video_recovery = true;
                        peer.metrics(|metrics| {
                            metrics.video_lost_timestamp = Some(timestamp);
                        });
                    }
                    if engine
                        .video_lost
                        .is_some_and(|(lost, until)| lost == timestamp && now < until)
                    {
                        peer.metrics(|metrics| metrics.video_dropped += 1);
                        continue;
                    }
                }
                // Fixture loss occurs before SRTP ingestion. STUN, DTLS and
                // RTCP remain intact, so the test exercises actual RTP gaps.
                if engine.audio_drop > 0
                    && !video
                    && engine.connected
                    && length >= 12
                    && buffer[0] & 0xc0 == 0x80
                    && !(192..=223).contains(&buffer[1])
                {
                    engine.audio_drop -= 1;
                    peer.metrics(|metrics| metrics.audio_dropped += 1);
                    continue;
                }
                if let Ok(receive) = Receive::new(
                    Protocol::Udp,
                    gather::socket_address(source)?,
                    sockets[index].local,
                    &buffer[..length],
                ) {
                    let input = Input::Receive(Instant::now(), receive);
                    if engine.rtc.accepts(&input) {
                        engine
                            .rtc
                            .handle_input(input)
                            .map_err(|error| format!("OperationError: {error}"))?;
                    }
                }
            }
            Wake::Commands | Wake::Sent | Wake::Timeout => {}
        }
        nagoya::yield_now().await;
    }
}

pub(super) async fn run(peer: Arc<Peer>, config: Config, epoch: Instant) {
    if let Err(error) = drive(&peer, config, epoch).await
        && !peer.closed()
    {
        peer.post(json!({"type": "fatal", "error": error}));
    }
    // drive has returned and dropped every descriptor before these counters
    // become observable as released. Capture tracks are leases, not stopped.
    peer.close();
    peer.metrics(|metrics| {
        metrics.sockets = 0;
        metrics.running = false;
    });
}
