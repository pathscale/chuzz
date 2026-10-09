//! The only media layer allowed to open an OS audio device.
//!
//! CPAL owns device callbacks. Decode tasks only supply bounded shared PCM.
//! Muted output consumes the same PCM timeline without opening a device.

use std::sync::Arc;
use std::time::Instant;

use super::MediaError;
use super::backend::{Shared, State};

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

pub(super) enum Output {
    Silent {
        last: Instant,
        remainder: f64,
    },
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    Device(cpal::Stream),
}

impl Output {
    pub fn open(shared: Arc<Shared>, silent: bool) -> Result<Self, MediaError> {
        if silent {
            return Ok(Self::Silent {
                last: Instant::now(),
                remainder: 0.0,
            });
        }
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        {
            let device = cpal::default_host()
                .default_output_device()
                .ok_or_else(|| MediaError::decode("no audio output device is available"))?;
            let range = device
                .supported_output_configs()
                .map_err(|error| MediaError::decode(format!("audio configurations: {error}")))?
                .find(|range| {
                    matches!(range.channels(), 1 | 2)
                        && matches!(
                            range.sample_format(),
                            cpal::SampleFormat::F32
                                | cpal::SampleFormat::I16
                                | cpal::SampleFormat::U16
                        )
                })
                .ok_or_else(|| MediaError::unsupported("no supported PCM output configuration"))?;
            let rate = 48000_u32.clamp(range.min_sample_rate().0, range.max_sample_rate().0);
            let supported = range.with_sample_rate(cpal::SampleRate(rate));
            let format = supported.sample_format();
            let config: cpal::StreamConfig = supported.into();
            let stream = match format {
                cpal::SampleFormat::F32 => build::<f32>(&device, &config, shared)?,
                cpal::SampleFormat::I16 => build::<i16>(&device, &config, shared)?,
                cpal::SampleFormat::U16 => build::<u16>(&device, &config, shared)?,
                _ => return Err(MediaError::unsupported("unsupported PCM output format")),
            };
            stream
                .play()
                .map_err(|error| MediaError::decode(format!("start audio output: {error}")))?;
            Ok(Self::Device(stream))
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            let _ = shared;
            Err(MediaError::unsupported(
                "audible output is unsupported on this platform",
            ))
        }
    }

    pub fn configure(&mut self, paused: bool) -> Result<(), MediaError> {
        match self {
            Self::Silent { last, remainder } => {
                *last = Instant::now();
                *remainder = 0.0;
                Ok(())
            }
            #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
            Self::Device(stream) => {
                let result = if paused {
                    stream.pause().map_err(|error| error.to_string())
                } else {
                    stream.play().map_err(|error| error.to_string())
                };
                result
                    .map_err(|error| MediaError::decode(format!("configure audio output: {error}")))
            }
        }
    }

    pub fn poll(&mut self, shared: &Shared) -> Result<(), MediaError> {
        if let Self::Silent { last, remainder } = self {
            let now = Instant::now();
            let elapsed = now.duration_since(*last).as_secs_f64();
            *last = now;
            let mut state = shared.lock()?;
            if !state.runnable() {
                *remainder = 0.0;
                return Ok(());
            }
            let frames = elapsed * 48000.0 + *remainder;
            *remainder = frames.fract();
            // A suspended document cannot consume an unbounded callback's
            // worth of PCM in one poll. Backpressure freezes its clock.
            for _ in 0..(frames as usize).min(12000) {
                let mut silence = [0.0_f32; 2];
                render_frame(&mut state, 48000, &mut silence);
            }
            publish_clock(&mut state, 0.0);
        }
        Ok(())
    }
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    shared: Arc<Shared>,
) -> Result<cpal::Stream, MediaError>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = usize::from(config.channels);
    let rate = config.sample_rate.0;
    let errors = Arc::clone(&shared);
    device
        .build_output_stream(
            config,
            move |output: &mut [T], information: &cpal::OutputCallbackInfo| {
                output.fill(T::from_sample(0.0));
                if shared.stopped.load(std::sync::atomic::Ordering::Acquire) {
                    return;
                }
                // A device callback never waits for a decoder or DOM poll.
                let Some(mut state) = shared.try_lock() else {
                    return;
                };
                if !state.runnable() {
                    return;
                }
                for frame in output.chunks_exact_mut(channels) {
                    let mut values = [0.0_f32; 2];
                    render_frame(&mut state, rate, &mut values[..channels]);
                    for (output, value) in frame.iter_mut().zip(values) {
                        *output = T::from_sample(value);
                    }
                }
                let timestamp = information.timestamp();
                let latency = timestamp
                    .playback
                    .duration_since(&timestamp.callback)
                    .map_or(0.0, |duration| duration.as_secs_f64());
                publish_clock(&mut state, latency);
            },
            move |error| {
                if let Ok(mut state) = errors.lock() {
                    state.error = Some(MediaError::decode(format!("audio output: {error}")));
                }
            },
            None,
        )
        .map_err(|error| MediaError::decode(format!("create audio output: {error}")))
}

fn render_frame(state: &mut State, output_rate: u32, output: &mut [f32]) {
    output.fill(0.0);
    if !state.runnable() {
        return;
    }
    while state
        .audio
        .front()
        .is_some_and(|pcm| pcm.end() <= state.audio_cursor)
    {
        state.audio.pop_front();
    }
    let duration = state.duration();
    // Keep feeding silence after the final submitted sample until the
    // latency-compensated presentation clock reaches the media duration.
    let end = if state.video_done && state.audio_done {
        duration + 1.0
    } else {
        state.video_end().min(state.audio_end())
    };
    if state.audio_cursor >= end {
        return;
    }
    if let Some(pcm) = state.audio.front() {
        if !state.muted {
            let output_channels = output.len();
            for (channel, value) in output.iter_mut().enumerate() {
                *value =
                    pcm.sample(state.audio_cursor, channel, output_channels) * state.volume as f32;
            }
        }
    } else if !state.audio_done {
        return;
    }
    state.audio_cursor = (state.audio_cursor + state.rate / f64::from(output_rate)).min(end);
}

fn publish_clock(state: &mut State, latency: f64) {
    if state.runnable() {
        let presented = (state.audio_cursor - latency * state.rate).max(state.target);
        state.position = state.position.max(presented).min(state.duration());
    }
}
