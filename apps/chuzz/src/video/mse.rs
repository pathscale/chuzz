//! Document-owned MSE state and native byte snapshots.
//! No media payload is serialized through JSON.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use super::fragments::{Codec, Encoded, Fragments, Track};
use super::{MediaError, VideoDecoder};

#[path = "mse_playback.rs"]
mod playback;

#[path = "mse_script.rs"]
mod script;

pub(super) use script::install;

const SOURCE_LIMIT: usize = 128 * 1024 * 1024;
const DOCUMENT_LIMIT: usize = 256 * 1024 * 1024;
const UPLOAD_LIMIT: usize = 16 * 1024 * 1024;
const MAX_ID: u64 = 9_007_199_254_740_991;

#[derive(serde::Serialize)]
struct Error {
    name: &'static str,
    message: String,
}

impl Error {
    fn state(message: &str) -> Self {
        Self {
            name: "InvalidStateError",
            message: message.into(),
        }
    }

    fn decode(error: MediaError) -> Self {
        Self {
            name: "EncodingError",
            message: error.message,
        }
    }

    fn quota() -> Self {
        Self {
            name: "QuotaExceededError",
            message: "MSE memory or resource limit reached".into(),
        }
    }
}

#[derive(Clone, Copy)]
struct Mime {
    video: bool,
    audio: bool,
}

fn mime(value: &str) -> Option<Mime> {
    let value = value.to_ascii_lowercase();
    let mut fields = value.split(';');
    let container = fields.next()?.trim();
    if !matches!(container, "video/mp4" | "audio/mp4") {
        return None;
    }
    let mut codecs = None;
    for field in fields {
        let (key, value) = field.trim().split_once('=')?;
        if key.trim() != "codecs" || codecs.is_some() {
            return None;
        }
        let value = value.trim();
        let value = if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
            &value[1..value.len() - 1]
        } else if value.contains('"') {
            return None;
        } else {
            value
        };
        codecs = Some(value);
    }
    let Some(codecs) = codecs else {
        return Some(Mime {
            video: container == "video/mp4",
            audio: container == "audio/mp4",
        });
    };
    let mut result = Mime {
        video: false,
        audio: false,
    };
    for codec in codecs.split(',').map(str::trim) {
        if codec == "mp4a.40.2" && !result.audio {
            result.audio = true;
        } else {
            let profile = codec.strip_prefix("avc1.")?;
            if result.video
                || container == "audio/mp4"
                || profile.len() != 6
                || !profile.bytes().all(|byte| byte.is_ascii_hexdigit())
                || !matches!(
                    u8::from_str_radix(&profile[..2], 16).ok(),
                    Some(66 | 77 | 100)
                )
            {
                return None;
            }
            result.video = true;
        }
    }
    (result.video || result.audio).then_some(result)
}

pub(super) fn merge(mut ranges: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    ranges.retain(|range| range[0].is_finite() && range[1].is_finite() && range[1] > range[0]);
    ranges.sort_by(|left, right| left[0].total_cmp(&right[0]));
    let mut result: Vec<[f64; 2]> = Vec::new();
    for range in ranges {
        if let Some(last) = result.last_mut()
            && range[0] <= last[1] + 0.000001
        {
            last[1] = last[1].max(range[1]);
        } else {
            result.push(range);
        }
    }
    result
}

fn intersection(left: &[[f64; 2]], right: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let mut result = Vec::new();
    let (mut a, mut b) = (0, 0);
    while a < left.len() && b < right.len() {
        let start = left[a][0].max(right[b][0]);
        let end = left[a][1].min(right[b][1]);
        if end > start {
            result.push([start, end]);
        }
        if left[a][1] < right[b][1] {
            a += 1;
        } else {
            b += 1;
        }
    }
    result
}

#[derive(Clone)]
struct Buffer {
    mime: Mime,
    parser: Fragments,
    video: Arc<Vec<Encoded>>,
    audio: Arc<Vec<Encoded>>,
    offset: f64,
    need_sync: bool,
}

impl Buffer {
    fn new(mime: Mime) -> Self {
        Self {
            mime,
            parser: Fragments::default(),
            video: Arc::new(Vec::new()),
            audio: Arc::new(Vec::new()),
            offset: 0.0,
            need_sync: true,
        }
    }

    fn end(&self) -> f64 {
        self.video
            .iter()
            .chain(self.audio.iter())
            .map(|packet| packet.sample.end())
            .fold(0.0, f64::max)
    }

    fn ranges(&self, ended: Option<f64>) -> Vec<[f64; 2]> {
        let ranges = |packets: &[Encoded]| {
            let mut ranges = merge(
                packets
                    .iter()
                    .map(|packet| [packet.sample.pts.max(0.0), packet.sample.end()])
                    .collect(),
            );
            if let Some(end) = ended
                && let Some(last) = ranges.last_mut()
            {
                last[1] = last[1].max(end);
            }
            ranges
        };
        match (self.mime.video, self.mime.audio) {
            (true, true) => intersection(&ranges(&self.video), &ranges(&self.audio)),
            (true, false) => ranges(&self.video),
            (false, true) => ranges(&self.audio),
            _ => Vec::new(),
        }
    }

    fn remove(&mut self, start: f64, end: f64) -> bool {
        let before = self.video.len() + self.audio.len();
        let video = Arc::make_mut(&mut self.video);
        let mut dependent = false;
        video.retain(|packet| {
            let overlaps = packet.sample.pts < end && packet.sample.end() > start;
            if packet.sample.sync && packet.sample.pts >= end {
                dependent = false;
            }
            if overlaps {
                dependent = true;
            }
            !overlaps && !dependent
        });
        Arc::make_mut(&mut self.audio)
            .retain(|packet| packet.sample.pts >= end || packet.sample.end() <= start);
        before != self.video.len() + self.audio.len()
    }

    fn append(&mut self, bytes: Arc<Vec<u8>>, request: &Request) -> Result<bool, Error> {
        if !request.timestamp_offset.is_finite()
            || !request.append_window_start.is_finite()
            || request.append_window_start < 0.0
            || request
                .append_window_end
                .is_some_and(|end| !end.is_finite() || end <= request.append_window_start)
        {
            return Err(Error::state("invalid append parameters"));
        }
        let mut staged = self.clone();
        let packets = staged.parser.append(bytes).map_err(Error::decode)?;
        if !staged.parser.tracks.is_empty() {
            let video = staged
                .parser
                .tracks
                .iter()
                .filter(|track| matches!(&track.codec, Codec::Video { .. }))
                .count();
            let audio = staged
                .parser
                .tracks
                .iter()
                .filter(|track| matches!(&track.codec, Codec::Audio { .. }))
                .count();
            if video != usize::from(staged.mime.video) || audio != usize::from(staged.mime.audio) {
                return Err(Error::state(
                    "initialization tracks do not match the declared codecs",
                ));
            }
        }
        if request.timestamp_offset != staged.offset {
            staged.need_sync = true;
        }
        let origin = packets
            .iter()
            .map(|packet| packet.sample.pts)
            .reduce(f64::min);
        let shift = if request.sequence {
            request.timestamp_offset - origin.unwrap_or(request.timestamp_offset)
        } else {
            request.timestamp_offset
        };
        let mut video = Vec::new();
        let mut audio = Vec::new();
        for mut packet in packets {
            let track = staged
                .parser
                .tracks
                .iter()
                .find(|track| track.id == packet.track)
                .ok_or_else(|| Error::state("unknown appended track"))?;
            let is_video = matches!(&track.codec, Codec::Video { .. });
            packet.sample.pts += shift;
            packet.decode_time += shift;
            if !packet.sample.pts.is_finite()
                || !packet.sample.end().is_finite()
                || packet.sample.pts < request.append_window_start
                || packet.sample.end() > request.append_window_end.unwrap_or(f64::INFINITY)
            {
                if is_video {
                    staged.need_sync = true;
                }
                continue;
            }
            if is_video {
                if staged.need_sync && !packet.sample.sync {
                    continue;
                }
                staged.need_sync = false;
                video.push(packet);
            } else {
                audio.push(packet);
            }
        }
        let mut changed = false;
        for range in merge(
            video
                .iter()
                .chain(audio.iter())
                .map(|packet| [packet.sample.pts, packet.sample.end()])
                .collect(),
        ) {
            changed |= staged.remove(range[0], range[1]);
        }
        changed |= video
            .first()
            .zip(staged.video.last())
            .is_some_and(|(new, old)| new.decode_time < old.decode_time);
        changed |= audio
            .first()
            .zip(staged.audio.last())
            .is_some_and(|(new, old)| new.decode_time < old.decode_time);
        let group_end = video
            .iter()
            .chain(audio.iter())
            .map(|packet| packet.sample.end())
            .reduce(f64::max);
        Arc::make_mut(&mut staged.video).extend(video);
        Arc::make_mut(&mut staged.audio).extend(audio);
        Arc::make_mut(&mut staged.video).sort_by(|a, b| a.decode_time.total_cmp(&b.decode_time));
        Arc::make_mut(&mut staged.audio).sort_by(|a, b| a.decode_time.total_cmp(&b.decode_time));
        if staged.video.len() + staged.audio.len() > 1_000_000 {
            return Err(Error::quota());
        }
        staged.offset = if request.sequence {
            group_end.unwrap_or(request.timestamp_offset)
        } else {
            request.timestamp_offset
        };
        *self = staged;
        Ok(changed)
    }
}

pub(super) struct Presentation {
    buffers: BTreeMap<u64, Buffer>,
    duration: f64,
    ended: bool,
    closed: bool,
    epoch: u64,
    live: Option<[f64; 2]>,
    error: Option<MediaError>,
}

impl Default for Presentation {
    fn default() -> Self {
        Self {
            buffers: BTreeMap::new(),
            duration: f64::NAN,
            ended: false,
            closed: false,
            epoch: 0,
            live: None,
            error: None,
        }
    }
}

pub(super) struct Input {
    pub track: Arc<Track>,
    pub packets: Arc<Vec<Encoded>>,
}

pub(super) struct View {
    pub video: Option<Input>,
    pub audio: Option<Input>,
    pub duration: f64,
    pub ended: bool,
    pub closed: bool,
    pub ready: bool,
    pub epoch: u64,
    pub buffered: Vec<[f64; 2]>,
    pub seekable: Vec<[f64; 2]>,
    pub error: Option<MediaError>,
}

impl Presentation {
    fn end(&self) -> f64 {
        self.buffers.values().map(Buffer::end).fold(0.0, f64::max)
    }

    fn ready(&self) -> bool {
        !self.buffers.is_empty()
            && self
                .buffers
                .values()
                .all(|buffer| !buffer.parser.tracks.is_empty())
    }

    fn memory(&self) -> usize {
        let mut seen = HashSet::new();
        self.buffers
            .values()
            .map(|buffer| {
                buffer.parser.pending_bytes()
                    + buffer
                        .video
                        .iter()
                        .chain(buffer.audio.iter())
                        .filter(|packet| seen.insert(Arc::as_ptr(&packet.bytes)))
                        .map(|packet| packet.bytes.len())
                        .sum::<usize>()
            })
            .sum()
    }

    fn ranges(&self) -> Vec<[f64; 2]> {
        let end = self.ended.then(|| self.end());
        let mut ranges: Option<Vec<[f64; 2]>> = None;
        for buffer in self.buffers.values() {
            let next = buffer.ranges(end);
            ranges = Some(match ranges {
                None => next,
                Some(previous) => intersection(&previous, &next),
            });
        }
        ranges.unwrap_or_default()
    }

    pub fn view(&self) -> View {
        let mut video = None;
        let mut audio = None;
        for buffer in self.buffers.values() {
            for track in &buffer.parser.tracks {
                let input = Input {
                    track: Arc::clone(track),
                    packets: match &track.codec {
                        Codec::Video { .. } => Arc::clone(&buffer.video),
                        Codec::Audio { .. } => Arc::clone(&buffer.audio),
                    },
                };
                match &track.codec {
                    Codec::Video { .. } => video = Some(input),
                    Codec::Audio { .. } => audio = Some(input),
                }
            }
        }
        let buffered = self.ranges();
        let seekable = if self.duration == f64::INFINITY {
            if let Some(live) = self.live {
                vec![[
                    buffered
                        .first()
                        .map_or(live[0], |range| live[0].min(range[0])),
                    buffered
                        .last()
                        .map_or(live[1], |range| live[1].max(range[1])),
                ]]
            } else {
                buffered.clone()
            }
        } else {
            buffered.clone()
        };
        View {
            video,
            audio,
            duration: self.duration,
            ended: self.ended,
            closed: self.closed,
            ready: self.ready(),
            epoch: self.epoch,
            buffered,
            seekable,
            error: self.error.clone(),
        }
    }

    fn reply(&self, buffer: u64) -> Value {
        json!({
            "duration": self.duration.is_finite().then_some(self.duration),
            "durationKind": if self.duration.is_nan() { "unknown" }
                else if self.duration.is_infinite() { "infinite" } else { "finite" },
            "buffered": self.buffers.get(&buffer)
                .map(|buffer| buffer.ranges(self.ended.then(|| self.end()))),
            "timestampOffset": self.buffers.get(&buffer).map(|buffer| buffer.offset),
        })
    }
}

#[derive(Default)]
struct Store {
    sources: HashMap<u64, Arc<Mutex<Presentation>>>,
    uploads: HashMap<u64, Arc<Vec<u8>>>,
    next_upload: u64,
}

impl Store {
    fn memory(&self) -> usize {
        self.uploads
            .values()
            .map(|bytes| bytes.len())
            .sum::<usize>()
            + self
                .sources
                .values()
                .filter_map(|source| source.lock().ok())
                .map(|source| source.memory())
                .sum::<usize>()
    }
}

#[derive(Default)]
pub(super) struct Host {
    store: Mutex<Store>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    op: String,
    source: u64,
    #[serde(default)]
    buffer: u64,
    #[serde(default)]
    mime: String,
    #[serde(default)]
    handle: Option<u64>,
    #[serde(default)]
    value: Option<f64>,
    #[serde(default)]
    start: f64,
    #[serde(default)]
    end: Option<f64>,
    #[serde(default)]
    timestamp_offset: f64,
    #[serde(default)]
    append_window_start: f64,
    #[serde(default)]
    append_window_end: Option<f64>,
    #[serde(default)]
    sequence: bool,
    #[serde(default)]
    error: String,
}

impl Host {
    pub fn decoder(&self, id: u64) -> Result<Box<dyn VideoDecoder>, MediaError> {
        let source = self
            .store
            .lock()
            .map_err(|_| MediaError::decode("MSE store was poisoned"))?
            .sources
            .get(&id)
            .cloned()
            .ok_or_else(|| MediaError::unsupported("MediaSource is not attached"))?;
        Ok(Box::new(playback::open(source)))
    }

    fn control(&self, request: Request) -> Result<Value, Error> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| Error::state("MSE store was poisoned"))?;
        let bytes = request
            .handle
            .and_then(|handle| store.uploads.remove(&handle));
        if request.op == "open" {
            if request.source == 0
                || request.source > MAX_ID
                || store.sources.contains_key(&request.source)
            {
                return Err(Error::state("invalid or already attached MediaSource"));
            }
            if store.sources.len() >= 32 {
                return Err(Error::quota());
            }
            store.sources.insert(
                request.source,
                Arc::new(Mutex::new(Presentation::default())),
            );
            return Ok(json!({}));
        }
        if request.op == "close" {
            if let Some(source) = store.sources.remove(&request.source) {
                source
                    .lock()
                    .map_err(|_| Error::state("MSE source was poisoned"))?
                    .closed = true;
            }
            return Ok(json!({}));
        }
        let source = store
            .sources
            .get(&request.source)
            .cloned()
            .ok_or_else(|| Error::state("MediaSource is closed"))?;
        let mut source = source
            .lock()
            .map_err(|_| Error::state("MSE source was poisoned"))?;
        match request.op.as_str() {
            "info" => {}
            "add" => {
                let mime = mime(&request.mime).ok_or_else(|| Error {
                    name: "NotSupportedError",
                    message: "MSE supports H.264 and AAC-LC fragmented MP4 only".into(),
                })?;
                if source.ended
                    || request.buffer == 0
                    || source.buffers.contains_key(&request.buffer)
                {
                    return Err(Error::state("invalid SourceBuffer"));
                }
                if source.buffers.len() >= 2
                    || source.buffers.values().any(|buffer| {
                        buffer.mime.video && mime.video || buffer.mime.audio && mime.audio
                    })
                {
                    return Err(Error::quota());
                }
                source.buffers.insert(request.buffer, Buffer::new(mime));
                source.epoch += 1;
            }
            "delete" => {
                if source.buffers.remove(&request.buffer).is_none() {
                    return Err(Error::state("SourceBuffer was removed"));
                }
                source.epoch += 1;
            }
            "reopen" => {
                if source.ended {
                    source.ended = false;
                    source.epoch += 1;
                }
            }
            "append" => {
                if source.ended {
                    return Err(Error::state("MediaSource has ended"));
                }
                let bytes = bytes.ok_or_else(|| Error::state("invalid or consumed byte handle"))?;
                let mut staged = source
                    .buffers
                    .get(&request.buffer)
                    .cloned()
                    .ok_or_else(|| Error::state("SourceBuffer was removed"))?;
                let changed = staged.append(bytes, &request)?;
                let old = source.buffers.insert(request.buffer, staged);
                if source.memory() > SOURCE_LIMIT {
                    if let Some(old) = old {
                        source.buffers.insert(request.buffer, old);
                    }
                    return Err(Error::quota());
                }
                if changed {
                    source.epoch += 1;
                }
                if source.ready() {
                    if source.duration.is_nan() {
                        source.duration = f64::INFINITY;
                    } else if source.duration.is_finite() {
                        source.duration = source.duration.max(source.end());
                    }
                }
            }
            "abort" => {
                source
                    .buffers
                    .get_mut(&request.buffer)
                    .ok_or_else(|| Error::state("SourceBuffer was removed"))?
                    .parser
                    .abort();
            }
            "remove" => {
                let end = request.end.unwrap_or(f64::INFINITY);
                if !request.start.is_finite() || request.start < 0.0 || end <= request.start {
                    return Err(Error::state("invalid removal interval"));
                }
                if source
                    .buffers
                    .get_mut(&request.buffer)
                    .ok_or_else(|| Error::state("SourceBuffer was removed"))?
                    .remove(request.start, end)
                {
                    source.epoch += 1;
                }
            }
            "duration" => {
                let duration = request.value.unwrap_or(f64::INFINITY);
                if duration < 0.0 || duration.is_nan() || source.ended {
                    return Err(Error::state("invalid MSE duration"));
                }
                if duration < source.duration {
                    for buffer in source.buffers.values_mut() {
                        buffer.remove(duration, f64::INFINITY);
                    }
                    source.epoch += 1;
                }
                source.duration = duration;
            }
            "live" => {
                let end = request
                    .end
                    .ok_or_else(|| Error::state("missing live range end"))?;
                if !request.start.is_finite()
                    || !end.is_finite()
                    || request.start < 0.0
                    || end < request.start
                    || source.ended
                {
                    return Err(Error::state("invalid live seekable range"));
                }
                source.live = Some([request.start, end]);
            }
            "clearLive" => source.live = None,
            "end" => {
                if source.ended
                    || !source.ready()
                    || source.buffers.values().any(|buffer| {
                        buffer.parser.pending_bytes() != 0 || buffer.parser.fragment_pending()
                    })
                {
                    return Err(Error::state("incomplete or uninitialized MSE stream"));
                }
                source.ended = true;
                source.duration = source.end();
                source.error = match request.error.as_str() {
                    "" => None,
                    "network" => Some(MediaError::network(
                        "MediaSource ended with a network error",
                    )),
                    "decode" => Some(MediaError::decode("MediaSource ended with a decode error")),
                    _ => return Err(Error::state("invalid endOfStream error")),
                };
            }
            _ => return Err(Error::state("unknown MSE operation")),
        }
        Ok(source.reply(request.buffer))
    }
}
