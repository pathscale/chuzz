//! Page-worker capture state and metadata bridge.
//!
//! Fake devices are the default capture policy. They never open OS devices.
//! Rust consumers lease Track, VideoFrame and Pcm directly through Arc.
//! The JS bridge carries descriptors and measured diagnostics only.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use blitz_script::ScriptDocument;
use blitz_script::boa_engine::{
    Context, JsNativeError, JsResult, JsString, JsValue, NativeFunction,
};
use serde_json::{Value, json};

pub mod fake;
pub mod frame;
pub mod pcm;
pub mod source;
pub mod track;

use frame::VideoPixels;
use source::Source;
use track::{Kind, Settings, Track};

const TRACK_LIMIT: usize = 128;
const COMMAND_LIMIT: usize = 64 * 1024;
const MAX_ID: u64 = 9_007_199_254_740_991;
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, serde::Serialize)]
pub struct Error {
    pub name: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub constraint: Option<String>,
}

impl Error {
    pub fn state(message: &str) -> Self {
        Self::named("InvalidStateError", message)
    }

    pub fn typ(message: &str) -> Self {
        Self::named("TypeError", message)
    }

    pub fn named(name: &'static str, message: &str) -> Self {
        Self {
            name,
            message: message.into(),
            constraint: None,
        }
    }

    pub fn constraint(key: &str) -> Self {
        Self {
            name: "OverconstrainedError",
            message: format!("the fake device cannot satisfy {key}"),
            constraint: Some(key.into()),
        }
    }
}

pub(super) fn allocate() -> Result<u64, Error> {
    NEXT_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            (value < MAX_ID).then_some(value + 1)
        })
        .map_err(|_| Error::named("QuotaExceededError", "media handle space exhausted"))
}

struct Store {
    tracks: BTreeMap<u64, Arc<Track>>,
    audio_tracks: HashMap<u64, u64>,
    published: HashMap<u64, (bool, bool, bool)>,
    revision: u64,
    published_revision: u64,
    video: bool,
    audio: bool,
    output: bool,
    video_muted: bool,
    audio_muted: bool,
    video_granted: bool,
    audio_granted: bool,
    closed: bool,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            tracks: BTreeMap::new(),
            audio_tracks: HashMap::new(),
            published: HashMap::new(),
            revision: 0,
            published_revision: 0,
            video: true,
            audio: true,
            output: true,
            video_muted: false,
            audio_muted: false,
            video_granted: false,
            audio_granted: false,
            closed: false,
        }
    }
}

pub struct Host {
    store: Mutex<Store>,
    origin: Instant,
    secure: bool,
}

fn trusted(url: Option<&str>) -> bool {
    let Some(url) = url.and_then(|url| url::Url::parse(url).ok()) else {
        return false;
    };
    if matches!(url.scheme(), "https" | "file") {
        return true;
    }
    if url.scheme() != "http" {
        return false;
    }
    match url.host() {
        Some(url::Host::Domain(host)) => {
            let host = host.trim_end_matches('.');
            host.eq_ignore_ascii_case("localhost")
                || host.to_ascii_lowercase().ends_with(".localhost")
        }
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

fn request_constraints(value: Option<&Value>) -> Result<Option<Value>, Error> {
    match value {
        None | Some(Value::Null) | Some(Value::Bool(false)) => Ok(None),
        Some(Value::Bool(true)) => Ok(Some(json!({}))),
        Some(value) if value.is_object() => Ok(Some(value.clone())),
        _ => Err(Error::typ(
            "audio and video must be booleans or constraint dictionaries",
        )),
    }
}

impl Host {
    pub fn media_epoch(&self) -> Instant {
        self.origin
    }

    pub fn track(&self, id: u64) -> Result<Arc<Track>, Error> {
        let store = self
            .store
            .lock()
            .map_err(|_| Error::state("capture host state was poisoned"))?;
        if store.closed {
            return Err(Error::state("capture document has closed"));
        }
        store
            .tracks
            .get(&id)
            .cloned()
            .ok_or_else(|| Error::state("unknown or ended capture track"))
    }

    pub fn close(&self) {
        if let Ok(mut store) = self.store.lock() {
            store.closed = true;
            for track in store.tracks.values() {
                track.stop();
            }
            store.tracks.clear();
            store.audio_tracks.clear();
            store.published.clear();
        }
    }

    fn control(&self, request: Value, context: &Context) -> Result<Value, Error> {
        let op = request
            .get("op")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::typ("a media command requires an operation"))?;
        let mut store = self
            .store
            .lock()
            .map_err(|_| Error::state("capture host state was poisoned"))?;
        if store.closed {
            return Err(Error::state("capture document has closed"));
        }
        if !self.secure && matches!(op, "enumerate" | "acquire" | "present" | "mute") {
            return Err(Error::named(
                "NotAllowedError",
                "capture requires a secure context",
            ));
        }
        match op {
            "init" => Ok(json!({"secure": self.secure})),
            "id" => Ok(json!(format!("chuzz-media-{}", allocate()?))),
            "audioEnded" => Ok(Value::Null),
            "audioStream" => {
                if store.tracks.len() >= TRACK_LIMIT {
                    return Err(Error::named(
                        "QuotaExceededError",
                        "too many live capture tracks",
                    ));
                }
                let id = request
                    .get("audio")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| Error::typ("a graph PCM track identifier is required"))?;
                let audio = crate::audio::registered_track(context, id)
                    .map_err(|error| Error::state(&error.to_string()))?;
                let settings = Settings::Audio {
                    sample_rate: audio.sample_rate(),
                    channel_count: audio.channels(),
                };
                let source = Arc::new(Source::from_audio(audio, self.origin)?);
                let track = Arc::new(Track::new(allocate()?, source, json!({}), settings));
                let descriptor = track.descriptor()?;
                store.tracks.insert(track.id, track);
                Ok(descriptor)
            }
            "enumerate" => {
                let mut devices = fake::devices(
                    store.video,
                    store.audio,
                    store.output,
                    store.video_granted,
                    store.audio_granted,
                );
                for device in &mut devices {
                    if device["label"] == "" {
                        device
                            .as_object_mut()
                            .ok_or_else(|| Error::state("invalid fake device descriptor"))?
                            .remove("capabilities");
                    }
                }
                Ok(json!(devices))
            }
            "acquire" => {
                let constraints = request
                    .get("constraints")
                    .filter(|value| value.is_object())
                    .ok_or_else(|| Error::typ("getUserMedia requires a constraint dictionary"))?;
                let mut prepared = Vec::new();
                for kind in [Kind::Audio, Kind::Video] {
                    if let Some(constraints) = request_constraints(constraints.get(kind.name()))? {
                        let present = match kind {
                            Kind::Audio => store.audio,
                            Kind::Video => store.video,
                        };
                        if !present {
                            return Err(Error::named(
                                "NotFoundError",
                                "the requested device is absent",
                            ));
                        }
                        let settings = track::resolve(kind, &constraints)?;
                        prepared.push((kind, constraints, settings));
                    }
                }
                if prepared.is_empty() {
                    return Err(Error::typ("getUserMedia requires audio or video"));
                }
                if store.tracks.len() + prepared.len() > TRACK_LIMIT {
                    return Err(Error::named(
                        "QuotaExceededError",
                        "too many live capture tracks",
                    ));
                }
                let mut created = Vec::new();
                let mut descriptors = Vec::new();
                for (kind, constraints, settings) in prepared {
                    let muted = match kind {
                        Kind::Audio => store.audio_muted,
                        Kind::Video => store.video_muted,
                    };
                    let source = Arc::new(Source::new(kind, self.origin, muted)?);
                    let track = Arc::new(Track::new(allocate()?, source, constraints, settings));
                    descriptors.push(track.descriptor()?);
                    created.push(track);
                }
                // Commit both requested kinds only after all validation succeeds.
                for track in created {
                    match track.source.kind {
                        Kind::Audio => store.audio_granted = true,
                        Kind::Video => store.video_granted = true,
                    }
                    store.tracks.insert(track.id, track);
                }
                Ok(json!(descriptors))
            }
            "present" | "mute" => {
                let kind = request
                    .get("kind")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Error::typ("a fake device kind is required"))?;
                let value = request
                    .get("value")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| Error::typ("a fake device state must be boolean"))?;
                if op == "present" {
                    let present = match kind {
                        "video" => &mut store.video,
                        "audio" => &mut store.audio,
                        "audiooutput" => &mut store.output,
                        _ => return Err(Error::typ("unknown fake device kind")),
                    };
                    let changed = *present != value;
                    *present = value;
                    if changed {
                        store.revision = store.revision.wrapping_add(1);
                        if !value {
                            for track in store.tracks.values() {
                                if track.source.is_capture() && track.source.kind.name() == kind {
                                    track.stop();
                                }
                            }
                        }
                    }
                } else {
                    match kind {
                        "video" => store.video_muted = value,
                        "audio" => store.audio_muted = value,
                        _ => return Err(Error::typ("only capture sources can be muted")),
                    }
                    for track in store.tracks.values() {
                        if track.source.is_capture() && track.source.kind.name() == kind {
                            track.source.muted.store(value, Ordering::Release);
                        }
                    }
                }
                Ok(Value::Null)
            }
            _ => {
                let id = request
                    .get("handle")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| Error::typ("a media command requires a track handle"))?;
                let track = store
                    .tracks
                    .get(&id)
                    .cloned()
                    .ok_or_else(|| Error::state("unknown or ended capture track"))?;
                match op {
                    "audio" => {
                        if track.source.kind != Kind::Audio || track.is_stopped() {
                            return Err(Error::state("a live audio track is required"));
                        }
                        if let Some(id) = store.audio_tracks.get(&track.id) {
                            return Ok(json!(id));
                        }
                        let audio = crate::audio::pcm::Track::from_media(&track)
                            .map_err(|error| Error::state(&error))?;
                        let audio_id = crate::audio::register_track(context, audio)
                            .map_err(|error| Error::state(&error.to_string()))?;
                        store.audio_tracks.insert(track.id, audio_id);
                        Ok(json!(audio_id))
                    }
                    "describe" => track.descriptor(),
                    "stop" => {
                        track.stop();
                        store.tracks.remove(&id);
                        store.audio_tracks.remove(&id);
                        store.published.remove(&id);
                        Ok(Value::Null)
                    }
                    "enabled" => {
                        let enabled = request
                            .get("value")
                            .and_then(Value::as_bool)
                            .ok_or_else(|| Error::typ("enabled must be boolean"))?;
                        track.enabled.store(enabled, Ordering::Release);
                        Ok(Value::Null)
                    }
                    "apply" => {
                        let constraints = request.get("constraints").cloned().unwrap_or(json!({}));
                        track.apply(constraints)?;
                        track.descriptor()
                    }
                    "clone" => {
                        if store.tracks.len() >= TRACK_LIMIT {
                            return Err(Error::named(
                                "QuotaExceededError",
                                "too many live capture tracks",
                            ));
                        }
                        let cloned = Arc::new(track.clone_track(allocate()?)?);
                        let descriptor = cloned.descriptor()?;
                        if !cloned.is_stopped() {
                            store.tracks.insert(cloned.id, cloned);
                        }
                        Ok(descriptor)
                    }
                    "probe" => {
                        let micros = request
                            .get("micros")
                            .and_then(Value::as_u64)
                            .filter(|value| *value <= 86_400_000_000)
                            .ok_or_else(|| {
                                Error::typ("probe time must be integer microseconds within one day")
                            })?;
                        probe(&track, micros)
                    }
                    _ => Err(Error::typ("unknown media operation")),
                }
            }
        }
    }

    fn updates(&self) -> Result<Value, Error> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| Error::state("capture host state was poisoned"))?;
        let mut descriptors = Vec::new();
        let mut ended = Vec::new();
        for (&id, track) in &store.tracks {
            let status = track.status();
            if store.published.get(&id) != Some(&status) {
                descriptors.push(track.descriptor()?);
            }
            if status.2 {
                ended.push(id);
            }
        }
        let statuses: Vec<_> = store
            .tracks
            .iter()
            .map(|(&id, track)| (id, track.status()))
            .collect();
        for (id, status) in statuses {
            store.published.insert(id, status);
        }
        for id in ended {
            store.tracks.remove(&id);
            store.audio_tracks.remove(&id);
            store.published.remove(&id);
        }
        let changed = store.revision != store.published_revision;
        store.published_revision = store.revision;
        Ok(json!({"devicechange": changed, "tracks": descriptors}))
    }
}

fn probe(track: &Track, micros: u64) -> Result<Value, Error> {
    match track.settings()? {
        Settings::Video { .. } => {
            let frame = track.video_at(Duration::from_micros(micros))?;
            let VideoPixels::I420 { y, .. } = frame.pixels() else {
                return Err(Error::state("the fake camera must produce I420"));
            };
            let mut checksum = 2_166_136_261_u32;
            let mut black = true;
            for row in 0..y.rows() {
                for byte in y
                    .row(row)
                    .ok_or_else(|| Error::state("invalid fake frame row"))?
                {
                    checksum = (checksum ^ u32::from(*byte)).wrapping_mul(16_777_619);
                    black &= *byte == 16;
                }
            }
            Ok(json!({
                "sourceId": frame.source, "seed": track.source.seed,
                "width": frame.width, "height": frame.height,
                "timestamp": frame.presentation.seconds(),
                "checksum": checksum, "black": black,
                "current": frame.generation.is_current()
            }))
        }
        Settings::Audio {
            sample_rate,
            channel_count,
        } => {
            let first_frame = micros
                .checked_mul(u64::from(sample_rate))
                .ok_or_else(|| Error::state("fake PCM timestamp overflow"))?
                / 1_000_000;
            let pcm = track.pcm_at(first_frame, (sample_rate / 50) as usize)?;
            let mut crossings = 0;
            let mut previous = None;
            for channels in pcm.samples().chunks_exact(usize::from(channel_count)) {
                let positive = channels[0] > 0.0;
                if previous.is_some_and(|previous| previous != positive) {
                    crossings += 1;
                }
                previous = Some(positive);
            }
            Ok(json!({
                "sourceId": pcm.source, "sampleRate": pcm.sample_rate,
                "channels": pcm.channels, "frames": pcm.frames(),
                "sampleCount": pcm.samples().len(),
                "timestamp": pcm.presentation.seconds(),
                "energy": pcm.energy(), "rms": pcm.rms(),
                "zeroCrossings": crossings,
                "current": pcm.generation.is_current()
            }))
        }
    }
}

struct NativeHost(Arc<Host>);

impl Drop for NativeHost {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// Obtain the page's native capture registry without passing samples through JS.
pub fn host(context: &Context) -> JsResult<Arc<Host>> {
    context
        .get_data::<NativeHost>()
        .map(|host| Arc::clone(&host.0))
        .ok_or_else(|| {
            JsNativeError::typ()
                .with_message("capture host is unavailable")
                .into()
        })
}

fn control(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let text = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let result = if text.len() > COMMAND_LIMIT {
        Err(Error::typ("media command exceeds 64 KiB"))
    } else {
        serde_json::from_str::<Value>(&text)
            .map_err(|_| Error::typ("invalid media command"))
            .and_then(|request| host.control(request, context))
    };
    let updates = host
        .updates()
        .unwrap_or(json!({"devicechange": false, "tracks": []}));
    let reply = match result {
        Ok(value) => json!({"ok": true, "value": value, "updates": updates}),
        Err(error) => json!({"ok": false, "error": error, "updates": updates}),
    };
    Ok(JsString::from(reply.to_string()).into())
}

pub(crate) fn install(document: &mut ScriptDocument) {
    let url = document.current_url().map(ToString::to_string);
    let host = Arc::new(Host {
        store: Mutex::new(Store::default()),
        origin: Instant::now(),
        secure: trusted(url.as_deref()),
    });
    document.with_js_context(|context| {
        context.insert_data(NativeHost(Arc::clone(&host)));
        context
            .register_global_builtin_callable(
                JsString::from("__chuzzMediaControl"),
                1,
                NativeFunction::from_fn_ptr(control),
            )
            .expect("capture metadata bridge");
    });
    document.eval(include_str!("../media.ts"));
    document.add_poll_hook(move |document, _| {
        let Ok(updates) = host.updates() else {
            return false;
        };
        if updates["devicechange"] != true && updates["tracks"].as_array().is_none_or(Vec::is_empty)
        {
            return false;
        }
        document.eval(&format!("globalThis.__chuzzMediaUpdate({updates});"));
        true
    });
}
