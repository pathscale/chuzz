//! Live sources lease capture tracks directly. They never create network loads.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use blitz_dom::Document as _;
use blitz_dom::node::{ImageData, RasterImageData, SpecialElementData};
use blitz_script::ScriptDocument;
use blitz_script::boa_engine::{
    Context, JsNativeError, JsResult, JsString, JsValue, NativeFunction,
};
use serde_json::json;

use super::sink::HeadlessSink;
use super::{Command, MediaError, TIMEUPDATE_INTERVAL};
use crate::media::frame::{VideoFrame, VideoPixels};
use crate::media::track::{Kind, Track};

const FRAME_BYTES_LIMIT: usize = 512 * 1024 * 1024;
const LEASE_LIMIT: usize = 8192;
const LEASE_TIME: Duration = Duration::from_secs(10);
const MAX_HANDLE: u64 = 9_007_199_254_740_991;

#[derive(Clone)]
pub(crate) struct FrameLease {
    pub image: Arc<RasterImageData>,
    pub pixels: Arc<Vec<u8>>,
}

#[derive(Default)]
struct Registry {
    current: HashMap<u64, (u64, FrameLease)>,
    leases: HashMap<u64, (Instant, FrameLease)>,
    next: u64,
    closed: bool,
}

#[derive(Default)]
pub(crate) struct Frames(Mutex<Registry>);

impl Frames {
    pub(crate) fn publish(
        &self,
        id: u64,
        generation: u64,
        width: u32,
        height: u32,
        pixels: Arc<Vec<u8>>,
    ) -> Result<(), String> {
        let length = u64::from(width) * u64::from(height) * 4;
        if width == 0 || height == 0 || length != pixels.len() as u64 {
            return Err("invalid native video pixels".to_owned());
        }
        let mut registry = self.0.lock().map_err(|_| "video registry was poisoned")?;
        if registry.closed {
            return Err("video document has closed".to_owned());
        }
        let now = Instant::now();
        registry
            .leases
            .retain(|_, (created, _)| now.duration_since(*created) < LEASE_TIME);
        let other_bytes = registry
            .current
            .iter()
            .filter(|(other, _)| **other != id)
            .map(|(_, (_, frame))| frame.pixels.len())
            .chain(registry.leases.values().map(|(_, frame)| frame.pixels.len()))
            .sum::<usize>();
        if other_bytes.saturating_add(pixels.len()) > FRAME_BYTES_LIMIT
            || !registry.current.contains_key(&id) && registry.current.len() >= LEASE_LIMIT
        {
            return Err("native video allocation limit reached".to_owned());
        }
        let image = Arc::new(RasterImageData::new(width, height, Arc::clone(&pixels)));
        registry
            .current
            .insert(id, (generation, FrameLease { image, pixels }));
        Ok(())
    }

    pub(crate) fn remove(&self, id: u64) {
        if let Ok(mut registry) = self.0.lock() {
            registry.current.remove(&id);
        }
    }

    fn acquire(&self, id: u64, generation: u64) -> Result<serde_json::Value, String> {
        let mut registry = self.0.lock().map_err(|_| "video registry was poisoned")?;
        if registry.closed {
            return Err("video document has closed".to_owned());
        }
        let now = Instant::now();
        registry
            .leases
            .retain(|_, (created, _)| now.duration_since(*created) < LEASE_TIME);
        let frame = registry
            .current
            .get(&id)
            .filter(|(current, _)| *current == generation)
            .map(|(_, frame)| frame.clone())
            .ok_or("video frame is unavailable or belongs to an old source")?;
        let allocated = registry
            .current
            .values()
            .map(|(_, frame)| frame.pixels.len())
            .chain(registry.leases.values().map(|(_, frame)| frame.pixels.len()))
            .sum::<usize>();
        if registry.leases.len() >= LEASE_LIMIT
            || registry.next >= MAX_HANDLE
            || allocated.saturating_add(frame.pixels.len()) > FRAME_BYTES_LIMIT
        {
            return Err("native video lease limit reached".to_owned());
        }
        registry.next += 1;
        let handle = registry.next;
        let descriptor = json!({
            "handle": handle,
            "width": frame.image.width,
            "height": frame.image.height
        });
        registry.leases.insert(handle, (now, frame));
        Ok(descriptor)
    }

    pub(crate) fn take(&self, handle: u64) -> Result<FrameLease, String> {
        let mut registry = self.0.lock().map_err(|_| "video registry was poisoned")?;
        if registry.closed {
            return Err("video document has closed".to_owned());
        }
        let (created, frame) = registry
            .leases
            .remove(&handle)
            .ok_or("invalid or consumed video frame lease")?;
        if created.elapsed() >= LEASE_TIME {
            return Err("video frame lease expired".to_owned());
        }
        Ok(frame)
    }

    fn close(&self) {
        if let Ok(mut registry) = self.0.lock() {
            registry.closed = true;
            registry.current.clear();
            registry.leases.clear();
        }
    }
}

struct NativeFrames(Arc<Frames>);

impl Drop for NativeFrames {
    fn drop(&mut self) {
        self.0.close();
    }
}

pub(crate) fn frame_registry(context: &mut Context) -> Arc<Frames> {
    if let Some(registry) = context.get_data::<NativeFrames>() {
        return Arc::clone(&registry.0);
    }
    let frames = Arc::new(Frames::default());
    context.insert_data(NativeFrames(Arc::clone(&frames)));
    frames
}

pub(crate) fn take_frame(context: &Context, handle: u64) -> Result<FrameLease, String> {
    context
        .get_data::<NativeFrames>()
        .ok_or("native video registry is unavailable")?
        .0
        .take(handle)
}

fn acquire(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let mut numbers = [0_u64; 2];
    for (index, number) in numbers.iter_mut().enumerate() {
        let value = args
            .get(index)
            .cloned()
            .unwrap_or_default()
            .to_number(context)?;
        if !value.is_finite()
            || value.fract() != 0.0
            || !(0.0..=MAX_HANDLE as f64).contains(&value)
        {
            return Err(JsNativeError::typ().with_message("invalid video handle").into());
        }
        *number = value as u64;
    }
    let value = frame_registry(context)
        .acquire(numbers[0], numbers[1])
        .map_err(|error| JsNativeError::typ().with_message(error))?;
    Ok(JsString::from(value.to_string()).into())
}

pub(super) fn install_frame_bridge(context: &mut Context) {
    context
        .register_global_builtin_callable(
            JsString::from("__chuzzVideoAcquire"),
            2,
            NativeFunction::from_fn_ptr(acquire),
        )
        .expect("native video frame leases");
}

type FrameKey = (u64, i64, u32, u32, (bool, bool, bool));

struct Entry {
    generation: u64,
    audio_only: bool,
    tracks: Vec<Arc<Track>>,
    paused: bool,
    ended: bool,
    was_active: bool,
    metadata: bool,
    ready: bool,
    reported_playing: bool,
    muted: bool,
    volume: f64,
    activated: bool,
    position: f64,
    last_tick: Instant,
    last_publish: Instant,
    frame_key: Option<FrameKey>,
    width: u32,
    height: u32,
    total_frames: u64,
    ink: bool,
    sink: HeadlessSink,
    error: Option<MediaError>,
    events: Vec<&'static str>,
    dirty: bool,
}

impl Entry {
    fn new(command: &Command, now: Instant) -> Self {
        Self {
            generation: command.generation,
            audio_only: command.audio_only,
            tracks: Vec::new(),
            paused: true,
            ended: false,
            was_active: false,
            metadata: false,
            ready: false,
            reported_playing: false,
            muted: command.muted,
            volume: command.volume.clamp(0.0, 1.0),
            activated: command.activated,
            position: 0.0,
            last_tick: now,
            last_publish: now,
            frame_key: None,
            width: 0,
            height: 0,
            total_frames: 0,
            ink: false,
            sink: HeadlessSink::default(),
            error: None,
            events: vec!["loadstart"],
            dirty: true,
        }
    }

    fn fail(&mut self, error: MediaError) {
        self.error = Some(error);
        self.paused = true;
        self.reported_playing = false;
        self.events.push("error");
        self.dirty = true;
    }

    fn set_tracks(
        &mut self,
        capture: &crate::media::Host,
        handles: &[u64],
    ) -> Result<(), MediaError> {
        if handles.len() > 32 {
            return Err(MediaError::unsupported("too many tracks in a media element"));
        }
        let mut tracks = Vec::with_capacity(handles.len());
        for &handle in handles {
            let track = capture
                .track(handle)
                .map_err(|error| MediaError::decode(error.message))?;
            if !tracks.iter().any(|other: &Arc<Track>| other.id == track.id)
                && (!self.audio_only || track.source.kind == Kind::Audio)
            {
                tracks.push(track);
            }
        }
        self.tracks = tracks;
        self.ended = false;
        self.dirty = true;
        Ok(())
    }

    fn command(&mut self, command: &Command, capture: &crate::media::Host) {
        self.muted = command.muted;
        self.volume = command.volume.clamp(0.0, 1.0);
        self.activated |= command.activated;
        if let Some(handles) = &command.tracks
            && let Err(error) = self.set_tracks(capture, handles)
        {
            self.fail(error);
            return;
        }
        match command.op.as_str() {
            "play" if self.error.is_none() => {
                if !self.muted && self.volume > 0.0 && !self.activated {
                    self.events.push("blocked");
                } else if self.paused {
                    self.paused = false;
                    self.ended = false;
                    self.last_tick = Instant::now();
                    self.events.push("play");
                }
            }
            "pause" if !self.paused => {
                self.paused = true;
                self.reported_playing = false;
                self.last_tick = Instant::now();
                self.events.push("pause");
            }
            _ => {}
        }
        self.dirty = true;
    }

    fn tick(
        &mut self,
        document: &mut ScriptDocument,
        capture: &crate::media::Host,
        frames: &Frames,
        id: u64,
        now: Instant,
    ) -> Result<(), MediaError> {
        let elapsed = now.duration_since(self.last_tick);
        self.last_tick = now;
        if self.error.is_some() {
            return Ok(());
        }
        self.tracks.retain(|track| !track.is_stopped());
        let active = !self.tracks.is_empty();
        if !active {
            if self.was_active && !self.ended {
                self.ended = true;
                self.paused = true;
                self.reported_playing = false;
                self.events.extend(["timeupdate", "ended"]);
                self.dirty = true;
            }
            return Ok(());
        }
        self.was_active = true;
        if !self.metadata {
            self.metadata = true;
            self.events.extend(["durationchange", "loadedmetadata"]);
            self.dirty = true;
        }

        let at = capture.media_epoch().elapsed();
        let video = self
            .tracks
            .iter()
            .find(|track| !self.audio_only && track.source.kind == Kind::Video);
        if let Some(track) = video
            && (!self.paused || self.frame_key.is_none())
        {
            let frame = track
                .video_at(at)
                .map_err(|error| MediaError::decode(error.message))?;
            let key = (
                track.id,
                frame.presentation.ticks,
                frame.width,
                frame.height,
                track.status(),
            );
            if self.frame_key != Some(key) {
                let pixels = Arc::new(rgba(&frame)?);
                self.ink = pixels
                    .chunks_exact(4)
                    .any(|pixel| pixel[..3].iter().any(|value| *value != 0));
                frames
                    .publish(id, self.generation, frame.width, frame.height, pixels)
                    .map_err(MediaError::decode)?;
                if self.width != frame.width || self.height != frame.height {
                    self.width = frame.width;
                    self.height = frame.height;
                    self.events.push("resize");
                }
                self.frame_key = Some(key);
                self.total_frames += 1;
                paint(document, id, Some(frames_image(frames, id)?));
                self.dirty = true;
            }
        } else if video.is_none() && self.frame_key.take().is_some() {
            frames.remove(id);
            paint(document, id, None);
            self.width = 0;
            self.height = 0;
            self.ink = false;
            self.events.push("resize");
            self.dirty = true;
        }

        let available = video.is_none() || self.frame_key.is_some();
        if available && !self.ready {
            self.ready = true;
            self.events.extend(["loadeddata", "canplay", "canplaythrough"]);
            self.dirty = true;
        }
        if !self.paused && available {
            self.position += elapsed.as_secs_f64().min(0.25);
            let audio: Vec<_> = self
                .tracks
                .iter()
                .filter(|track| track.source.kind == Kind::Audio)
                .cloned()
                .collect();
            let gain = if self.muted || !self.activated {
                0.0
            } else {
                self.volume as f32
            };
            self.sink.consume(&audio, elapsed, at, gain);
            if !self.reported_playing {
                self.reported_playing = true;
                self.events.push("playing");
                self.dirty = true;
            }
            if now.duration_since(self.last_publish) >= TIMEUPDATE_INTERVAL {
                self.events.push("timeupdate");
                self.dirty = true;
            }
        }
        Ok(())
    }

    fn publish(&mut self, document: &mut ScriptDocument, id: u64, now: Instant) {
        let payload = json!({
            "src": "",
            "duration": null,
            "durationInfinite": self.metadata,
            "frameInk": self.ink,
            "time": self.position,
            "paused": self.paused,
            "ended": self.ended,
            "seeking": false,
            "readyState": if self.ready { 4 } else if self.metadata { 1 } else { 0 },
            "networkState": 1,
            "buffered": [],
            "seekable": [],
            "hasFrame": self.ready,
            "width": self.width,
            "height": self.height,
            "totalVideoFrames": self.total_frames,
            "sink": self.sink.stats(),
            "error": self.error,
            "events": std::mem::take(&mut self.events)
        });
        self.dirty = false;
        self.last_publish = now;
        document.eval(&format!(
            "globalThis.__chuzzVideoUpdate({id}, {}, {payload});",
            self.generation
        ));
    }
}

fn frames_image(frames: &Frames, id: u64) -> Result<Arc<RasterImageData>, MediaError> {
    let registry = frames
        .0
        .lock()
        .map_err(|_| MediaError::decode("video registry was poisoned"))?;
    registry
        .current
        .get(&id)
        .map(|(_, frame)| Arc::clone(&frame.image))
        .ok_or_else(|| MediaError::decode("live video frame was released"))
}

pub(super) struct Host {
    capture: Arc<crate::media::Host>,
    frames: Arc<Frames>,
    entries: HashMap<u64, Entry>,
}

impl Host {
    pub(super) fn new(capture: Arc<crate::media::Host>, frames: Arc<Frames>) -> Self {
        Self {
            capture,
            frames,
            entries: HashMap::new(),
        }
    }

    pub(super) fn load(&mut self, command: &Command, now: Instant) {
        let mut entry = Entry::new(command, now);
        if let Err(error) =
            entry.set_tracks(&self.capture, command.tracks.as_deref().unwrap_or_default())
        {
            entry.fail(error);
        }
        self.entries.insert(command.id, entry);
    }

    pub(super) fn remove(&mut self, document: &mut ScriptDocument, id: u64) -> bool {
        if self.entries.remove(&id).is_some() {
            self.frames.remove(id);
            paint(document, id, None);
            true
        } else {
            false
        }
    }

    pub(super) fn command(&mut self, command: &Command) -> bool {
        let Some(entry) = self.entries.get_mut(&command.id) else {
            return false;
        };
        if entry.generation == command.generation {
            entry.command(command, &self.capture);
        }
        true
    }

    pub(super) fn poll(&mut self, document: &mut ScriptDocument, now: Instant) -> bool {
        let mut changed = false;
        for (&id, entry) in &mut self.entries {
            if let Err(error) = entry.tick(document, &self.capture, &self.frames, id, now) {
                self.frames.remove(id);
                entry.fail(error);
            }
            if entry.dirty || !entry.events.is_empty() {
                entry.publish(document, id, now);
                changed = true;
            }
        }
        changed
    }
}

fn paint(document: &mut ScriptDocument, marker: u64, image: Option<Arc<RasterImageData>>) {
    let marker = marker.to_string();
    let mut document = document.inner_mut();
    let id = document.tree().iter().find_map(|(id, node)| {
        let element = node.element_data()?;
        element
            .attrs()
            .iter()
            .any(|attribute| {
                attribute.name.local.as_ref() == "data-chuzz-video"
                    && attribute.value.as_ref() == marker
            })
            .then_some(id)
    });
    let Some(node) = id.and_then(|id| document.get_node_mut(id)) else {
        return;
    };
    node.cache_mut().clear();
    let Some(element) = node.element_data_mut() else {
        return;
    };
    element.special_data = image.map_or(SpecialElementData::None, |image| {
        SpecialElementData::Image(Box::new(ImageData::Raster((*image).clone())))
    });
    if let Some(damage) = node.damage() {
        node.insert_damage(damage.union(damage.complement()));
    }
    node.set_dirty_descendants();
    node.mark_ancestors_dirty();
}

fn rgba(frame: &VideoFrame) -> Result<Vec<u8>, MediaError> {
    if !frame.generation.is_current() {
        return Err(MediaError::decode("live video frame was cancelled"));
    }
    let width = frame.width as usize;
    let height = frame.height as usize;
    let mut output = vec![0_u8; width * height * 4];
    match frame.pixels() {
        VideoPixels::Rgba(plane) => {
            for row in 0..height {
                let source = plane
                    .row(row)
                    .ok_or_else(|| MediaError::decode("invalid RGBA video row"))?;
                output[row * width * 4..(row + 1) * width * 4].copy_from_slice(source);
            }
        }
        VideoPixels::I420 { y, u, v } => {
            for row in 0..height {
                let y = y.row(row).ok_or_else(|| MediaError::decode("invalid Y row"))?;
                let u = u
                    .row(row / 2)
                    .ok_or_else(|| MediaError::decode("invalid U row"))?;
                let v = v
                    .row(row / 2)
                    .ok_or_else(|| MediaError::decode("invalid V row"))?;
                for column in 0..width {
                    let luma = (i32::from(y[column]) - 16).max(0);
                    let blue = i32::from(u[column / 2]) - 128;
                    let red = i32::from(v[column / 2]) - 128;
                    let pixel = &mut output[(row * width + column) * 4..][..4];
                    pixel[0] = ((298 * luma + 409 * red + 128) >> 8).clamp(0, 255) as u8;
                    pixel[1] =
                        ((298 * luma - 100 * blue - 208 * red + 128) >> 8).clamp(0, 255) as u8;
                    pixel[2] = ((298 * luma + 516 * blue + 128) >> 8).clamp(0, 255) as u8;
                    pixel[3] = 255;
                }
            }
        }
    }
    Ok(output)
}

