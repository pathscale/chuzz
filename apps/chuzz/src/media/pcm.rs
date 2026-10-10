//! Immutable, interleaved floating-point PCM shared by capture and output.

use std::sync::Arc;

use super::Error;
use super::frame::{Generation, MediaTime};

pub const MAX_BLOCK_FRAMES: usize = 12_000;

#[derive(Clone, Debug)]
pub struct Pcm {
    pub source: u64,
    pub sample_rate: u32,
    pub channels: u16,
    pub first_frame: u64,
    pub presentation: MediaTime,
    pub generation: Generation,
    samples: Arc<[f32]>,
}

impl Pcm {
    pub fn new(
        source: u64,
        sample_rate: u32,
        channels: u16,
        first_frame: u64,
        generation: Generation,
        samples: Arc<[f32]>,
    ) -> Result<Self, Error> {
        let channel_count = usize::from(channels);
        if !(8000..=192_000).contains(&sample_rate)
            || !(1..=32).contains(&channels)
            || samples.is_empty()
            || !samples.len().is_multiple_of(channel_count)
            || samples.len() / channel_count > MAX_BLOCK_FRAMES
            || samples.iter().any(|sample| !sample.is_finite())
        {
            return Err(Error::state("invalid shared PCM block"));
        }
        let frames = samples.len() / channel_count;
        first_frame
            .checked_add(frames as u64)
            .ok_or_else(|| Error::state("PCM sample position overflow"))?;
        let ticks = i64::try_from(first_frame)
            .map_err(|_| Error::state("PCM timestamp exceeds the media clock"))?;
        Ok(Self {
            source,
            sample_rate,
            channels,
            first_frame,
            presentation: MediaTime::new(ticks, sample_rate)?,
            generation,
            samples,
        })
    }

    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels)
    }

    pub fn duration(&self) -> MediaTime {
        MediaTime {
            ticks: self.frames() as i64,
            rate: self.sample_rate,
        }
    }

    pub fn energy(&self) -> f64 {
        self.samples
            .iter()
            .map(|sample| f64::from(*sample).powi(2))
            .sum()
    }

    pub fn rms(&self) -> f64 {
        (self.energy() / self.samples.len() as f64).sqrt()
    }
}
