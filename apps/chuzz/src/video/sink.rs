//! Device-free, bounded PCM playout with measured post-mix output.

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;

use crate::media::track::{Settings, Track};

const RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
const BLOCK: usize = 480;
const MAX_POLL_FRAMES: usize = 12_000;

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Stats {
    frames: u64,
    samples: u64,
    energy: f64,
    consumed_seconds: f64,
    underruns: u64,
    dropped_frames: u64,
    sample_rate: u32,
    channels: u16,
}

pub(super) struct HeadlessSink {
    remainder: f64,
    stats: Stats,
}

impl Default for HeadlessSink {
    fn default() -> Self {
        Self {
            remainder: 0.0,
            stats: Stats {
                sample_rate: RATE,
                channels: CHANNELS,
                ..Stats::default()
            },
        }
    }
}

impl HeadlessSink {
    pub(super) fn stats(&self) -> &Stats {
        &self.stats
    }

    pub(super) fn consume(
        &mut self,
        tracks: &[Arc<Track>],
        elapsed: Duration,
        end: Duration,
        gain: f32,
    ) {
        if tracks.is_empty() {
            self.remainder = 0.0;
            return;
        }
        let exact = elapsed.as_secs_f64() * f64::from(RATE) + self.remainder;
        let wanted = exact.floor() as usize;
        self.remainder = exact.fract();
        let count = wanted.min(MAX_POLL_FRAMES);
        if wanted > count {
            self.stats.underruns += 1;
            self.stats.dropped_frames += (wanted - count) as u64;
        }
        let start = (end.as_secs_f64() - count as f64 / f64::from(RATE)).max(0.0);
        for offset in (0..count).step_by(BLOCK) {
            let frames = BLOCK.min(count - offset);
            let time = start + offset as f64 / f64::from(RATE);
            let mut mixed = vec![0.0_f32; frames * usize::from(CHANNELS)];
            for track in tracks {
                let Ok(Settings::Audio { sample_rate, .. }) = track.settings() else {
                    self.stats.underruns += 1;
                    continue;
                };
                let position = time * f64::from(sample_rate);
                let first = position.floor() as u64;
                let fraction = position.fract();
                let ratio = f64::from(sample_rate) / f64::from(RATE);
                let needed = (fraction + (frames - 1) as f64 * ratio).floor() as usize + 2;
                let Ok(pcm) = track.pcm_at(first, needed) else {
                    self.stats.underruns += 1;
                    continue;
                };
                for (index, output) in mixed.chunks_exact_mut(usize::from(CHANNELS)).enumerate() {
                    let position = fraction + index as f64 * ratio;
                    let frame = position.floor() as usize;
                    let fraction = position.fract() as f32;
                    for (channel, value) in output.iter_mut().enumerate() {
                        let a = pcm.at(frame, channel, CHANNELS);
                        let b = pcm.at(frame + 1, channel, CHANNELS);
                        *value += a + (b - a) * fraction;
                    }
                }
            }
            for sample in mixed {
                let sample = f64::from((sample * gain).clamp(-1.0, 1.0));
                self.stats.energy += sample * sample;
            }
            self.stats.frames += frames as u64;
            self.stats.samples += frames as u64 * u64::from(CHANNELS);
        }
        self.stats.consumed_seconds = self.stats.frames as f64 / f64::from(RATE);
    }
}
