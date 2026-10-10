//! One task owns one Rtc, its sockets, pacing and protocol deadlines.
//!
//! The PCMU path is a bounded bootstrap audio path. Adaptive playout, Opus,
//! video codecs and RTP statistics remain separate media waves.

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
use super::pcmu;
use super::sdp::{self, Description, Section};

const AUDIO_QUANTUM: Duration = Duration::from_millis(20);
const OUTPUTS: usize = 128;
const TRANSMITS: usize = 256;

struct Slot {
    id: u64,
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
    cancellation: Cancellation,
}

fn rtc(certificate: DtlsCert) -> Rtc {
    super::crypto::config()
        .set_dtls_cert(certificate)
        .clear_codecs()
        .enable_pcmu(true, false)
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
                    "mid": slot.mid.map(|mid| mid.to_string()),
                    "direction": slot.direction,
                    "currentDirection": slot.current,
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
                    .any(|(old, new)| old.mid != new.mid)
            {
                return Err("OperationError: established MID identity and m-line order are immutable".into());
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
            return Err("OperationError: add an audio transceiver before creating an offer".into());
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
                slot.mid = Some(changes.add_media(
                    MediaKind::Audio,
                    direction,
                    slot.stream.clone(),
                    slot.track
                        .as_ref()
                        .map(|track| format!("chuzz-track-{}", track.id)),
                    None,
                ));
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
        let description = Description {
            r#type: "offer".into(),
            sdp: offer.to_sdp_string(),
        };
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
            let remote = slot.remote.as_ref().ok_or("OperationError: missing remote slot")?;
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
        let description = Description {
            r#type: "answer".into(),
            sdp: sdp::answer(&generated.sdp, &entries),
        };
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
                    return Err("OperationError: local offer must be the current generated offer".into());
                }
                self.pending_local = Some(value.clone());
                self.signaling = "have-local-offer";
            }
            "answer" if self.signaling == "have-remote-offer" => {
                if self.generated_answer.as_ref() != Some(&value) {
                    return Err("OperationError: local answer must be the current generated answer".into());
                }
                self.current_local = Some(value.clone());
                self.current_remote = self.pending_remote.take();
                self.pending_local = None;
                self.signaling = "stable";
                for slot in &mut self.slots {
                    if let Some(remote) = &slot.remote {
                        slot.current = Some(
                            sdp::intersect(&slot.direction, &remote.direction).into(),
                        );
                    }
                }
            }
            _ => return Err("InvalidStateError: invalid local-description transition".into()),
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
                        .find(|slot| {
                            slot.mid.is_some_and(|mid| mid.to_string() == section.mid)
                        })
                        .or_else(|| {
                            self.slots
                                .iter()
                                .find(|slot| slot.mid.is_none() && !used.contains(&slot.id))
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
                        self.slots.push(Slot {
                            id,
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
                    self.pending_local.as_ref().ok_or("OperationError: missing local offer")?,
                )?;
                if sections.len() != offered.len()
                    || sections.iter().zip(&offered).any(|(answer, offer)| {
                        answer.mid != offer.mid
                            || (sdp::sends(&answer.direction)
                                && !sdp::receives(&offer.direction))
                            || (sdp::receives(&answer.direction)
                                && !sdp::sends(&offer.direction))
                    })
                {
                    return Err("OperationError: answer does not match the offered slot table".into());
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
                        .find(|slot| {
                            slot.mid.is_some_and(|mid| mid.to_string() == section.mid)
                        })
                        .ok_or("OperationError: missing answer slot")?;
                    slot.current = Some(
                        sdp::intersect(&slot.direction, &section.direction).into(),
                    );
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
                self.slots.push(Slot {
                    id,
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
                });
                Ok(Value::Null)
            }
            "bind" => {
                let slot = self.slot(slot_id(&job.data)?)?;
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
                    .ok_or("InvalidStateError: set a remote description before adding candidates")?;
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
                    sections.get(index as usize).is_none_or(|indexed| indexed.mid != section.mid)
                }) || value["usernameFragment"]
                    .as_str()
                    .is_some_and(|ufrag| ufrag != section.ufrag)
                {
                    return Err("OperationError: candidate belongs to another ICE generation or MID".into());
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

    fn audio(&mut self, peer: &Peer, now: Instant) -> Result<(), String> {
        if !self.connected || self.signaling != "stable" || now < self.audio_deadline {
            return Ok(());
        }
        // Skip capture debt instead of producing a burst of late packets.
        self.audio_deadline = now + AUDIO_QUANTUM;
        let start = now.saturating_duration_since(self.epoch);
        for slot in &self.slots {
            if !slot.active
                || !slot.current.as_deref().is_some_and(sdp::sends)
                || slot.track.as_ref().is_none_or(|track| track.is_stopped())
            {
                continue;
            }
            let (Some(input), Some(mid)) = (&slot.input, slot.mid) else {
                continue;
            };
            let samples = input.read(start, 160, 8000, 1);
            let bytes = pcmu::encode(&samples);
            let Some(writer) = self.rtc.writer(mid) else {
                continue;
            };
            let Some(pt) = writer
                .payload_params()
                .find(|params| params.spec().codec == Codec::PCMU)
                .map(|params| params.pt())
            else {
                continue;
            };
            writer
                .write(pt, now, MediaTime::from_seconds(start.as_secs_f64()), bytes)
                .map_err(|error| format!("OperationError: {error}"))?;
            peer.metrics(|metrics| metrics.samples_sent += 160);
        }
        Ok(())
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
            Event::MediaData(data) if data.params.spec().codec == Codec::PCMU => {
                if data.data.is_empty() || data.data.len() > 12_000 {
                    return Err("OperationError: PCMU receive block exceeds media bounds".into());
                }
                let Some(slot) = self.slots.iter_mut().find(|slot| slot.mid == Some(data.mid)) else {
                    return Ok(());
                };
                if !slot.current.as_deref().is_some_and(sdp::receives) {
                    return Ok(());
                }
                let (Some(output), Some(receiver)) = (&slot.output, &slot.receiver) else {
                    return Ok(());
                };
                let samples = pcmu::decode(&data.data);
                let position = (data
                    .network_time
                    .saturating_duration_since(self.epoch)
                    .as_secs_f64()
                    * 8000.0) as u64;
                let position = position.max(slot.rx_next);
                let pcm = Arc::new(
                    Pcm::new(
                        receiver.source.id,
                        8000,
                        1,
                        position,
                        self.cancellation.generation(),
                        samples,
                    )
                    .map_err(|error| format!("{}: {}", error.name, error.message))?,
                );
                slot.rx_next = position + pcm.frames() as u64;
                let frames = pcm.frames() as u64;
                let energy = pcm.energy();
                output.push(pcm)?;
                receiver.source.muted.store(false, Ordering::Release);
                peer.metrics(|metrics| {
                    metrics.samples_received += frames;
                    metrics.received_energy += energy;
                });
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

async fn drive(
    peer: &Peer,
    config: Config,
    epoch: Instant,
) -> Result<(), String> {
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
        engine
            .rtc
            .handle_input(Input::Timeout(now))
            .map_err(|error| format!("OperationError: {error}"))?;

        let mut protocol_deadline = None;
        for _ in 0..OUTPUTS {
            match engine.rtc.poll_output().map_err(|error| format!("OperationError: {error}"))? {
                Output::Timeout(deadline) => {
                    protocol_deadline = Some(deadline);
                    break;
                }
                Output::Transmit(packet) => {
                    if packet.proto != Protocol::Udp || transmits.len() == TRANSMITS {
                        return Err("OperationError: unsupported transport or transmit backlog".into());
                    }
                    transmits.push_back(packet);
                }
                Output::Event(event) => engine.event(event, peer)?,
            }
        }
        let Some(mut deadline) = protocol_deadline else {
            nagoya::yield_now().await;
            continue;
        };
        if engine.connected && engine.signaling == "stable" {
            deadline = deadline.min(engine.audio_deadline);
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
                    return Poll::Ready(Err("OperationError: str0m selected an unowned UDP source".into()));
                };
                match socket.poll_send_to(
                    context,
                    &packet.contents,
                    gather::address(packet.destination),
                ) {
                    Poll::Ready(Ok(length)) if length == packet.contents.len() => {
                        peer.metrics(|metrics| {
                            metrics.udp_sent += 1;
                            metrics.bytes_sent += length as u64;
                            metrics.dtls_sent += u64::from(dtls(&packet.contents));
                        });
                        transmits.pop_front();
                        return Poll::Ready(Ok(Wake::Sent));
                    }
                    Poll::Ready(Ok(_)) => {
                        return Poll::Ready(Err("OperationError: partial UDP datagram send".into()));
                    }
                    Poll::Ready(Err(error)) => {
                        return Poll::Ready(Err(format!("OperationError: UDP send failed: {error}")));
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
                        return Poll::Ready(Err(format!("OperationError: UDP receive failed: {error}")));
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

