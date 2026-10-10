//! Synchronous Web Audio processing. Scheduling belongs to the page host.

use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::PI;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::media::frame::Cancellation;

use super::analysis::Analysis;
use super::pcm::{Buffer, Pcm, Track};
use super::resample;

pub(crate) const QUANTUM: usize = 128;
const NODES: usize = 256;
const AUTOMATION_EVENTS: usize = 128;

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum EventKind {
    Set,
    Linear,
    Exponential,
    Target,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Event {
    kind: EventKind,
    time: f64,
    value: f64,
    #[serde(default)]
    time_constant: f64,
}

struct Param {
    default: f64,
    events: Vec<Event>,
}

impl Param {
    fn new(default: f64) -> Self {
        Self {
            default,
            events: Vec::new(),
        }
    }

    fn value(&self, time: f64) -> f64 {
        let mut anchor_time = 0.0;
        let mut anchor_value = self.default;
        let mut target: Option<(f64, f64, f64, f64)> = None;

        let held = |at: f64, value: f64, target: Option<(f64, f64, f64, f64)>| {
            if let Some((start, initial, goal, constant)) = target {
                goal + (initial - goal) * (-(at - start) / constant).exp()
            } else {
                value
            }
        };

        for event in &self.events {
            if time < event.time {
                if matches!(event.kind, EventKind::Linear | EventKind::Exponential)
                    && event.time > anchor_time
                    && target.is_none()
                {
                    let fraction =
                        ((time - anchor_time) / (event.time - anchor_time)).clamp(0.0, 1.0);
                    return if event.kind == EventKind::Exponential
                        && anchor_value > 0.0
                        && event.value > 0.0
                    {
                        anchor_value * (event.value / anchor_value).powf(fraction)
                    } else {
                        anchor_value + (event.value - anchor_value) * fraction
                    };
                }
                return held(time, anchor_value, target);
            }

            let previous = held(event.time, anchor_value, target);
            anchor_time = event.time;
            match event.kind {
                EventKind::Target => {
                    target = Some((event.time, previous, event.value, event.time_constant));
                    anchor_value = previous;
                }
                _ => {
                    anchor_value = event.value;
                    target = None;
                }
            }
        }
        held(time, anchor_value, target)
    }

    fn insert(&mut self, event: Event) -> Result<(), String> {
        if !event.time.is_finite()
            || event.time < 0.0
            || !event.value.is_finite()
            || event.value.abs() > f64::from(f32::MAX)
            || (event.kind == EventKind::Exponential && event.value <= 0.0)
            || (event.kind == EventKind::Target
                && (!event.time_constant.is_finite() || event.time_constant <= 0.0))
        {
            return Err("RangeError: invalid audio automation event".to_owned());
        }
        if self.events.len() == AUTOMATION_EVENTS {
            return Err("QuotaExceededError: audio automation event limit".to_owned());
        }
        let index = self
            .events
            .partition_point(|existing| existing.time <= event.time);
        self.events.insert(index, event);
        Ok(())
    }

    fn cancel(&mut self, time: f64, hold: bool) -> Result<(), String> {
        if !time.is_finite() || time < 0.0 {
            return Err("RangeError: invalid automation cancellation time".to_owned());
        }
        let value = self.value(time);
        let interrupted = self
            .events
            .iter()
            .find(|event| event.time > time)
            .filter(|event| matches!(event.kind, EventKind::Linear | EventKind::Exponential))
            .map(|event| event.kind);
        self.events.retain(|event| event.time < time);
        if hold {
            if let Some(kind) = interrupted {
                self.insert(Event {
                    kind,
                    time,
                    value,
                    time_constant: 0.0,
                })?;
            }
            self.insert(Event {
                kind: EventKind::Set,
                time,
                value,
                time_constant: 0.0,
            })?;
        }
        Ok(())
    }
}

#[derive(Clone)]
enum Kind {
    Destination,
    Oscillator,
    Gain,
    Analyser,
    Source(Arc<Track>),
    StreamDestination(Arc<Buffer>),
}

struct Node {
    kind: Kind,
    params: BTreeMap<String, Param>,
    analysis: Option<Analysis>,
    wave: String,
    phase: f64,
    start: Option<f64>,
    stop: f64,
    ended: bool,
}

impl Node {
    fn new(kind: Kind) -> Self {
        let mut params = BTreeMap::new();
        if matches!(&kind, Kind::Gain) {
            params.insert("gain".to_owned(), Param::new(1.0));
        }
        if matches!(&kind, Kind::Oscillator) {
            params.insert("frequency".to_owned(), Param::new(440.0));
            params.insert("detune".to_owned(), Param::new(0.0));
        }
        let analysis = matches!(&kind, Kind::Analyser).then(Analysis::default);
        Self {
            kind,
            params,
            analysis,
            wave: "sine".to_owned(),
            phase: 0.0,
            start: None,
            stop: f64::INFINITY,
            ended: false,
        }
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Edge {
    from: u64,
    to: u64,
    param: Option<String>,
}

pub(crate) struct Graph {
    pub(crate) id: u64,
    pub(crate) sample_rate: u32,
    pub(crate) running: bool,
    pub(crate) closed: bool,
    cursor: u64,
    next_node: u64,
    nodes: BTreeMap<u64, Node>,
    edges: BTreeSet<Edge>,
    samples: u64,
    sum_squares: f64,
    underruns: u64,
    cancellation: Cancellation,
}

impl Graph {
    pub(crate) fn new(id: u64, sample_rate: u32) -> Self {
        Self {
            id,
            sample_rate,
            running: false,
            closed: false,
            cursor: 0,
            next_node: 2,
            nodes: BTreeMap::from([(1, Node::new(Kind::Destination))]),
            edges: BTreeSet::new(),
            samples: 0,
            sum_squares: 0.0,
            underruns: 0,
            cancellation: Cancellation::default(),
        }
    }

    fn open(&self) -> Result<(), String> {
        if self.closed {
            Err("InvalidStateError: AudioContext is closed".to_owned())
        } else {
            Ok(())
        }
    }

    pub(crate) fn time(&self) -> f64 {
        self.cursor as f64 / f64::from(self.sample_rate)
    }

    pub(crate) fn inspect(&self) -> Value {
        json!({
            "currentTime": self.time(),
            "sampleRate": self.sample_rate,
            "state": if self.closed {
                "closed"
            } else if self.running {
                "running"
            } else {
                "suspended"
            },
            "samples": self.samples,
            "sumSquares": self.sum_squares,
            "energy": if self.samples == 0 {
                0.0
            } else {
                self.sum_squares / self.samples as f64
            },
            "underruns": self.underruns,
        })
    }

    pub(crate) fn create(
        &mut self,
        kind: &str,
        track: Option<Arc<Track>>,
    ) -> Result<(u64, Option<Arc<Track>>), String> {
        self.open()?;
        if self.nodes.len() == NODES {
            return Err("QuotaExceededError: audio node limit".to_owned());
        }
        let id = self.next_node;
        let mut output = None;
        let kind = match kind {
            "oscillator" => Kind::Oscillator,
            "gain" => Kind::Gain,
            "analyser" => Kind::Analyser,
            "source" => {
                Kind::Source(track.ok_or("InvalidAccessError: unknown audio stream track")?)
            }
            "streamDestination" => {
                let buffer = Buffer::new(self.sample_rate, 2, Some((self.id, id)));
                output = Some(Track::from_buffer(Arc::clone(&buffer)));
                Kind::StreamDestination(buffer)
            }
            _ => return Err("NotSupportedError: unsupported audio node".to_owned()),
        };
        self.next_node += 1;
        self.nodes.insert(id, Node::new(kind));
        Ok((id, output))
    }

    fn reaches(&self, start: u64, target: u64) -> bool {
        let mut pending = vec![start];
        let mut visited = BTreeSet::new();
        while let Some(id) = pending.pop() {
            if id == target {
                return true;
            }
            if !visited.insert(id) {
                continue;
            }
            pending.extend(
                self.edges
                    .iter()
                    .filter(|edge| edge.from == id)
                    .map(|edge| edge.to),
            );
            for (&source, node) in &self.nodes {
                if let Kind::Source(track) = &node.kind
                    && track.buffer.producer == Some((self.id, id))
                {
                    pending.push(source);
                }
            }
        }
        false
    }

    pub(crate) fn connect(
        &mut self,
        from: u64,
        to: u64,
        param: Option<String>,
    ) -> Result<(), String> {
        self.open()?;
        let source = self
            .nodes
            .get(&from)
            .ok_or("InvalidAccessError: unknown source node")?;
        let destination = self
            .nodes
            .get(&to)
            .ok_or("InvalidAccessError: unknown destination node")?;
        if matches!(&source.kind, Kind::Destination | Kind::StreamDestination(_)) {
            return Err("IndexSizeError: audio node has no output".to_owned());
        }
        if let Some(name) = &param {
            if !destination.params.contains_key(name) {
                return Err("InvalidAccessError: unknown AudioParam".to_owned());
            }
        } else if matches!(&destination.kind, Kind::Oscillator | Kind::Source(_)) {
            return Err("IndexSizeError: audio node has no input".to_owned());
        }
        let edge = Edge { from, to, param };
        if self.edges.contains(&edge) {
            return Ok(());
        }
        if self.reaches(to, from) {
            return Err("NotSupportedError: audio feedback requires a DelayNode".to_owned());
        }
        self.edges.insert(edge);
        Ok(())
    }

    pub(crate) fn disconnect(
        &mut self,
        from: u64,
        to: Option<u64>,
        param: Option<String>,
    ) -> Result<(), String> {
        self.open()?;
        if !self.nodes.contains_key(&from) {
            return Err("InvalidAccessError: unknown source node".to_owned());
        }
        let before = self.edges.len();
        self.edges.retain(|edge| {
            !(edge.from == from && to.is_none_or(|to| edge.to == to && edge.param == param))
        });
        if to.is_some() && self.edges.len() == before {
            return Err("InvalidAccessError: audio connection does not exist".to_owned());
        }
        Ok(())
    }

    fn param(&mut self, node: u64, name: &str) -> Result<&mut Param, String> {
        self.open()?;
        self.nodes
            .get_mut(&node)
            .and_then(|node| node.params.get_mut(name))
            .ok_or_else(|| "InvalidAccessError: unknown AudioParam".to_owned())
    }

    pub(crate) fn automate(&mut self, node: u64, name: &str, event: Event) -> Result<(), String> {
        self.param(node, name)?.insert(event)
    }

    pub(crate) fn parameter_value(&mut self, node: u64, name: &str) -> Result<f64, String> {
        let time = self.time();
        Ok(self.param(node, name)?.value(time))
    }

    pub(crate) fn cancel(
        &mut self,
        node: u64,
        name: &str,
        time: f64,
        hold: bool,
    ) -> Result<(), String> {
        self.param(node, name)?.cancel(time, hold)
    }

    pub(crate) fn wave(&mut self, id: u64, wave: String) -> Result<(), String> {
        self.open()?;
        if !matches!(wave.as_str(), "sine" | "square" | "sawtooth" | "triangle") {
            return Err("NotSupportedError: unsupported oscillator waveform".to_owned());
        }
        let node = self
            .nodes
            .get_mut(&id)
            .ok_or("InvalidAccessError: unknown oscillator")?;
        if !matches!(&node.kind, Kind::Oscillator) {
            return Err("InvalidAccessError: node is not an oscillator".to_owned());
        }
        node.wave = wave;
        Ok(())
    }

    pub(crate) fn schedule(&mut self, id: u64, start: bool, time: f64) -> Result<(), String> {
        self.open()?;
        if !time.is_finite() || time < 0.0 {
            return Err("RangeError: invalid oscillator time".to_owned());
        }
        let node = self
            .nodes
            .get_mut(&id)
            .ok_or("InvalidAccessError: unknown oscillator")?;
        if !matches!(&node.kind, Kind::Oscillator) {
            return Err("InvalidAccessError: node is not an oscillator".to_owned());
        }
        if start {
            if node.start.is_some() {
                return Err("InvalidStateError: oscillator already started".to_owned());
            }
            node.start = Some(time);
        } else {
            if node.start.is_none() {
                return Err("InvalidStateError: oscillator has not started".to_owned());
            }
            if !node.ended {
                node.stop = time;
            }
        }
        Ok(())
    }

    pub(crate) fn configure(
        &mut self,
        id: u64,
        fft_size: usize,
        min_decibels: f64,
        max_decibels: f64,
        smoothing: f64,
    ) -> Result<(), String> {
        self.open()?;
        self.nodes
            .get_mut(&id)
            .and_then(|node| node.analysis.as_mut())
            .ok_or("InvalidAccessError: node is not an analyser")?
            .configure(fft_size, min_decibels, max_decibels, smoothing)
    }

    pub(crate) fn analysis_bytes(&mut self, id: u64, kind: &str) -> Result<Vec<u8>, String> {
        self.open()?;
        self.nodes
            .get_mut(&id)
            .and_then(|node| node.analysis.as_mut())
            .ok_or("InvalidAccessError: node is not an analyser")?
            .bytes(kind)
    }

    pub(crate) fn close(&mut self) {
        self.closed = true;
        self.running = false;
        self.cancellation.cancel();
        for node in self.nodes.values() {
            if let Kind::StreamDestination(buffer) = &node.kind {
                buffer.close();
            }
        }
        self.nodes.clear();
        self.edges.clear();
    }

    pub(crate) fn skip(&mut self, quanta: u64) {
        self.cursor = self
            .cursor
            .saturating_add(quanta.saturating_mul(QUANTUM as u64));
        self.underruns = self.underruns.saturating_add(quanta);
    }

    fn pcm(&self, start: Duration, channels: u16, samples: Vec<f32>) -> Arc<Pcm> {
        let first_frame = (start.as_secs_f64() * f64::from(self.sample_rate)).round() as u64;
        Arc::new(
            Pcm::new(
                self.id,
                self.sample_rate,
                channels,
                first_frame,
                self.cancellation.generation(),
                Arc::from(samples),
            )
            .expect("rendered audio has a validated format"),
        )
    }

    fn render(
        &mut self,
        id: u64,
        start: Duration,
        rendered: &mut BTreeMap<u64, Arc<Pcm>>,
        ended: &mut Vec<u64>,
    ) -> Arc<Pcm> {
        if let Some(pcm) = rendered.get(&id) {
            return Arc::clone(pcm);
        }

        let kind = self.nodes[&id].kind.clone();
        if let Kind::Source(track) = &kind {
            let pcm = if track.ended() || !track.enabled() {
                self.pcm(
                    start,
                    track.channels(),
                    vec![0.0; QUANTUM * usize::from(track.channels())],
                )
            } else if let Some((context, producer)) = track.buffer.producer
                && context == self.id
            {
                self.render(producer, start, rendered, ended)
            } else {
                self.pcm(
                    start,
                    track.channels(),
                    resample::read(track, start, QUANTUM, self.sample_rate, track.channels()),
                )
            };
            rendered.insert(id, Arc::clone(&pcm));
            return pcm;
        }

        let incoming: Vec<Edge> = self
            .edges
            .iter()
            .filter(|edge| edge.to == id)
            .cloned()
            .collect();
        let mut inputs = Vec::new();
        let mut modulation: BTreeMap<String, Vec<Arc<Pcm>>> = BTreeMap::new();
        for edge in incoming {
            let pcm = self.render(edge.from, start, rendered, ended);
            if let Some(name) = edge.param {
                modulation.entry(name).or_default().push(pcm);
            } else {
                inputs.push(pcm);
            }
        }
        let channels = if matches!(&kind, Kind::Destination | Kind::StreamDestination(_)) {
            2
        } else if matches!(&kind, Kind::Oscillator) {
            1
        } else {
            inputs.iter().map(|pcm| pcm.channels()).max().unwrap_or(1)
        };

        let time = self.time();
        let rate = f64::from(self.sample_rate);
        let node = self.nodes.get_mut(&id).expect("render node exists");
        let parameter = |name: &str, frame: usize| {
            let base = node.params[name].value(time + frame as f64 / rate);
            let additional: f64 = modulation
                .get(name)
                .into_iter()
                .flatten()
                .map(|pcm| f64::from(pcm.at(frame, 0, 1)))
                .sum();
            (base + additional).clamp(-f64::from(f32::MAX), f64::from(f32::MAX))
        };

        let mut samples = vec![0.0; QUANTUM * usize::from(channels)];
        if matches!(&kind, Kind::Oscillator) {
            let mut phase = node.phase;
            for (frame, sample) in samples.iter_mut().enumerate() {
                let at = time + frame as f64 / rate;
                if node.start.is_some_and(|start| at >= start) && at < node.stop {
                    let frequency = parameter("frequency", frame);
                    let detune = parameter("detune", frame).clamp(-153_600.0, 153_600.0);
                    let frequency = if frequency == 0.0 {
                        0.0
                    } else {
                        (frequency * 2.0_f64.powf(detune / 1_200.0)).clamp(-rate / 2.0, rate / 2.0)
                    };
                    *sample = oscillator(&node.wave, phase, frequency.abs() / rate);
                    phase = (phase + frequency / rate).rem_euclid(1.0);
                }
            }
            node.phase = phase;
            if node.start.is_some() && !node.ended && time + QUANTUM as f64 / rate >= node.stop {
                node.ended = true;
                ended.push(id);
            }
        } else {
            for frame in 0..QUANTUM {
                let gain = if matches!(&kind, Kind::Gain) {
                    parameter("gain", frame)
                } else {
                    1.0
                };
                for channel in 0..usize::from(channels) {
                    let sum: f64 = inputs
                        .iter()
                        .map(|pcm| f64::from(pcm.at(frame, channel, channels)))
                        .sum();
                    samples[frame * usize::from(channels) + channel] =
                        (sum * gain).clamp(-f64::from(f32::MAX), f64::from(f32::MAX)) as f32;
                }
            }
        }

        let pcm = self.pcm(start, channels, samples);
        if let Some(analysis) = self
            .nodes
            .get_mut(&id)
            .and_then(|node| node.analysis.as_mut())
        {
            analysis.push(&pcm);
        }
        if let Kind::StreamDestination(buffer) = &kind {
            // The host gives successive quanta increasing media timestamps.
            let _ = buffer.push(Arc::clone(&pcm));
        }
        rendered.insert(id, Arc::clone(&pcm));
        pcm
    }

    pub(crate) fn process(&mut self, start: Duration) -> Vec<u64> {
        if !self.running || self.closed {
            return Vec::new();
        }
        let mut rendered = BTreeMap::new();
        let mut ended = Vec::new();
        // Analysers are roots even when their outputs are disconnected.
        let ids: Vec<u64> = self.nodes.keys().copied().collect();
        for id in ids {
            self.render(id, start, &mut rendered, &mut ended);
        }
        let destination = &rendered[&1];
        self.samples = self
            .samples
            .saturating_add(destination.samples().len() as u64);
        self.sum_squares += destination
            .samples()
            .iter()
            .map(|&sample| f64::from(sample).powi(2))
            .sum::<f64>();
        self.cursor = self.cursor.saturating_add(QUANTUM as u64);
        ended
    }
}

fn poly_blep(phase: f64, step: f64) -> f64 {
    if step == 0.0 {
        0.0
    } else if phase < step {
        let x = phase / step;
        x + x - x * x - 1.0
    } else if phase > 1.0 - step {
        let x = (phase - 1.0) / step;
        x * x + x + x + 1.0
    } else {
        0.0
    }
}

fn oscillator(wave: &str, phase: f64, step: f64) -> f32 {
    let sample = match wave {
        "square" => {
            (if phase < 0.5 { 1.0 } else { -1.0 }) + poly_blep(phase, step)
                - poly_blep((phase + 0.5).rem_euclid(1.0), step)
        }
        "sawtooth" => 2.0 * phase - 1.0 - poly_blep(phase, step),
        "triangle" => {
            let maximum = if step > 0.0 {
                (0.5 / step).floor().min(63.0) as usize
            } else {
                63
            };
            let mut sum = 0.0;
            for harmonic in (1..=maximum).step_by(2) {
                let sign = if (harmonic / 2).is_multiple_of(2) {
                    1.0
                } else {
                    -1.0
                };
                sum +=
                    sign * (2.0 * PI * harmonic as f64 * phase).sin() / (harmonic as f64).powi(2);
            }
            8.0 * sum / PI.powi(2)
        }
        _ => (2.0 * PI * phase).sin(),
    };
    sample as f32
}
