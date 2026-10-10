//! Track-local constraints and lifecycle over a shared capture source.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use super::Error;
use super::fake::{AUDIO_GROUP, CAMERA, MICROPHONE, Source, VIDEO_GROUP};
use super::frame::{Cancellation, VideoFrame};
use super::pcm::Pcm;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Audio,
    Video,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Video => "video",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Audio => "Chuzz deterministic microphone",
            Self::Video => "Chuzz deterministic camera",
        }
    }

    pub fn device(self) -> &'static str {
        match self {
            Self::Audio => MICROPHONE,
            Self::Video => CAMERA,
        }
    }

    pub fn group(self) -> &'static str {
        match self {
            Self::Audio => AUDIO_GROUP,
            Self::Video => VIDEO_GROUP,
        }
    }

    pub fn capabilities(self) -> Value {
        let mut value = match self {
            Self::Video => json!({
                "width": {"min": 16, "max": 320},
                "height": {"min": 16, "max": 240},
                "aspectRatio": {"min": 16.0 / 240.0, "max": 320.0 / 16.0},
                "frameRate": {"min": 1, "max": 30},
                "facingMode": ["user"],
                "resizeMode": ["none"]
            }),
            Self::Audio => json!({
                "sampleRate": {"min": 8000, "max": 48000},
                "sampleSize": {"min": 32, "max": 32},
                "channelCount": {"min": 1, "max": 2},
                "latency": {"min": 0.02, "max": 0.02},
                "echoCancellation": [false],
                "noiseSuppression": [false],
                "autoGainControl": [false]
            }),
        };
        value["deviceId"] = json!(self.device());
        value["groupId"] = json!(self.group());
        value
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Settings {
    Video {
        width: u32,
        height: u32,
        frame_rate: f64,
    },
    Audio {
        sample_rate: u32,
        channel_count: u16,
    },
}

impl Settings {
    pub fn json(self) -> Value {
        let (kind, mut value) = match self {
            Self::Video {
                width,
                height,
                frame_rate,
            } => (
                Kind::Video,
                json!({
                    "width": width, "height": height, "frameRate": frame_rate,
                    "aspectRatio": f64::from(width) / f64::from(height),
                    "facingMode": "user", "resizeMode": "none"
                }),
            ),
            Self::Audio {
                sample_rate,
                channel_count,
            } => (
                Kind::Audio,
                json!({
                    "sampleRate": sample_rate, "sampleSize": 32,
                    "channelCount": channel_count, "latency": 0.02,
                    "echoCancellation": false, "noiseSuppression": false,
                    "autoGainControl": false
                }),
            ),
        };
        value["deviceId"] = json!(kind.device());
        value["groupId"] = json!(kind.group());
        value
    }
}

struct State {
    constraints: Value,
    settings: Settings,
}

pub struct Track {
    pub id: u64,
    pub source: Arc<Source>,
    pub enabled: AtomicBool,
    stopped: AtomicBool,
    cancellation: Cancellation,
    state: Mutex<State>,
}

impl Track {
    pub fn new(id: u64, source: Arc<Source>, constraints: Value, settings: Settings) -> Self {
        Self {
            id,
            source,
            enabled: AtomicBool::new(true),
            stopped: AtomicBool::new(false),
            cancellation: Cancellation::default(),
            state: Mutex::new(State {
                constraints,
                settings,
            }),
        }
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }

    pub fn stop(&self) {
        if !self.stopped.swap(true, Ordering::AcqRel) {
            self.cancellation.cancel();
        }
    }

    pub fn status(&self) -> (bool, bool, bool) {
        (
            self.enabled.load(Ordering::Acquire),
            self.source.is_muted(),
            self.is_stopped(),
        )
    }

    pub fn descriptor(&self) -> Result<Value, Error> {
        let state = self
            .state
            .lock()
            .map_err(|_| Error::state("capture track state was poisoned"))?;
        let (enabled, muted, ended) = self.status();
        Ok(json!({
            "handle": self.id,
            "id": format!("chuzz-track-{}", self.id),
            "kind": self.source.kind.name(),
            "label": self.source.kind.label(),
            "enabled": enabled,
            "muted": muted,
            "readyState": if ended { "ended" } else { "live" },
            "settings": state.settings.json(),
            "capabilities": self.source.kind.capabilities(),
            "constraints": state.constraints
        }))
    }

    pub fn clone_track(&self, id: u64) -> Result<Self, Error> {
        let state = self
            .state
            .lock()
            .map_err(|_| Error::state("capture track state was poisoned"))?;
        let track = Self::new(
            id,
            Arc::clone(&self.source),
            state.constraints.clone(),
            state.settings,
        );
        track
            .enabled
            .store(self.enabled.load(Ordering::Acquire), Ordering::Release);
        if self.is_stopped() {
            track.stop();
        }
        Ok(track)
    }

    pub fn apply(&self, constraints: Value) -> Result<(), Error> {
        if self.is_stopped() {
            return Ok(());
        }
        let settings = resolve(self.source.kind, &constraints)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| Error::state("capture track state was poisoned"))?;
        if !self.is_stopped() {
            *state = State {
                constraints,
                settings,
            };
        }
        Ok(())
    }

    pub fn settings(&self) -> Result<Settings, Error> {
        self.state
            .lock()
            .map(|state| state.settings)
            .map_err(|_| Error::state("capture track state was poisoned"))
    }

    pub fn video_at(&self, at: Duration) -> Result<VideoFrame, Error> {
        let generation = self.cancellation.generation();
        if self.is_stopped() {
            return Err(Error::state("capture track has ended"));
        }
        let silent = !self.enabled.load(Ordering::Acquire) || self.source.is_muted();
        let frame = self
            .source
            .video(&self.settings()?, at, silent, generation)?;
        if !frame.generation.is_current() {
            return Err(Error::state("capture frame was cancelled"));
        }
        Ok(frame)
    }

    pub fn pcm_at(&self, first_frame: u64, frames: usize) -> Result<Pcm, Error> {
        let generation = self.cancellation.generation();
        if self.is_stopped() {
            return Err(Error::state("capture track has ended"));
        }
        let silent = !self.enabled.load(Ordering::Acquire) || self.source.is_muted();
        let pcm = self
            .source
            .pcm(&self.settings()?, first_frame, frames, silent, generation)?;
        if !pcm.generation.is_current() {
            return Err(Error::state("capture PCM was cancelled"));
        }
        Ok(pcm)
    }
}

impl Drop for Track {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Numeric {
    min: f64,
    max: f64,
    ideal: Option<f64>,
    fallback: f64,
    integral: bool,
    touched: bool,
}

impl Numeric {
    fn accepts(&self, value: f64) -> bool {
        value >= self.min - 1e-9 && value <= self.max + 1e-9
    }

    fn pick(&self, key: &str) -> Result<f64, Error> {
        let (min, max) = if self.integral {
            (self.min.ceil(), self.max.floor())
        } else {
            (self.min, self.max)
        };
        if min > max {
            return Err(Error::constraint(key));
        }
        let value = self.ideal.unwrap_or(self.fallback).clamp(min, max);
        Ok(if self.integral { value.round() } else { value })
    }

    fn distance(&self, value: f64) -> f64 {
        let target = self.ideal.unwrap_or(self.fallback);
        let weight = if self.ideal.is_some() { 1.0 } else { 0.001 };
        weight * (value - target).abs() / value.abs().max(target.abs()).max(1e-9)
    }
}

type Set<'a> = (&'a Value, bool);

fn number(value: &Value) -> Result<f64, Error> {
    value
        .as_f64()
        .filter(|value| value.is_finite() && *value >= 0.0)
        .ok_or_else(|| Error::typ("a numeric capture constraint must be finite and nonnegative"))
}

fn numeric(
    sets: &[Set<'_>],
    key: &str,
    range: [f64; 2],
    fallback: f64,
    integral: bool,
) -> Result<Numeric, Error> {
    let mut result = Numeric {
        min: range[0],
        max: range[1],
        ideal: None,
        fallback,
        integral,
        touched: false,
    };
    for &(set, advanced) in sets {
        let Some(value) = set.get(key).filter(|value| !value.is_null()) else {
            continue;
        };
        if value.is_number() {
            let value = number(value)?;
            result.touched = true;
            if advanced {
                result.min = result.min.max(value);
                result.max = result.max.min(value);
            } else {
                result.ideal = Some(value);
            }
        } else if let Some(fields) = value.as_object() {
            for name in ["min", "max", "exact", "ideal"] {
                let Some(value) = fields.get(name) else {
                    continue;
                };
                let value = number(value)?;
                if name != "ideal" || !advanced {
                    result.touched = true;
                }
                match name {
                    "min" => result.min = result.min.max(value),
                    "max" => result.max = result.max.min(value),
                    "exact" => {
                        result.min = result.min.max(value);
                        result.max = result.max.min(value);
                    }
                    "ideal" if !advanced => result.ideal = Some(value),
                    _ => {}
                }
            }
        } else {
            return Err(Error::typ("invalid numeric capture constraint"));
        }
    }
    result.pick(key)?;
    Ok(result)
}

fn scalar_matches(value: &Value, actual: &Value) -> Result<bool, Error> {
    if actual.is_string() {
        if let Some(values) = value.as_array() {
            if values.iter().any(|value| !value.is_string()) {
                return Err(Error::typ(
                    "a device constraint sequence must contain strings",
                ));
            }
            return Ok(values.contains(actual));
        }
        if !value.is_string() {
            return Err(Error::typ(
                "a device constraint must be a string or string sequence",
            ));
        }
    } else if !value.is_boolean() {
        return Err(Error::typ("an audio processing constraint must be boolean"));
    }
    Ok(value == actual)
}

fn scalar(sets: &[Set<'_>], key: &str, actual: Value) -> Result<(), Error> {
    for &(set, advanced) in sets {
        let Some(value) = set.get(key).filter(|value| !value.is_null()) else {
            continue;
        };
        if let Some(fields) = value.as_object() {
            if let Some(exact) = fields.get("exact")
                && !scalar_matches(exact, &actual)?
            {
                return Err(Error::constraint(key));
            }
            if let Some(ideal) = fields.get("ideal") {
                scalar_matches(ideal, &actual)?;
            }
        } else {
            let matches = scalar_matches(value, &actual)?;
            if advanced && !matches {
                return Err(Error::constraint(key));
            }
        }
    }
    Ok(())
}

fn select(kind: Kind, sets: &[Set<'_>]) -> Result<Settings, Error> {
    scalar(sets, "deviceId", json!(kind.device()))?;
    scalar(sets, "groupId", json!(kind.group()))?;
    match kind {
        Kind::Audio => {
            for key in ["echoCancellation", "noiseSuppression", "autoGainControl"] {
                scalar(sets, key, json!(false))?;
            }
            numeric(sets, "sampleSize", [32.0, 32.0], 32.0, true)?;
            numeric(sets, "latency", [0.02, 0.02], 0.02, false)?;
            let rate = numeric(sets, "sampleRate", [8000.0, 48000.0], 48000.0, true)?;
            let channels = numeric(sets, "channelCount", [1.0, 2.0], 1.0, true)?;
            Ok(Settings::Audio {
                sample_rate: rate.pick("sampleRate")? as u32,
                channel_count: channels.pick("channelCount")? as u16,
            })
        }
        Kind::Video => {
            scalar(sets, "facingMode", json!("user"))?;
            scalar(sets, "resizeMode", json!("none"))?;
            let width = numeric(sets, "width", [16.0, 320.0], 64.0, true)?;
            let height = numeric(sets, "height", [16.0, 240.0], 48.0, true)?;
            let rate = numeric(sets, "frameRate", [1.0, 30.0], 10.0, false)?;
            let ratio = numeric(
                sets,
                "aspectRatio",
                [16.0 / 240.0, 320.0 / 16.0],
                64.0 / 48.0,
                false,
            )?;
            let (w, h) = if ratio.touched {
                let mut best: Option<(f64, u32, u32)> = None;
                for w in width.min.ceil() as u32..=width.max.floor() as u32 {
                    for h in height.min.ceil() as u32..=height.max.floor() as u32 {
                        let aspect = f64::from(w) / f64::from(h);
                        if !ratio.accepts(aspect) {
                            continue;
                        }
                        let score = width.distance(f64::from(w))
                            + height.distance(f64::from(h))
                            + ratio.distance(aspect);
                        if best.is_none_or(|previous| score < previous.0) {
                            best = Some((score, w, h));
                        }
                    }
                }
                let (_, w, h) = best.ok_or_else(|| Error::constraint("aspectRatio"))?;
                (w, h)
            } else {
                (width.pick("width")? as u32, height.pick("height")? as u32)
            };
            Ok(Settings::Video {
                width: w,
                height: h,
                frame_rate: rate.pick("frameRate")?,
            })
        }
    }
}

/// Basic exact/min/max constraints are mandatory; bare values are preferences.
/// Advanced sets are applied in order only when the entire set is satisfiable.
pub fn resolve(kind: Kind, constraints: &Value) -> Result<Settings, Error> {
    if !constraints.is_object() {
        return Err(Error::typ("capture constraints must be a dictionary"));
    }
    let mut sets = vec![(constraints, false)];
    let mut selected = select(kind, &sets)?;
    if let Some(advanced) = constraints.get("advanced") {
        let advanced = advanced
            .as_array()
            .filter(|sets| sets.len() <= 32)
            .ok_or_else(|| Error::typ("advanced must contain at most 32 dictionaries"))?;
        for set in advanced {
            if !set.is_object() {
                return Err(Error::typ(
                    "an advanced constraint set must be a dictionary",
                ));
            }
            sets.push((set, true));
            match select(kind, &sets) {
                Ok(settings) => selected = settings,
                Err(error) if error.name == "OverconstrainedError" => {
                    sets.pop();
                }
                Err(error) => return Err(error),
            }
        }
    }
    Ok(selected)
}
