//! Synchronous deterministic capture. Scheduling belongs to the host.
//!
//! Captures share the document clock. Pulling the same source position yields
//! identical data; no timers, threads, channels or retained frame cache exist.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::frame::{Generation, MediaTime, Plane, VideoFrame, VideoPixels};
use super::pcm::{MAX_BLOCK_FRAMES, Pcm};
use super::track::{Kind, Settings};
use super::{Error, allocate};

pub const CAMERA: &str = "chuzz-fake-camera";
pub const MICROPHONE: &str = "chuzz-fake-microphone";
pub const OUTPUT: &str = "chuzz-fake-output";
pub const VIDEO_GROUP: &str = "chuzz-fake-video";
pub const AUDIO_GROUP: &str = "chuzz-fake-audio";

pub struct Source {
    pub id: u64,
    pub kind: Kind,
    pub seed: u32,
    pub muted: AtomicBool,
    origin: Instant,
}

impl Source {
    pub fn new(kind: Kind, origin: Instant, muted: bool) -> Result<Self, Error> {
        Ok(Self {
            id: allocate()?,
            kind,
            seed: 0x435a_0004,
            muted: AtomicBool::new(muted),
            origin,
        })
    }

    pub fn elapsed(&self) -> Duration {
        self.origin.elapsed()
    }

    pub fn video(
        &self,
        settings: &Settings,
        at: Duration,
        silent: bool,
        generation: Generation,
    ) -> Result<VideoFrame, Error> {
        let Settings::Video {
            width,
            height,
            frame_rate,
        } = *settings
        else {
            return Err(Error::state("audio settings cannot produce video"));
        };
        let index = (at.as_secs_f64() * frame_rate).floor() as u64;
        let ticks = (index as f64 * 1_000_000.0 / frame_rate).round();
        if !ticks.is_finite() || ticks > i64::MAX as f64 {
            return Err(Error::state("fake video timestamp overflow"));
        }
        let w = width as usize;
        let h = height as usize;
        let mut y = vec![16_u8; w * h];
        let chroma_w = w.div_ceil(2);
        let chroma_h = h.div_ceil(2);
        let mut u = vec![128_u8; chroma_w * chroma_h];
        let mut v = vec![128_u8; chroma_w * chroma_h];
        if !silent {
            let marker = (index % u64::from(width)) as usize;
            for row in 0..h {
                for column in 0..w {
                    let pattern =
                        (column as u64 + row as u64 + index % 220 + u64::from(self.seed % 220))
                            % 220;
                    y[row * w + column] = if column == marker {
                        235
                    } else {
                        16 + pattern as u8
                    };
                }
            }
            u.fill(96);
            v.fill(160);
        }
        let y = Plane::new(Arc::from(y), 0, w, w, h)?;
        let u = Plane::new(Arc::from(u), 0, chroma_w, chroma_w, chroma_h)?;
        let v = Plane::new(Arc::from(v), 0, chroma_w, chroma_w, chroma_h)?;
        VideoFrame::new(
            self.id,
            [width, height],
            MediaTime::new(ticks as i64, 1_000_000)?,
            MediaTime::new((1_000_000.0 / frame_rate).round() as i64, 1_000_000)?,
            generation,
            VideoPixels::I420 { y, u, v },
        )
    }

    pub fn pcm(
        &self,
        settings: &Settings,
        first_frame: u64,
        frames: usize,
        silent: bool,
        generation: Generation,
    ) -> Result<Pcm, Error> {
        let Settings::Audio {
            sample_rate,
            channel_count,
        } = *settings
        else {
            return Err(Error::state("video settings cannot produce PCM"));
        };
        if frames == 0 || frames > MAX_BLOCK_FRAMES {
            return Err(Error::state("fake PCM request exceeds the block limit"));
        }
        first_frame
            .checked_add(frames as u64)
            .ok_or_else(|| Error::state("fake PCM position overflow"))?;
        let mut samples = vec![0.0_f32; frames * usize::from(channel_count)];
        if !silent {
            for (index, channels) in samples
                .chunks_exact_mut(usize::from(channel_count))
                .enumerate()
            {
                let sample = (first_frame + index as u64) % u64::from(sample_rate);
                let phase = sample as f64 * 440.0 / f64::from(sample_rate);
                let value = (std::f64::consts::TAU * phase).sin() as f32 * 0.25;
                channels.fill(value);
            }
        }
        Pcm::new(
            self.id,
            sample_rate,
            channel_count,
            first_frame,
            generation,
            Arc::from(samples),
        )
    }

    pub fn is_muted(&self) -> bool {
        self.muted.load(Ordering::Acquire)
    }
}

pub fn devices(
    video: bool,
    audio: bool,
    output: bool,
    video_granted: bool,
    audio_granted: bool,
) -> Vec<Value> {
    let mut devices = Vec::new();
    if audio {
        devices.push(json!({
            "deviceId": MICROPHONE,
            "groupId": if audio_granted { AUDIO_GROUP } else { "" },
            "kind": "audioinput",
            "label": if audio_granted { Kind::Audio.label() } else { "" },
            "capabilities": Kind::Audio.capabilities()
        }));
    }
    if video {
        devices.push(json!({
            "deviceId": CAMERA,
            "groupId": if video_granted { VIDEO_GROUP } else { "" },
            "kind": "videoinput",
            "label": if video_granted { Kind::Video.label() } else { "" },
            "capabilities": Kind::Video.capabilities()
        }));
    }
    if output {
        devices.push(json!({
            "deviceId": OUTPUT,
            "groupId": if audio_granted { AUDIO_GROUP } else { "" },
            "kind": "audiooutput",
            "label": if audio_granted { "Chuzz deterministic output" } else { "" }
        }));
    }
    devices
}
