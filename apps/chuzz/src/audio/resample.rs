//! Timestamp-based, windowed-sinc PCM resampling.
//!
//! Output positions come from the output clock, rather than an accumulated
//! floating-point phase. Immutable input blocks are retained through Arc.

use std::f64::consts::PI;
use std::sync::Arc;
use std::time::Duration;

use super::pcm::{Pcm, Track};

const RADIUS: i32 = 16;

fn input_sample(blocks: &[Arc<Pcm>], time: f64, channel: usize, output_channels: u16) -> f64 {
    for block in blocks {
        let offset = (time - block.start().as_secs_f64()) * f64::from(block.sample_rate());
        let frame = offset.round();
        if frame >= 0.0 && frame < block.frames() as f64 && (offset - frame).abs() < 0.01 {
            return f64::from(block.at(frame as usize, channel, output_channels));
        }
    }
    0.0
}

fn weight(distance: f64, cutoff: f64) -> f64 {
    if distance.abs() >= f64::from(RADIUS) {
        return 0.0;
    }
    let argument = PI * distance * cutoff;
    let sinc = if argument.abs() < 1.0e-12 {
        cutoff
    } else {
        argument.sin() / (PI * distance)
    };
    let window = 0.5 + 0.5 * (PI * distance / f64::from(RADIUS)).cos();
    sinc * window
}

pub(crate) fn read(
    track: &Track,
    start: Duration,
    frames: usize,
    output_rate: u32,
    output_channels: u16,
) -> Vec<f32> {
    let mut output = vec![0.0; frames * usize::from(output_channels)];
    read_into(track, start, output_rate, output_channels, &mut output);
    output
}

pub(crate) fn read_into(
    track: &Track,
    start: Duration,
    output_rate: u32,
    output_channels: u16,
    output: &mut [f32],
) {
    output.fill(0.0);
    if output_channels == 0
        || output_rate == 0
        || !output.len().is_multiple_of(usize::from(output_channels))
        || track.ended()
        || !track.enabled()
    {
        return;
    }
    let frames = output.len() / usize::from(output_channels);

    let input_rate = f64::from(track.sample_rate());
    let target_rate = f64::from(output_rate);
    let cutoff = (target_rate / input_rate).min(1.0);
    // Different context clocks can run in either order. Leave enough history
    // for an input quantum and the symmetric interpolation filter.
    let latency =
        2.0 * (128.0 / target_rate).max(128.0 / input_rate) + f64::from(RADIUS) / input_rate;
    let blocks = if let Some(media) = track.media()
        && media.source.is_capture()
    {
        // Capture is synchronous and deterministic. Pull only the window
        // needed by this render quantum, including the interpolation taps.
        let first = ((start.as_secs_f64() - latency) * input_rate)
            .floor()
            .max(0.0) as u64;
        let first = first.saturating_sub(RADIUS as u64);
        let last = ((start.as_secs_f64() + frames as f64 / target_rate - latency) * input_rate)
            .ceil()
            .max(0.0) as u64;
        let last = last.saturating_add(RADIUS as u64 + 1);
        let length = last.saturating_sub(first);
        let Ok(length) = usize::try_from(length) else {
            return;
        };
        let Ok(pcm) = media.pcm_at(first, length) else {
            return;
        };
        vec![Arc::new(pcm)]
    } else {
        track.buffer.snapshot()
    };

    for frame in 0..frames {
        let time = start.as_secs_f64() + frame as f64 / target_rate - latency;
        let position = time * input_rate;
        let center = position.floor();
        for channel in 0..usize::from(output_channels) {
            let mut sum = 0.0;
            let mut weight_sum = 0.0;
            for tap in -RADIUS..=RADIUS {
                let index = center + f64::from(tap);
                let weight = weight(position - index, cutoff);
                sum += input_sample(&blocks, index / input_rate, channel, output_channels) * weight;
                weight_sum += weight;
            }
            if weight_sum.abs() > 1.0e-12 {
                output[frame * usize::from(output_channels) + channel] =
                    (sum / weight_sum).clamp(-f64::from(f32::MAX), f64::from(f32::MAX)) as f32;
            }
        }
    }
}

/// Convert an already decoded block without the capture/graph clock delay.
/// Edge extension prevents packet boundaries introducing artificial silence.
pub(crate) fn block(pcm: &Pcm, output_rate: u32, output_channels: u16) -> Arc<[f32]> {
    let frames =
        (pcm.frames() as u64 * u64::from(output_rate) / u64::from(pcm.sample_rate())) as usize;
    let input_rate = f64::from(pcm.sample_rate());
    let target_rate = f64::from(output_rate);
    let cutoff = (target_rate / input_rate).min(1.0);
    let mut output = vec![0.0; frames * usize::from(output_channels)];
    for frame in 0..frames {
        let position = frame as f64 * input_rate / target_rate;
        let center = position.floor();
        for channel in 0..usize::from(output_channels) {
            let mut sum = 0.0;
            let mut weight_sum = 0.0;
            for tap in -RADIUS..=RADIUS {
                let index = center + f64::from(tap);
                let weight = weight(position - index, cutoff);
                let index = index.clamp(0.0, (pcm.frames() - 1) as f64) as usize;
                sum += f64::from(pcm.at(index, channel, output_channels)) * weight;
                weight_sum += weight;
            }
            if weight_sum.abs() > 1.0e-12 {
                output[frame * usize::from(output_channels) + channel] = (sum / weight_sum) as f32;
            }
        }
    }
    output.into()
}
