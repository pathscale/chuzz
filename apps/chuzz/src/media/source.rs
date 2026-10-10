//! Capture and graph sources share PCM and a page-owned media clock.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::fake;
use super::frame::{Generation, VideoFrame};
use super::pcm::{MAX_BLOCK_FRAMES, Pcm};
use super::track::{Kind, Settings};
use super::{Error, allocate};

pub mod remote;

use crate::audio::pcm::Track as AudioTrack;

enum Input {
    Capture(fake::Source),
    Audio(Arc<AudioTrack>),
    Video(Arc<remote::Video>),
}

pub struct Source {
    pub id: u64,
    pub kind: Kind,
    pub seed: u32,
    pub muted: AtomicBool,
    origin: Instant,
    input: Input,
}

impl Source {
    pub fn new(kind: Kind, origin: Instant, muted: bool) -> Result<Self, Error> {
        let capture = fake::Source::new(kind, origin, muted)?;
        Ok(Self {
            id: capture.id,
            kind,
            seed: capture.seed,
            muted: AtomicBool::new(muted),
            origin,
            input: Input::Capture(capture),
        })
    }

    pub fn from_audio(track: Arc<AudioTrack>, origin: Instant) -> Result<Self, Error> {
        if track.ended() {
            return Err(Error::state("the graph PCM source has ended"));
        }
        Ok(Self {
            id: allocate()?,
            kind: Kind::Audio,
            seed: 0,
            muted: AtomicBool::new(false),
            origin,
            input: Input::Audio(track),
        })
    }

    pub fn from_video(video: Arc<remote::Video>, origin: Instant) -> Result<Self, Error> {
        if video.ended() {
            return Err(Error::state("the remote video source has ended"));
        }
        Ok(Self {
            id: allocate()?,
            kind: Kind::Video,
            seed: 0,
            muted: AtomicBool::new(true),
            origin,
            input: Input::Video(video),
        })
    }

    pub fn remote_video(&self) -> Option<&Arc<remote::Video>> {
        match &self.input {
            Input::Video(video) => Some(video),
            _ => None,
        }
    }

    pub fn video_ready(&self) -> bool {
        self.remote_video().is_none_or(|video| video.ready())
    }

    pub fn audio(&self) -> Option<&Arc<AudioTrack>> {
        match &self.input {
            Input::Capture(_) | Input::Video(_) => None,
            Input::Audio(track) => Some(track),
        }
    }

    pub fn is_capture(&self) -> bool {
        matches!(&self.input, Input::Capture(_))
    }

    pub fn ended(&self) -> bool {
        self.audio().is_some_and(|track| track.ended())
            || self.remote_video().is_some_and(|video| video.ended())
    }

    pub fn elapsed(&self) -> Duration {
        self.origin.elapsed()
    }

    pub fn is_muted(&self) -> bool {
        self.muted.load(Ordering::Acquire)
    }

    pub fn label(&self) -> &'static str {
        if self.is_capture() {
            self.kind.label()
        } else if self.remote_video().is_some() {
            "Remote WebRTC video"
        } else {
            "Web Audio destination"
        }
    }

    pub fn settings_json(&self, settings: Settings) -> Value {
        if let Some(track) = self.audio() {
            json!({
                "sampleRate": track.sample_rate(),
                "sampleSize": 32,
                "channelCount": track.channels(),
                "latency": 128.0 / f64::from(track.sample_rate())
            })
        } else if let Some(video) = self.remote_video() {
            let [width, height] = video.dimensions().unwrap_or([0, 0]);
            json!({
                "width": width,
                "height": height,
                "aspectRatio": if height == 0 { 0.0 } else { f64::from(width) / f64::from(height) }
            })
        } else {
            settings.json()
        }
    }

    pub fn capabilities(&self) -> Value {
        if let Some(track) = self.audio() {
            let rate = track.sample_rate();
            let channels = track.channels();
            let latency = 128.0 / f64::from(rate);
            json!({
                "sampleRate": {"min": rate, "max": rate},
                "sampleSize": {"min": 32, "max": 32},
                "channelCount": {"min": channels, "max": channels},
                "latency": {"min": latency, "max": latency}
            })
        } else if self.remote_video().is_some() {
            json!({
                "width": {"min": 2, "max": 1920},
                "height": {"min": 2, "max": 1080}
            })
        } else {
            self.kind.capabilities()
        }
    }

    pub fn video(
        &self,
        settings: &Settings,
        at: Duration,
        silent: bool,
        generation: Generation,
    ) -> Result<VideoFrame, Error> {
        match &self.input {
            Input::Capture(capture) => capture.video(settings, at, silent, generation),
            Input::Video(video) => video.frame(generation, silent),
            Input::Audio(_) => Err(Error::state("a graph audio source cannot produce video")),
        }
    }

    pub fn pcm(
        &self,
        settings: &Settings,
        first_frame: u64,
        frames: usize,
        silent: bool,
        generation: Generation,
    ) -> Result<Pcm, Error> {
        match &self.input {
            Input::Capture(capture) => {
                capture.pcm(settings, first_frame, frames, silent, generation)
            }
            Input::Video(_) => Err(Error::state("a remote video source cannot produce PCM")),
            Input::Audio(track) => {
                if frames == 0 || frames > MAX_BLOCK_FRAMES {
                    return Err(Error::state("graph PCM request exceeds the block limit"));
                }
                first_frame
                    .checked_add(frames as u64)
                    .ok_or_else(|| Error::state("graph PCM position overflow"))?;
                if track.ended() {
                    return Err(Error::state("the graph PCM source has ended"));
                }
                let rate = track.sample_rate();
                let channels = track.channels();
                let samples = if silent {
                    vec![0.0; frames * usize::from(channels)]
                } else {
                    track.read(
                        Duration::from_secs_f64(first_frame as f64 / f64::from(rate)),
                        frames,
                        rate,
                        channels,
                    )
                };
                Pcm::new(
                    self.id,
                    rate,
                    channels,
                    first_frame,
                    generation,
                    Arc::from(samples),
                )
            }
        }
    }
}

