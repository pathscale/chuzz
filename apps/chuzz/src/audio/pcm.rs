//! Immutable PCM shared by capture, Web Audio and codec hosts.
//!
//! Timestamps are relative to the page-worker media epoch. Samples are
//! interleaved. Clones share capture storage but own their enabled and stopped
//! state.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

const BLOCKS: usize = 64;

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

pub struct Pcm {
    start: Duration,
    sample_rate: u32,
    channels: u16,
    samples: Arc<[f32]>,
}

impl Pcm {
    pub fn new(
        start: Duration,
        sample_rate: u32,
        channels: u16,
        samples: Arc<[f32]>,
    ) -> Result<Self, String> {
        if !(8_000..=96_000).contains(&sample_rate)
            || !(1..=2).contains(&channels)
            || samples.is_empty()
            || !samples.len().is_multiple_of(usize::from(channels))
            || samples.len() / usize::from(channels) > sample_rate as usize * 120 / 1_000
            || samples.iter().any(|sample| !sample.is_finite())
        {
            return Err("NotSupportedError: invalid PCM block".to_owned());
        }
        Ok(Self {
            start,
            sample_rate,
            channels,
            samples,
        })
    }

    pub fn start(&self) -> Duration {
        self.start
    }

    pub fn end(&self) -> Duration {
        self.start
            + Duration::from_secs_f64(self.frames() as f64 / f64::from(self.sample_rate))
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels)
    }

    pub(crate) fn at(&self, frame: usize, channel: usize, output_channels: u16) -> f32 {
        if frame >= self.frames() {
            return 0.0;
        }
        let channels = usize::from(self.channels);
        if channels == 2 && output_channels == 1 {
            let left = f64::from(self.samples[frame * 2]);
            let right = f64::from(self.samples[frame * 2 + 1]);
            ((left + right) * 0.5) as f32
        } else {
            self.samples[frame * channels + channel.min(channels - 1)]
        }
    }
}

pub(crate) struct Buffer {
    pub(crate) sample_rate: u32,
    pub(crate) channels: u16,
    pub(crate) producer: Option<(u64, u64)>,
    ended: AtomicBool,
    blocks: Mutex<VecDeque<Arc<Pcm>>>,
}

impl Buffer {
    pub(crate) fn new(
        sample_rate: u32,
        channels: u16,
        producer: Option<(u64, u64)>,
    ) -> Arc<Self> {
        Arc::new(Self {
            sample_rate,
            channels,
            producer,
            ended: AtomicBool::new(false),
            blocks: Mutex::new(VecDeque::new()),
        })
    }

    pub(crate) fn push(&self, pcm: Arc<Pcm>) -> Result<(), String> {
        if pcm.sample_rate() != self.sample_rate || pcm.channels() != self.channels {
            return Err("NotSupportedError: PCM format changed".to_owned());
        }
        let mut blocks = lock(&self.blocks);
        if self.ended.load(Ordering::Acquire) {
            return Err("InvalidStateError: PCM source ended".to_owned());
        }
        if let Some(previous) = blocks.back()
            && (pcm.start() <= previous.start()
                || pcm.start() + Duration::from_micros(1) < previous.end())
        {
            return Err("InvalidStateError: PCM timestamps overlap".to_owned());
        }
        if blocks.len() == BLOCKS {
            blocks.pop_front();
        }
        blocks.push_back(pcm);
        Ok(())
    }

    pub(crate) fn snapshot(&self) -> Vec<Arc<Pcm>> {
        lock(&self.blocks).iter().cloned().collect()
    }

    pub(crate) fn close(&self) {
        let mut blocks = lock(&self.blocks);
        self.ended.store(true, Ordering::Release);
        blocks.clear();
    }
}

pub struct Track {
    pub(crate) buffer: Arc<Buffer>,
    enabled: AtomicBool,
    stopped: AtomicBool,
}

impl Track {
    pub fn new(sample_rate: u32, channels: u16) -> Result<Arc<Self>, String> {
        if !(8_000..=96_000).contains(&sample_rate) || !(1..=2).contains(&channels) {
            return Err("NotSupportedError: unsupported audio track format".to_owned());
        }
        Ok(Self::from_buffer(Buffer::new(sample_rate, channels, None)))
    }

    pub(crate) fn from_buffer(buffer: Arc<Buffer>) -> Arc<Self> {
        Arc::new(Self {
            buffer,
            enabled: AtomicBool::new(true),
            stopped: AtomicBool::new(false),
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.buffer.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.buffer.channels
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
    }

    pub fn ended(&self) -> bool {
        self.stopped.load(Ordering::Acquire) || self.buffer.ended.load(Ordering::Acquire)
    }

    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
    }

    pub fn fork(&self) -> Arc<Self> {
        Arc::new(Self {
            buffer: Arc::clone(&self.buffer),
            enabled: AtomicBool::new(self.enabled()),
            stopped: AtomicBool::new(self.ended()),
        })
    }

    /// Capture ownership is separate from one track's lifecycle. Stopping the
    /// original track does not prevent the capture owner feeding live clones.
    pub fn push(&self, pcm: Arc<Pcm>) -> Result<(), String> {
        self.buffer.push(pcm)
    }

    /// The capture owner calls this when the shared source actually ends.
    pub fn end_source(&self) {
        self.buffer.close();
    }
}
