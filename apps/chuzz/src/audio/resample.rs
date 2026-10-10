//! Timestamp-based, windowed-sinc PCM resampling.
//!
//! Output positions come from the output clock, rather than an accumulated
//! floating-point phase. Immutable input blocks are retained through Arc.

use std::f64::consts::PI;
use std::sync::Arc;
use std::time::Duration;

use super::pcm::{Pcm, Track};

const RADIUS: i32 = 16;

fn input_sample(
    blocks: &[Arc<Pcm>],
    time: f64,
    channel: usize,
    output_channels: u16,
) -> f64 {
    for block in blocks {
        let offset = (time - block.start().as_secs_f64()) * f64::from(block.sample_rate());
        let frame = offset.round();
        if frame >= 0.0 && frame < block.frames() as f64 && (offset - frame).abs() < 0.01 {
            return f64::from(block.at(frame as usize, channel, output_channels));
        }
    }
    0.0
}

pub(crate) fn read(
    track: &Track,
    start: Duration,
    frames: usize,
    output_rate: u32,
    output_channels: u16,
) -> Vec<f32> {
    let mut output = vec![0.0; frames * usize::from(output_channels)];
    if track.ended() || !track.enabled() {
        return output;
    }

    let blocks = track.buffer.snapshot();
    let input_rate = f64::from(track.sample_rate());
    let target_rate = f64::from(output_rate);
    let cutoff = (target_rate / input_rate).min(1.0);
    // Different context clocks can run in either order. Leave enough history
    // for an input quantum and the symmetric interpolation filter.
    let latency = 2.0 * (128.0 / target_rate).max(128.0 / input_rate)
        + f64::from(RADIUS) / input_rate;

    for frame in 0..frames {
        let time = start.as_secs_f64() + frame as f64 / target_rate - latency;
        let position = time * input_rate;
        let center = position.floor();
        for channel in 0..usize::from(output_channels) {
            let mut sum = 0.0;
            let mut weight_sum = 0.0;
            for tap in -RADIUS..=RADIUS {
                let index = center + f64::from(tap);
                let distance = position - index;
                if distance.abs() >= f64::from(RADIUS) {
                    continue;
                }
                let argument = PI * distance * cutoff;
                let sinc = if argument.abs() < 1.0e-12 {
                    cutoff
                } else {
                    argument.sin() / (PI * distance)
                };
                let window = 0.5 + 0.5 * (PI * distance / f64::from(RADIUS)).cos();
                let weight = sinc * window;
                sum += input_sample(
                    &blocks,
                    index / input_rate,
                    channel,
                    output_channels,
                ) * weight;
                weight_sum += weight;
            }
            if weight_sum.abs() > 1.0e-12 {
                output[frame * usize::from(output_channels) + channel] =
                    (sum / weight_sum).clamp(-f64::from(f32::MAX), f64::from(f32::MAX)) as f32;
            }
        }
    }
    output
}
