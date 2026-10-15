//! Document-thread bridge. Boa values never enter a transport task.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::task::Waker;
use std::time::Instant;

use serde::Serialize;
use serde_json::{Value, json};

use crate::audio::pcm::Track as AudioTrack;
use crate::media::source::remote::Video;
use crate::media::track::{Kind, Track};

use super::{gather, sdp, transport};

const PEERS: usize = 16;
const COMMANDS: usize = 64;
const EVENTS: usize = 4096;
const MAX_ID: u64 = 9_007_199_254_740_991;

#[path = "host_script.rs"]
mod script;
pub(super) use script::install;

pub(super) fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|error| error.into_inner())
}

pub(super) trait Resources {
    fn media_epoch(&self) -> Result<Instant, String>;
    fn media(&self) -> Result<Arc<crate::media::Host>, String>;
    fn register_audio(&self, track: Arc<crate::audio::pcm::Track>) -> Result<u64, String>;
    fn audio(&self, id: u64) -> Result<Arc<crate::audio::pcm::Track>, String>;
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Metrics {
    pub running: bool,
    pub sockets: usize,
    pub udp_sent: u64,
    pub udp_received: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub dtls_sent: u64,
    pub dtls_received: u64,
    pub dtls_connected: bool,
    pub dtls_version: Option<&'static str>,
    pub remote_fingerprint: Option<String>,
    pub samples_sent: u64,
    pub samples_received: u64,
    pub received_energy: f64,
    pub tx_codec: Option<&'static str>,
    pub rx_codec: Option<&'static str>,
    pub opus_packets_sent: u64,
    pub opus_bytes_sent: u64,
    pub opus_min_packet_bytes: Option<usize>,
    pub opus_max_packet_bytes: Option<usize>,
    pub audio_dropped: u64,
    pub concealed_frames: u64,
    pub plc_frames: u64,
    pub fec_attempts: u64,
    pub codec_errors: u64,
    pub max_silence_frames: u64,
    pub max_playout_gap_frames: u64,
    pub frames_encoded: u64,
    pub frames_decoded: u64,
    pub keyframes_sent: u64,
    pub keyframes_decoded: u64,
    pub video_bytes_sent: u64,
    pub video_packets_sent: u64,
    pub video_max_datagram_bytes: usize,
    pub video_access_unit_bytes: u64,
    pub capture_frames_dropped: u64,
    pub video_dropped: u64,
    pub video_decode_errors: u64,
    pub video_recoveries: u64,
    pub video_lost_timestamp: Option<u32>,
    pub last_video_timestamp: Option<u32>,
    pub video_width: u32,
    pub video_height: u32,
    pub video_profile: Option<&'static str>,
}

pub(super) struct Job {
    pub data: Value,
    pub token: u64,
    pub track: Option<Arc<Track>>,
    pub input: Option<Arc<AudioTrack>>,
    pub output: Option<Arc<AudioTrack>>,
    pub receiver: Option<Arc<Track>>,
}

#[derive(Default)]
struct Inbox {
    jobs: VecDeque<Job>,
    reader: Option<Waker>,
}

pub(super) struct Peer {
    pub id: u64,
    page: Weak<Page>,
    closed: AtomicBool,
    inbox: Mutex<Inbox>,
    outputs: Mutex<Vec<Arc<AudioTrack>>>,
    videos: Mutex<Vec<Arc<Video>>>,
    metrics: Mutex<Metrics>,
}

impl Peer {
    pub(super) fn closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    pub(super) fn metrics(&self, change: impl FnOnce(&mut Metrics)) {
        change(&mut lock(&self.metrics));
    }

    pub(super) fn register(&self, reader: &Waker) -> bool {
        let mut inbox = lock(&self.inbox);
        if self.closed() {
            return false;
        }
        inbox.reader = Some(reader.clone());
        true
    }

    pub(super) fn next(&self) -> Option<Job> {
        lock(&self.inbox).jobs.pop_front()
    }

    pub(super) fn has_jobs(&self) -> bool {
        !lock(&self.inbox).jobs.is_empty()
    }

    fn enqueue(&self, job: Job) -> Result<(), String> {
        let reader = {
            let mut inbox = lock(&self.inbox);
            if self.closed() {
                return Err("InvalidStateError: peer is closed".into());
            }
            if inbox.jobs.len() == COMMANDS {
                return Err("OperationError: peer operation queue is full".into());
            }
            inbox.jobs.push_back(job);
            inbox.reader.take()
        };
        if let Some(reader) = reader {
            reader.wake();
        }
        Ok(())
    }

    pub(super) fn close(&self) {
        let reader = {
            let mut inbox = lock(&self.inbox);
            self.closed.store(true, Ordering::Release);
            inbox.jobs.clear();
            inbox.reader.take()
        };
        for output in lock(&self.outputs).drain(..) {
            output.end_source();
        }
        for video in lock(&self.videos).drain(..) {
            video.end();
        }
        if let Some(reader) = reader {
            reader.wake();
        }
    }

    pub(super) fn post(&self, event: Value) {
        if let Some(page) = self.page.upgrade()
            && !self.closed()
        {
            page.post(self.id, event);
        }
    }

    pub(super) fn reply(&self, token: u64, result: Result<Value, String>) {
        if token == 0 {
            if let Err(error) = result {
                self.post(json!({"type": "fatal", "error": error}));
                self.close();
            }
            return;
        }
        self.post(match result {
            Ok(value) => json!({"type": "reply", "token": token, "value": value}),
            Err(error) => json!({"type": "reply", "token": token, "error": error}),
        });
    }
}

#[derive(Default)]
pub(super) struct State {
    next: u64,
    peers: BTreeMap<u64, Arc<Peer>>,
    pub(super) events: VecDeque<Value>,
    pub(super) reader: Option<Waker>,
}

pub(super) struct Page {
    pub(super) state: Mutex<State>,
}

impl Page {
    fn post(&self, peer: u64, event: Value) {
        let reader = {
            let mut state = lock(&self.state);
            if state.events.len() >= EVENTS {
                state
                    .events
                    .retain(|event| event["peer"].as_u64() != Some(peer));
                state.events.push_back(json!({
                    "peer": peer,
                    "event": {"type": "fatal", "error": "OperationError: peer event queue overflow"}
                }));
                if let Some(peer) = state.peers.get(&peer) {
                    peer.close();
                }
            } else {
                state
                    .events
                    .push_back(json!({"peer": peer, "event": event}));
            }
            state.reader.take()
        };
        if let Some(reader) = reader {
            reader.wake();
        }
    }

    fn peer(&self, id: u64) -> Result<Arc<Peer>, String> {
        lock(&self.state)
            .peers
            .get(&id)
            .cloned()
            .ok_or_else(|| "InvalidStateError: unknown peer connection".into())
    }

    pub(super) fn close(&self) {
        let mut state = lock(&self.state);
        for peer in state.peers.values() {
            peer.close();
        }
        state.events.clear();
    }
}

pub(super) struct NativeHost(pub Arc<Page>);

impl Drop for NativeHost {
    fn drop(&mut self) {
        self.0.close();
    }
}

fn integer(data: &Value, key: &str) -> Result<u64, String> {
    data.get(key)
        .and_then(Value::as_u64)
        .filter(|value| *value <= MAX_ID)
        .ok_or_else(|| format!("TypeError: invalid {key} identifier"))
}

pub(super) fn execute(
    page: &Arc<Page>,
    data: Value,
    resources: &dyn Resources,
) -> Result<Value, String> {
    let op = data["op"]
        .as_str()
        .ok_or("TypeError: a peer operation is required")?;
    if op == "create" {
        let config = gather::Config::parse(&data["configuration"])?;
        let mut state = lock(&page.state);
        state
            .peers
            .retain(|_, peer| !peer.closed() || lock(&peer.metrics).running);
        if state.peers.len() >= PEERS || state.next == MAX_ID {
            return Err("OperationError: page peer-connection limit reached".into());
        }
        state.next += 1;
        let peer = Arc::new(Peer {
            id: state.next,
            page: Arc::downgrade(page),
            closed: AtomicBool::new(false),
            inbox: Mutex::new(Inbox::default()),
            outputs: Mutex::new(Vec::new()),
            videos: Mutex::new(Vec::new()),
            metrics: Mutex::new(Metrics {
                running: true,
                ..Metrics::default()
            }),
        });
        let id = peer.id;
        state.peers.insert(id, Arc::clone(&peer));
        drop(state);
        let epoch = resources.media_epoch()?;
        drop(nagoya::spawn(transport::run(peer, config, epoch)));
        return Ok(json!(id));
    }

    let peer = page.peer(integer(&data, "peer")?)?;
    if op == "inspect" {
        return serde_json::to_value(&*lock(&peer.metrics)).map_err(|error| error.to_string());
    }
    if op == "close" {
        peer.close();
        return Ok(Value::Null);
    }
    if peer.closed() {
        return Err("InvalidStateError: peer is closed".into());
    }
    if op == "receiver" {
        if data["kind"] == "video" {
            let mut videos = lock(&peer.videos);
            if videos.len() >= sdp::SLOT_LIMIT {
                return Err("OperationError: receiver limit reached".into());
            }
            let video = Arc::new(Video::default());
            let descriptor = resources
                .media()?
                .video_receiver(Arc::clone(&video))
                .map_err(|error| format!("{}: {}", error.name, error.message))?;
            videos.push(video);
            return Ok(json!({"video": descriptor}));
        }
        if data["kind"].as_str().is_some_and(|kind| kind != "audio") {
            return Err("TypeError: invalid receiver kind".into());
        }
        let mut outputs = lock(&peer.outputs);
        if outputs.len() >= sdp::SLOT_LIMIT {
            return Err("OperationError: receiver limit reached".into());
        }
        let output = AudioTrack::new(48_000, 2)?;
        let id = resources.register_audio(Arc::clone(&output))?;
        outputs.push(output);
        return Ok(json!({"track": id}));
    }

    let token = data
        .get("token")
        .map_or(Ok(0), |_| integer(&data, "token"))?;
    let mut track = None;
    let mut input = None;
    let mut output = None;
    let mut receiver = None;
    let mut updates = Vec::new();

    if matches!(op, "add" | "replace") {
        integer(&data, "slot")?;
        if let Some(handle) = data["handle"].as_u64() {
            let leased = resources
                .media()?
                .track(handle)
                .map_err(|error| format!("{}: {}", error.name, error.message))?;
            if leased.is_stopped() {
                return Err("InvalidAccessError: a live media track is required".into());
            }
            if leased.source.kind == Kind::Audio {
                input = Some(AudioTrack::from_media(&leased)?);
            }
            track = Some(leased);
        }
    }
    if matches!(op, "add" | "direction") {
        sdp::direction(
            data["direction"]
                .as_str()
                .ok_or("TypeError: a direction is required")?,
        )?;
    }
    if matches!(op, "add" | "bind") {
        integer(&data, "slot")?;
        let media = resources.media()?;
        let leased = media
            .track(integer(&data, "receiverHandle")?)
            .map_err(|error| format!("{}: {}", error.name, error.message))?;
        if leased.source.kind == Kind::Video {
            let Some(video) = leased.source.remote_video() else {
                return Err("InvalidAccessError: receiver is not remote video".into());
            };
            if !lock(&peer.videos)
                .iter()
                .any(|owned| Arc::ptr_eq(owned, video))
            {
                return Err("InvalidAccessError: video receiver belongs to another peer".into());
            }
        } else {
            let audio = resources.audio(integer(&data, "audio")?)?;
            if !leased
                .source
                .audio()
                .is_some_and(|source| Arc::ptr_eq(source, &audio))
                || !lock(&peer.outputs)
                    .iter()
                    .any(|owned| Arc::ptr_eq(owned, &audio))
            {
                return Err("InvalidAccessError: receiver PCM registration does not match".into());
            }
            output = Some(audio);
        }
        if op == "add"
            && (data["kind"].as_str() != Some(leased.source.kind.name())
                || track
                    .as_ref()
                    .is_some_and(|track| track.source.kind != leased.source.kind))
        {
            return Err("InvalidAccessError: transceiver media kinds do not match".into());
        }
        leased.source.muted.store(true, Ordering::Release);
        updates.push(
            leased
                .descriptor()
                .map_err(|error| format!("{}: {}", error.name, error.message))?,
        );
        receiver = Some(leased);
    }

    peer.enqueue(Job {
        data,
        token,
        track,
        input,
        output,
        receiver,
    })?;
    Ok(json!({"tracks": updates}))
}
