//! Progressive video, driven by document polling.
//!
//! Browser policy and the media clock stay here. A platform decoder supplies
//! presentation timestamps and RGBA frames; Blitz owns sizing and painting.
//! The script bridge drains commands during host polls and does not replace
//! the IPC handler used by fetch, cookies and WebSocket.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::{Duration, Instant};

use blitz_dom::NodeId;
use blitz_dom::node::{ImageData, RasterImageData, SpecialElementData};
use blitz_script::ScriptDocument;
use blitz_traits::net::{Bytes, Request};

use crate::document_loader::NetProvider;

#[cfg(target_os = "macos")]
mod macos;

const FETCH_DEADLINE: Duration = Duration::from_secs(20);
const MAX_MOVIE_BYTES: usize = 128 * 1024 * 1024;
const FRAMES_PER_POLL: usize = 8;
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
    time: f64,
    width: u32,
    height: u32,
    rgba: Arc<Vec<u8>>,
}

/// A decoder has no clock, DOM, network provider or autoplay policy.
trait VideoDecoder {
    fn duration(&self) -> f64;
    fn next_frame(&mut self) -> Result<Option<Frame>, MediaError>;
    fn seek(&mut self, time: f64) -> Result<(), MediaError>;
}

fn open_decoder(bytes: &[u8]) -> Result<Box<dyn VideoDecoder>, MediaError> {
    #[cfg(target_os = "macos")]
    {
        macos::open(bytes).map(|decoder| Box::new(decoder) as Box<dyn VideoDecoder>)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = bytes;
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
}

struct Delivery {
    id: u64,
    generation: u64,
    result: Result<(String, Bytes), MediaError>,
}

struct Entry {
    node: NodeId,
    generation: u64,
    src: String,
    decoder: Option<Box<dyn VideoDecoder>>,
    cancelled: Arc<AtomicBool>,
    next: Option<Frame>,
    duration: f64,
    position: f64,
    anchor_position: f64,
    anchor: Instant,
    last_timeupdate: Instant,
    width: u32,
    height: u32,
    metadata: bool,
    has_frame: bool,
    eof: bool,
    paused: bool,
    ended: bool,
    looping: bool,
    reported_playing: bool,
    dirty: bool,
    error: Option<MediaError>,
    events: Vec<&'static str>,
}

impl Drop for Entry {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

impl Entry {
    fn new(node: NodeId, command: &Command, now: Instant) -> Self {
        Self {
            node,
            generation: command.generation,
            src: command.src.clone().unwrap_or_default(),
            decoder: None,
            cancelled: Arc::new(AtomicBool::new(false)),
            next: None,
            duration: 0.0,
            position: 0.0,
            anchor_position: 0.0,
            anchor: now,
            last_timeupdate: now,
            width: 0,
            height: 0,
            metadata: false,
            has_frame: false,
            eof: false,
            paused: true,
            ended: false,
            looping: command.looping,
            reported_playing: false,
            dirty: true,
            error: None,
            events: Vec::new(),
        }
    }

    fn time(&self, now: Instant) -> f64 {
        if self.paused || self.decoder.is_none() {
            self.position
        } else {
            (self.anchor_position + now.duration_since(self.anchor).as_secs_f64())
                .min(self.duration)
        }
    }

    fn fail(&mut self, error: MediaError) {
        self.cancelled.store(true, Ordering::Release);
        self.decoder = None;
        self.next = None;
        self.paused = true;
        self.reported_playing = false;
        self.error = Some(error);
        self.events.push("error");
        self.dirty = true;
    }

    fn seek(&mut self, time: f64, now: Instant) -> Result<(), MediaError> {
        self.position = if self.decoder.is_some() {
            time.clamp(0.0, self.duration)
        } else {
            time.max(0.0)
        };
        if let Some(decoder) = self.decoder.as_mut() {
            decoder.seek(self.position)?;
        }
        self.anchor = now;
        self.anchor_position = self.position;
        self.next = None;
        self.eof = false;
        self.has_frame = false;
        self.ended = false;
        self.reported_playing = false;
        self.dirty = true;
        Ok(())
    }

    fn command(&mut self, command: &Command, now: Instant) -> Result<(), MediaError> {
        self.looping = command.looping;
        match command.op.as_str() {
            "play" if self.error.is_none() => {
                if self.ended {
                    self.seek(0.0, now)?;
                }
                if self.paused {
                    self.anchor = now;
                    self.anchor_position = self.position;
                    self.paused = false;
                    self.reported_playing = false;
                    self.events.push("play");
                    self.dirty = true;
                }
            }
            "pause" => {
                if !self.paused {
                    self.position = self.time(now);
                    self.paused = true;
                    self.reported_playing = false;
                    self.events.push("pause");
                    self.dirty = true;
                }
            }
            "seek" => {
                if let Some(time) = command.time.filter(|time| time.is_finite()) {
                    self.seek(time, now)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn loaded(&mut self, src: String, bytes: Bytes, now: Instant) -> Result<(), MediaError> {
        let mut decoder = open_decoder(&bytes)?;
        self.duration = decoder.duration();
        self.position = self.position.clamp(0.0, self.duration);
        if self.position > 0.0 {
            decoder.seek(self.position)?;
        }
        self.src = src;
        self.decoder = Some(decoder);
        self.anchor = now;
        self.anchor_position = self.position;
        self.last_timeupdate = now;
        self.dirty = true;
        Ok(())
    }

    fn tick(&mut self, now: Instant) -> Result<Option<Frame>, MediaError> {
        if self.decoder.is_none() || self.error.is_some() {
            return Ok(None);
        }
        if self.paused && self.has_frame {
            return Ok(None);
        }

        let target = self.time(now);
        self.position = target;
        let mut selected = None;

        // Decode a bounded number of samples per host turn. Retain at most
        // one future frame; never decode the whole movie into memory.
        for _ in 0..FRAMES_PER_POLL {
            if self.next.is_none() && !self.eof {
                self.next = self.decoder.as_mut().unwrap().next_frame()?;
                self.eof = self.next.is_none();
            }
            let Some(frame) = self.next.as_ref() else {
                break;
            };
            if frame.time > target && self.has_frame {
                break;
            }
            let frame = self.next.take().unwrap();
            self.width = frame.width;
            self.height = frame.height;
            self.has_frame = true;
            if !self.metadata {
                self.metadata = true;
                self.events
                    .extend(["loadedmetadata", "loadeddata", "canplay"]);
            }
            selected = Some(frame);
        }

        if self.eof && !self.has_frame {
            return Err(MediaError::decode("the video contains no decodable frames"));
        }
        if !self.paused && self.has_frame {
            self.dirty = true;
            if !self.reported_playing {
                self.reported_playing = true;
                self.events.push("playing");
            }
            if now.duration_since(self.last_timeupdate) >= TIMEUPDATE_INTERVAL {
                self.last_timeupdate = now;
                self.events.push("timeupdate");
            }
            if self.eof && target >= self.duration {
                if self.looping {
                    self.seek(0.0, now)?;
                    self.reported_playing = true;
                } else {
                    self.position = self.duration;
                    self.paused = true;
                    self.ended = true;
                    self.reported_playing = false;
                    self.events.extend(["timeupdate", "ended"]);
                }
            }
        }
        Ok(selected)
    }

    fn publish(&mut self, document: &mut ScriptDocument, id: u64) {
        let ready_state = if self.error.is_some() {
            0
        } else if self.has_frame {
            4
        } else if self.metadata {
            1
        } else {
            0
        };
        let payload = serde_json::json!({
            "src": self.src,
            "duration": self.metadata.then_some(self.duration),
            "time": self.position,
            "paused": self.paused,
            "ended": self.ended,
            "readyState": ready_state,
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
    inbox: Arc<Mutex<Vec<Delivery>>>,
}

impl Host {
    fn fetch(&self, document: &ScriptDocument, id: u64, entry: &Entry) {
        let url = document.inner().url().join(&entry.src);
        let net = Arc::clone(&self.net);
        let cancelled = Arc::clone(&entry.cancelled);
        let inbox = Arc::downgrade(&self.inbox);
        let generation = entry.generation;

        let _ = nagoya::spawn(async move {
            let result = match url {
                Ok(url) if matches!(url.scheme(), "http" | "https" | "file" | "data") => {
                    let mut fetch = Box::pin(nagoya::timeout(
                        FETCH_DEADLINE,
                        net.fetch_response_async(Request::get(url)),
                    ));
                    let stop = Arc::clone(&cancelled);
                    let mut stopped = Box::pin(async move {
                        while !stop.load(Ordering::Acquire) {
                            nagoya::sleep(Duration::from_millis(5)).await;
                        }
                    });
                    let result = std::future::poll_fn(|context| {
                        if stopped.as_mut().poll(context).is_ready() {
                            return Poll::Ready(None);
                        }
                        match fetch.as_mut().poll(context) {
                            Poll::Ready(result) => Poll::Ready(Some(result)),
                            Poll::Pending => Poll::Pending,
                        }
                    })
                    .await;
                    match result {
                        None => return,
                        Some(Ok(Ok(response))) if !response.status.is_success() => {
                            Err(MediaError::network(format!(
                                "video request returned HTTP {}",
                                response.status
                            )))
                        }
                        Some(Ok(Ok(response))) if response.body.len() > MAX_MOVIE_BYTES => {
                            Err(MediaError::unsupported(
                                "the progressive video exceeds the 128 MiB limit",
                            ))
                        }
                        Some(Ok(Ok(response))) => {
                            Ok((response.url.to_string(), response.body))
                        }
                        Some(Ok(Err(error))) => {
                            Err(MediaError::network(format!("video fetch failed: {error:?}")))
                        }
                        Some(Err(_)) => Err(MediaError::network("video fetch timed out")),
                    }
                }
                _ => Err(MediaError::unsupported("unsupported video URL")),
            };
            if cancelled.load(Ordering::Acquire) {
                return;
            }
            if let Some(inbox) = inbox.upgrade()
                && let Ok(mut queue) = inbox.lock()
            {
                queue.push(Delivery {
                    id,
                    generation,
                    result,
                });
            }
        });
    }

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
                let mut entry = Entry::new(node, &command, now);
                if cfg!(target_os = "macos") {
                    self.fetch(document, command.id, &entry);
                } else {
                    entry.fail(MediaError::unsupported(
                        "video decoding is unavailable on this platform",
                    ));
                }
                self.entries.insert(command.id, entry);
                continue;
            }

            if let Some(entry) = self.entries.get_mut(&command.id)
                && entry.generation == command.generation
                && let Err(error) = entry.command(&command, now)
            {
                entry.fail(error);
            }
        }

        let deliveries = self
            .inbox
            .lock()
            .map(|mut queue| std::mem::take(&mut *queue))
            .unwrap_or_default();
        for delivery in deliveries {
            let Some(entry) = self.entries.get_mut(&delivery.id) else {
                continue;
            };
            if entry.generation != delivery.generation {
                continue;
            }
            let result = delivery
                .result
                .and_then(|(src, bytes)| entry.loaded(src, bytes, now));
            if let Err(error) = result {
                entry.fail(error);
            }
        }

        self.entries
            .retain(|_, entry| document.inner().get_node(entry.node).is_some());
        for (&id, entry) in &mut self.entries {
            match entry.tick(now) {
                Ok(Some(frame)) => {
                    set_image(document, entry.node, Some(frame));
                    changed = true;
                    entry.dirty = true;
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
    let Some(node) = document.inner_mut().get_node_mut(id) else {
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
        inbox: Arc::new(Mutex::new(Vec::new())),
    };
    document.add_poll_hook(move |document, _| host.poll(document));
}
