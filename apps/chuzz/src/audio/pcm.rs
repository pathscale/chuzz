//! Native PCM registrations over the shared media contract.
//!
//! Graph outputs retain bounded Arc-backed history. Microphone registrations
//! pull synchronous capture at the requested media-clock position.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

pub use crate::media::pcm::Pcm;
use crate::media::track::{Settings, Track as MediaTrack};

const BLOCKS: usize = 64;

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
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
        if !pcm.generation.is_current() {
            return Err("InvalidStateError: PCM generation ended".to_owned());
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
    media: Option<Weak<MediaTrack>>,
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
            media: None,
            enabled: AtomicBool::new(true),
            stopped: AtomicBool::new(false),
        })
    }

    pub(crate) fn from_media(track: &Arc<MediaTrack>) -> Result<Arc<Self>, String> {
        let Settings::Audio {
            sample_rate,
            channel_count,
        } = track.settings().map_err(|error| error.message)?
        else {
            return Err("NotSupportedError: the media track is not audio".to_owned());
        };
        if track.is_stopped() {
            return Err("InvalidStateError: the media track has ended".to_owned());
        }
        let buffer = if let Some(audio) = track.source.audio() {
            Arc::clone(&audio.buffer)
        } else {
            Buffer::new(sample_rate, channel_count, None)
        };
        Ok(Arc::new(Self {
            buffer,
            media: Some(Arc::downgrade(track)),
            enabled: AtomicBool::new(true),
            stopped: AtomicBool::new(false),
        }))
    }

    pub(crate) fn media(&self) -> Option<Arc<MediaTrack>> {
        self.media.as_ref().and_then(Weak::upgrade)
    }

    pub fn sample_rate(&self) -> u32 {
        if let Some(track) = self.media()
            && let Ok(Settings::Audio { sample_rate, .. }) = track.settings()
        {
            return sample_rate;
        }
        self.buffer.sample_rate
    }

    pub fn channels(&self) -> u16 {
        if let Some(track) = self.media()
            && let Ok(Settings::Audio { channel_count, .. }) = track.settings()
        {
            return channel_count;
        }
        self.buffer.channels
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
            && self.media.as_ref().is_none_or(|media| {
                media.upgrade().is_some_and(|track| {
                    track.enabled.load(Ordering::Acquire) && !track.source.is_muted()
                })
            })
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
    }

    pub fn ended(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
            || self.buffer.ended.load(Ordering::Acquire)
            || self.media.as_ref().is_some_and(|media| {
                media.upgrade().is_none_or(|track| track.is_stopped())
            })
    }

    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
    }

    pub fn fork(&self) -> Arc<Self> {
        Arc::new(Self {
            buffer: Arc::clone(&self.buffer),
            media: self.media.clone(),
            enabled: AtomicBool::new(self.enabled()),
            stopped: AtomicBool::new(self.ended()),
        })
    }

    /// Capture ownership is separate from one track's lifecycle. Stopping the
    /// original track does not prevent the capture owner feeding live clones.
    pub fn push(&self, pcm: Arc<Pcm>) -> Result<(), String> {
        self.buffer.push(pcm)
    }

    pub(crate) fn read(
        &self,
        start: Duration,
        frames: usize,
        output_rate: u32,
        output_channels: u16,
    ) -> Vec<f32> {
        super::resample::read(self, start, frames, output_rate, output_channels)
    }

    /// The capture owner calls this when the shared source actually ends.
    pub fn end_source(&self) {
        self.buffer.close();
    }
}

