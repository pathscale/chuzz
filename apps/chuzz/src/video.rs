//! Media playback driven by document polling.
//!
//! Chuzz owns source selection, autoplay policy and element lifetime.
//! The platform backend supplies a shared audio/video clock and RGBA frames;
//! Blitz owns layout and painting. Remote bytes use Chuzz's network provider.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use blitz_dom::Document as _;
use blitz_dom::NodeId;
use blitz_dom::node::{ImageData, RasterImageData, SpecialElementData};
use blitz_script::ScriptDocument;
use blitz_traits::net::Url;

use crate::document_loader::NetProvider;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod source;

const TIMEUPDATE_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Clone, serde::Serialize)]
struct MediaError {
    code: u8,
    message: String,
}

impl MediaError {
    fn network(message: impl Into<String>) -> Self {
        Self {
            code: 2,
            message: message.into(),
        }
    }

    fn decode(message: impl Into<String>) -> Self {
        Self {
            code: 3,
            message: message.into(),
        }
    }

    fn unsupported(message: impl Into<String>) -> Self {
        Self {
            code: 4,
            message: message.into(),
        }
    }
}

struct Frame {
    width: u32,
    height: u32,
    rgba: Arc<Vec<u8>>,
}

#[derive(Default)]
struct Snapshot {
    duration: Option<f64>,
    position: f64,
    width: u32,
    height: u32,
    buffered: Vec<[f64; 2]>,
    seekable: Vec<[f64; 2]>,
    seeking: bool,
    waiting: bool,
    playing: bool,
    enough: bool,
    frame: Option<Frame>,
    src: String,
}

/// All methods run on the document thread. Completion callbacks only update
/// atomic state; they never access the DOM or script context.
trait VideoDecoder {
    fn configure(&mut self, paused: bool, muted: bool, volume: f64, rate: f64);
    fn seek(&mut self, time: f64) -> Result<(), MediaError>;
    fn poll(&mut self) -> Result<Snapshot, MediaError>;
}

fn open_decoder(
    net: Arc<NetProvider>,
    url: Url,
    preload: &str,
) -> Result<Box<dyn VideoDecoder>, MediaError> {
    #[cfg(target_os = "macos")]
    {
        macos::open(net, url, preload).map(|decoder| Box::new(decoder) as Box<dyn VideoDecoder>)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (net, url, preload);
        Err(MediaError::unsupported(
            "video decoding is unavailable on this platform",
        ))
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Command {
    id: u64,
    generation: u64,
    op: String,
    #[serde(default)]
    src: Option<String>,
    #[serde(default)]
    time: Option<f64>,
    #[serde(default)]
    looping: bool,
    #[serde(default)]
    muted: bool,
    #[serde(default = "one")]
    volume: f64,
    #[serde(default = "one")]
    rate: f64,
    #[serde(default)]
    activated: bool,
    #[serde(default)]
    preload: String,
    #[serde(default)]
    force: bool,
}

fn one() -> f64 {
    1.0
}

struct Entry {
    node: NodeId,
    generation: u64,
    src: String,
    url: Url,
    decoder: Option<Box<dyn VideoDecoder>>,
    duration: Option<f64>,
    position: f64,
    pending_seek: Option<f64>,
    last_timeupdate: Instant,
    width: u32,
    height: u32,
    buffered: Vec<[f64; 2]>,
    seekable: Vec<[f64; 2]>,
    metadata: bool,
    has_frame: bool,
    paused: bool,
    ended: bool,
    seeking: bool,
    waiting: bool,
    enough: bool,
    looping: bool,
    muted: bool,
    volume: f64,
    rate: f64,
    activated: bool,
    preload: String,
    requested: bool,
    reported_playing: bool,
    ready_state: u8,
    dirty: bool,
    error: Option<MediaError>,
    events: Vec<&'static str>,
}

impl Entry {
    fn new(node: NodeId, url: Url, command: &Command, now: Instant) -> Self {
        Self {
            node,
            generation: command.generation,
            src: url.to_string(),
            url,
            decoder: None,
            duration: None,
            position: 0.0,
            pending_seek: None,
            last_timeupdate: now,
            width: 0,
            height: 0,
            buffered: Vec::new(),
            seekable: Vec::new(),
            metadata: false,
            has_frame: false,
            paused: true,
            ended: false,
            seeking: false,
            waiting: false,
            enough: false,
            looping: command.looping,
            muted: command.muted,
            volume: command.volume.clamp(0.0, 1.0),
            rate: command.rate.clamp(0.0, 16.0),
            activated: command.activated,
            preload: command.preload.clone(),
            requested: command.force || command.preload != "none",
            reported_playing: false,
            ready_state: 0,
            dirty: true,
            error: None,
            events: vec!["loadstart"],
        }
    }

    fn audible_allowed(&self) -> bool {
        self.muted || self.volume == 0.0 || self.activated
    }

    fn configure_decoder(&mut self) {
        if let Some(decoder) = self.decoder.as_mut() {
            decoder.configure(
                self.paused,
                self.muted || !self.activated,
                self.volume,
                self.rate,
            );
        }
    }

    fn fail(&mut self, error: MediaError) {
        self.decoder = None;
        self.paused = true;
        self.seeking = false;
        self.waiting = false;
        self.reported_playing = false;
        self.ready_state = 0;
        self.error = Some(error);
        self.events.push("error");
        self.dirty = true;
    }

    fn seek(&mut self, time: f64) -> Result<(), MediaError> {
        self.position = self
            .duration
            .map_or(time.max(0.0), |duration| time.clamp(0.0, duration));
        self.requested = true;
        self.ended = false;
        self.reported_playing = false;
        if !self.seeking {
            self.events.push("seeking");
        }
        self.seeking = true;
        if self.metadata {
            if let Some(decoder) = self.decoder.as_mut() {
                decoder.seek(self.position)?;
            }
            self.pending_seek = None;
        } else {
            self.pending_seek = Some(self.position);
        }
        self.dirty = true;
        Ok(())
    }

    fn command(&mut self, command: &Command) -> Result<(), MediaError> {
        self.looping = command.looping;
        self.muted = command.muted;
        if command.volume.is_finite() {
            self.volume = command.volume.clamp(0.0, 1.0);
        }
        if command.rate.is_finite() {
            self.rate = command.rate.clamp(0.0, 16.0);
        }
        self.activated |= command.activated;
        self.preload.clone_from(&command.preload);
        if self.preload != "none" {
            self.requested = true;
        }

        if !self.audible_allowed() && !self.paused {
            self.paused = true;
            self.reported_playing = false;
            self.events.push("pause");
        }

        match command.op.as_str() {
            "play" if self.error.is_none() => {
                if !self.audible_allowed() {
                    self.events.push("blocked");
                } else {
                    self.requested = true;
                    if self.ended {
                        self.seek(0.0)?;
                    }
                    if self.paused {
                        self.paused = false;
                        self.reported_playing = false;
                        self.events.push("play");
                    }
                }
            }
            "pause" => {
                if !self.paused {
                    self.paused = true;
                    self.reported_playing = false;
                    self.events.push("pause");
                }
            }
            "seek" => {
                if let Some(time) = command.time.filter(|time| time.is_finite()) {
                    self.seek(time)?;
                }
            }
            _ => {}
        }
        self.configure_decoder();
        self.dirty = true;
        Ok(())
    }

    fn tick(
        &mut self,
        net: &Arc<NetProvider>,
        now: Instant,
    ) -> Result<Option<Frame>, MediaError> {
        if self.error.is_some() || !self.requested {
            return Ok(None);
        }
        if self.decoder.is_none() {
            self.decoder = Some(open_decoder(
                Arc::clone(net),
                self.url.clone(),
                &self.preload,
            )?);
            self.configure_decoder();
            self.dirty = true;
        }

        let mut snapshot = self.decoder.as_mut().unwrap().poll()?;
        if !snapshot.src.is_empty() && self.src != snapshot.src {
            self.src = snapshot.src;
            self.dirty = true;
        }

        if let Some(duration) = snapshot.duration {
            if self.duration != Some(duration) {
                self.duration = Some(duration);
                self.events.push("durationchange");
            }
            if self.width != snapshot.width || self.height != snapshot.height {
                self.width = snapshot.width;
                self.height = snapshot.height;
                self.events.push("resize");
            }
            if !self.metadata {
                self.metadata = true;
                self.events.push("loadedmetadata");
            }
            if let Some(time) = self.pending_seek.take() {
                self.position = time.clamp(0.0, duration);
                self.decoder.as_mut().unwrap().seek(self.position)?;
                snapshot.frame = None;
                snapshot.seeking = true;
            }
        }

        if self.buffered != snapshot.buffered {
            self.buffered = snapshot.buffered;
            self.events.push("progress");
            self.dirty = true;
        }
        if self.seekable != snapshot.seekable {
            self.seekable = snapshot.seekable;
            self.dirty = true;
        }

        if self.seeking && self.metadata && !snapshot.seeking {
            self.seeking = false;
            self.events.extend(["timeupdate", "seeked"]);
            self.dirty = true;
        }
        if !self.seeking && self.position != snapshot.position {
            self.position = snapshot.position;
            self.dirty = true;
        }

        let frame = if snapshot.seeking {
            None
        } else {
            snapshot.frame.take()
        };
        if let Some(frame) = frame.as_ref() {
            if self.width != frame.width || self.height != frame.height {
                self.width = frame.width;
                self.height = frame.height;
                self.events.push("resize");
            }
            if !self.has_frame {
                self.has_frame = true;
                self.events.push("loadeddata");
            }
            self.dirty = true;
        }

        let ready_state = if !self.metadata {
            0
        } else if self.seeking || !self.has_frame {
            1
        } else if snapshot.enough {
            4
        } else if snapshot.waiting {
            2
        } else if self
            .buffered
            .iter()
            .any(|range| range[0] <= self.position && range[1] > self.position + 0.1)
        {
            3
        } else {
            2
        };
        if self.ready_state < 3 && ready_state >= 3 {
            self.events.push("canplay");
        }
        if ready_state == 4 && !self.enough {
            self.events.push("canplaythrough");
        }
        self.enough = ready_state == 4;
        if self.ready_state != ready_state {
            self.ready_state = ready_state;
            self.dirty = true;
        }

        let waiting = !self.paused && !self.seeking && snapshot.waiting;
        if waiting && !self.waiting {
            self.events.push("waiting");
            self.reported_playing = false;
        }
        self.waiting = waiting;

        if !self.paused && !self.seeking && self.has_frame && snapshot.playing {
            if !self.reported_playing {
                self.reported_playing = true;
                self.events.push("playing");
            }
            if now.duration_since(self.last_timeupdate) >= TIMEUPDATE_INTERVAL {
                self.last_timeupdate = now;
                self.events.push("timeupdate");
            }
        }

        if !self.paused
            && !self.seeking
            && self.rate > 0.0
            && self
                .duration
                .is_some_and(|duration| self.position >= duration - 0.001)
        {
            if self.looping {
                self.seek(0.0)?;
            } else {
                self.position = self.duration.unwrap();
                self.paused = true;
                self.ended = true;
                self.reported_playing = false;
                self.events.extend(["timeupdate", "pause", "ended"]);
                self.configure_decoder();
            }
            self.dirty = true;
        }
        Ok(frame)
    }

    fn publish(&mut self, document: &mut ScriptDocument, id: u64) {
        let network_state = if self.error.is_some() {
            3
        } else if self.decoder.is_none() || self.enough || self.paused && self.metadata {
            1
        } else {
            2
        };
        let payload = serde_json::json!({
            "src": self.src,
            "duration": self.duration,
            "time": self.position,
            "paused": self.paused,
            "ended": self.ended,
            "seeking": self.seeking,
            "readyState": self.ready_state,
            "networkState": network_state,
            "buffered": self.buffered,
            "seekable": self.seekable,
            "hasFrame": self.has_frame,
            "width": self.width,
            "height": self.height,
            "error": self.error,
            "events": std::mem::take(&mut self.events),
        });
        self.dirty = false;
        document.eval(&format!(
            "globalThis.__chuzzVideoUpdate({id}, {}, {payload});",
            self.generation
        ));
    }
}

struct Host {
    net: Arc<NetProvider>,
    entries: HashMap<u64, Entry>,
}

impl Host {
    fn poll(&mut self, document: &mut ScriptDocument) -> bool {
        let now = Instant::now();
        let commands = document
            .eval_json("globalThis.__chuzzVideoDrain()")
            .ok()
            .and_then(|value| serde_json::from_value::<Vec<Command>>(value).ok())
            .unwrap_or_default();
        let mut changed = false;

        for command in commands {
            if command.op == "load" || command.op == "release" {
                if let Some(previous) = self.entries.remove(&command.id) {
                    set_image(document, previous.node, None);
                    changed = true;
                }
                if command.op == "release"
                    || command.src.as_deref().is_none_or(str::is_empty)
                {
                    continue;
                }
                let selector = format!("video[data-chuzz-video='{}']", command.id);
                let Some(node) = document
                    .inner()
                    .query_selector(&selector)
                    .ok()
                    .flatten()
                else {
                    continue;
                };
                let url = document
                    .inner()
                    .url()
                    .join(command.src.as_deref().unwrap_or_default());
                let Ok(url) = url else {
                    continue;
                };
                let mut entry = Entry::new(node, url, &command, now);
                if !matches!(entry.url.scheme(), "http" | "https" | "file" | "data") {
                    entry.fail(MediaError::unsupported("unsupported video URL"));
                } else if !cfg!(target_os = "macos") {
                    entry.fail(MediaError::unsupported(
                        "video decoding is unavailable on this platform",
                    ));
                }
                self.entries.insert(command.id, entry);
                continue;
            }

            if let Some(entry) = self.entries.get_mut(&command.id)
                && entry.generation == command.generation
                && let Err(error) = entry.command(&command)
            {
                entry.fail(error);
            }
        }

        self.entries
            .retain(|_, entry| document.inner().get_node(entry.node).is_some());
        for (&id, entry) in &mut self.entries {
            match entry.tick(&self.net, now) {
                Ok(Some(frame)) => {
                    set_image(document, entry.node, Some(frame));
                    changed = true;
                }
                Ok(None) => {}
                Err(error) => entry.fail(error),
            }
            if entry.dirty || !entry.events.is_empty() {
                entry.publish(document, id);
                changed = true;
            }
        }
        changed
    }
}

fn set_image(document: &mut ScriptDocument, id: NodeId, frame: Option<Frame>) {
    // The guard must outlive the node borrow taken from it.
    let mut doc = document.inner_mut();
    let Some(node) = doc.get_node_mut(id) else {
        return;
    };
    let Some(element) = node.element_data_mut() else {
        return;
    };
    element.special_data = match frame {
        Some(frame) => SpecialElementData::Image(Box::new(ImageData::Raster(
            RasterImageData::new(frame.width, frame.height, frame.rgba),
        ))),
        None => SpecialElementData::None,
    };
    node.cache_mut().clear();
    // The public node API exposes the engine's damage value without requiring
    // Chuzz to depend on Stylo. Set every declared damage flag so intrinsic
    // dimensions and repaint-in-place both reach resolve and capture.
    if let Some(damage) = node.damage() {
        node.insert_damage(damage.union(damage.complement()));
    }
    node.set_dirty_descendants();
    node.mark_ancestors_dirty();
}

pub(crate) fn install(document: &mut ScriptDocument, net: Arc<NetProvider>) {
    document.eval(&format!(
        "globalThis.__chuzzVideoSupported = {};",
        cfg!(target_os = "macos")
    ));
    document.eval(include_str!("video.ts"));
    let mut host = Host {
        net,
        entries: HashMap::new(),
    };
    document.add_poll_hook(move |document, _| host.poll(document));
}
