//! Host scheduling around synchronous codecs. The codecs own no tasks or channels.

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ps_rust_h264::decoder::OrderedDecoder;
use ps_rust_h264::nal::{parse_annex_b, parse_avcc_config};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_AAC, CodecParameters, Decoder, DecoderOptions};
use symphonia::core::formats::Packet;

use super::{Input, Presentation, View};
use crate::video::backend::{self, Backend, Pcm, Shared, TimedPicture};
use crate::video::demux::{AudioTrack, Movie, Sample, Table, VideoTrack};
use crate::video::fragments::Codec;
use crate::video::{MediaError, Snapshot, VideoDecoder};

const WAIT: Duration = Duration::from_millis(4);

pub(super) struct Playback {
    backend: Backend,
    shared: Arc<Shared>,
    source: Arc<Mutex<Presentation>>,
    epoch: u64,
}

pub(super) fn open(source: Arc<Mutex<Presentation>>) -> Playback {
    let (backend, shared) = backend::streaming();
    let task_shared = Arc::clone(&shared);
    let task_source = Arc::clone(&source);
    let _ = nagoya::spawn(async move {
        if let Err(error) = run(&task_shared, &task_source).await
            && let Ok(state) = task_shared.lock()
        {
            let generation = state.generation;
            drop(state);
            task_shared.fail(generation, error);
        }
    });
    Playback {
        backend,
        shared,
        source,
        epoch: 0,
    }
}

fn view(source: &Mutex<Presentation>) -> Result<View, MediaError> {
    Ok(source
        .lock()
        .map_err(|_| MediaError::decode("MSE source was poisoned"))?
        .view())
}

impl VideoDecoder for Playback {
    fn configure(&mut self, paused: bool, muted: bool, volume: f64, rate: f64) {
        self.backend.configure(paused, muted, volume, rate);
    }

    fn seek(&mut self, time: f64) -> Result<(), MediaError> {
        self.backend.seek(time)
    }

    fn poll(&mut self) -> Result<Snapshot, MediaError> {
        let view = view(&self.source)?;
        if let Some(error) = view.error {
            return Err(error);
        }
        if view.epoch != self.epoch {
            self.epoch = view.epoch;
            let position = self.shared.lock()?.position;
            self.backend.seek(position)?;
        }
        let mut snapshot = self.backend.poll()?;
        snapshot.duration = view.ready.then_some(view.duration);
        if let Some(input) = &view.video
            && let Codec::Video { width, height, .. } = &input.track.codec
        {
            snapshot.width = *width;
            snapshot.height = *height;
        }
        let in_range = view.buffered.iter().any(|range| {
            range[0] <= snapshot.position + 0.000001 && range[1] > snapshot.position + 0.001
        });
        let at_end = view.ended && snapshot.position >= view.duration - 0.001;
        snapshot.waiting = !at_end && (snapshot.waiting || !in_range);
        snapshot.playing &= !snapshot.waiting;
        snapshot.enough &= view.ended;
        snapshot.buffered = view.buffered;
        snapshot.seekable = view.seekable;
        snapshot.end_allowed = view.ended;
        Ok(snapshot)
    }
}

fn metadata(view: &View) -> Arc<Movie> {
    let video = match view.video.as_ref().map(|input| &input.track.codec) {
        Some(Codec::Video {
            avcc,
            width,
            height,
        }) => VideoTrack {
            table: Table {
                samples: Vec::new(),
                end: 0.0,
            },
            avcc: avcc.clone(),
            width: *width,
            height: *height,
        },
        _ => VideoTrack {
            table: Table {
                samples: Vec::new(),
                end: 0.0,
            },
            avcc: Vec::new(),
            width: 0,
            height: 0,
        },
    };
    let audio = match view.audio.as_ref().map(|input| &input.track.codec) {
        Some(Codec::Audio { asc, rate }) => Some(AudioTrack {
            table: Table {
                samples: Vec::new(),
                end: 0.0,
            },
            asc: asc.clone(),
            sample_rate: *rate,
        }),
        _ => None,
    };
    Arc::new(Movie {
        video,
        audio,
        duration: view.duration,
    })
}

fn video_start(input: &Input, target: f64) -> usize {
    input
        .packets
        .iter()
        .enumerate()
        .filter(|(_, packet)| packet.sample.sync && packet.sample.pts <= target)
        .map(|(index, _)| index)
        .next_back()
        .or_else(|| input.packets.iter().position(|packet| packet.sample.sync))
        .unwrap_or(input.packets.len())
}

fn audio_start(input: &Input, target: f64) -> usize {
    input
        .packets
        .iter()
        .rposition(|packet| packet.sample.pts <= target)
        .map_or(0, |index| index.saturating_sub(2))
}

fn picture(
    timing: Sample,
    target: f64,
    picture: ps_rust_h264::decoder::Frame,
) -> Result<Option<TimedPicture>, MediaError> {
    let pixels = (picture.width as usize)
        .checked_mul(picture.height as usize)
        .filter(|pixels| {
            picture.width >= 2
                && picture.height >= 2
                && picture.width % 2 == 0
                && picture.height % 2 == 0
                && *pixels <= 3840 * 2160
        })
        .ok_or_else(|| MediaError::unsupported("unsupported MSE video dimensions"))?;
    if picture.y.len() != pixels || picture.u.len() != pixels / 4 || picture.v.len() != pixels / 4 {
        return Err(MediaError::decode("invalid MSE H.264 planes"));
    }
    Ok((timing.end() > target).then(|| TimedPicture {
        start: timing.pts,
        end: timing.end(),
        picture: Arc::new(picture),
    }))
}

async fn run(shared: &Arc<Shared>, source: &Mutex<Presentation>) -> Result<(), MediaError> {
    let mut previous = None;
    let mut video: Option<OrderedDecoder> = None;
    let mut audio: Option<Box<dyn Decoder>> = None;
    let mut video_index = 0;
    let mut audio_index = 0;
    let mut length_size = 0;
    let mut timings = Vec::new();
    let mut pictures = VecDeque::new();
    let mut flushed = false;
    let mut annex = Vec::new();
    loop {
        if shared.stopped.load(Ordering::Acquire) {
            return Ok(());
        }
        let view = view(source)?;
        if view.closed {
            return Ok(());
        }
        if let Some(error) = view.error {
            return Err(error);
        }
        if !view.ready {
            nagoya::sleep(WAIT).await;
            continue;
        }
        let (generation, target) = {
            let state = shared.lock()?;
            (state.generation, state.target)
        };
        let reset = previous != Some((generation, view.epoch));
        if reset {
            previous = Some((generation, view.epoch));
            timings.clear();
            pictures.clear();
            flushed = false;
            video = None;
            audio = None;
            if let Some(input) = &view.video {
                let Codec::Video { avcc, .. } = &input.track.codec else {
                    return Err(MediaError::decode("invalid MSE video track"));
                };
                let config = parse_avcc_config(avcc)
                    .map_err(|error| MediaError::decode(format!("MSE avcC: {error}")))?;
                if config.sps_nals.is_empty() || config.pps_nals.is_empty() {
                    return Err(MediaError::decode("MSE avcC has no parameter sets"));
                }
                length_size = config.length_size;
                // Presentation times that never go back in decode order mean no B-frames: release each picture
                // as soon as it decodes, or the last ~16 frames of every append would wait for the next one.
                let reordered = input
                    .packets
                    .windows(2)
                    .any(|pair| pair[1].sample.pts < pair[0].sample.pts);
                let mut decoder = if reordered {
                    OrderedDecoder::new()
                } else {
                    OrderedDecoder::with_max_depth(1)
                };
                for nal in config.sps_nals.iter().chain(&config.pps_nals) {
                    decoder.decode_nal(nal).map_err(backend::video_error)?;
                }
                video = Some(decoder);
                video_index = video_start(input, target);
            }
            if let Some(input) = &view.audio {
                let Codec::Audio { asc, rate } = &input.track.codec else {
                    return Err(MediaError::decode("invalid MSE audio track"));
                };
                let mut parameters = CodecParameters::new();
                parameters
                    .for_codec(CODEC_TYPE_AAC)
                    .with_sample_rate(*rate)
                    .with_extra_data(asc.clone().into_boxed_slice());
                audio = Some(
                    symphonia::default::get_codecs()
                        .make(&parameters, &DecoderOptions::default())
                        .map_err(backend::audio_error)?,
                );
                audio_index = audio_start(input, target);
            }
            let mut state = shared.lock()?;
            if state.generation != generation {
                continue;
            }
            state.video.clear();
            state.audio.clear();
            state.presented = false;
            state.shown_end = target;
            state.position = target;
            state.audio_cursor = target;
            state.video_done = view.video.is_none();
            state.audio_done = view.audio.is_none();
            state.movie = Some(metadata(&view));
        } else {
            let mut state = shared.lock()?;
            if state
                .movie
                .as_ref()
                .is_none_or(|movie| movie.duration != view.duration)
            {
                state.movie = Some(metadata(&view));
            }
        }

        let video_room = shared.lock()?.video.len() < 6;
        if video_room
            && pictures.is_empty()
            && let (Some(input), Some(decoder)) = (&view.video, video.as_mut())
        {
            if let Some(packet) = input.packets.get(video_index) {
                timings.push(packet.sample);
                timings.sort_by(|a, b| a.pts.total_cmp(&b.pts));
                backend::annex_b(packet.data(), length_size, &mut annex)?;
                for nal in parse_annex_b(&annex) {
                    for decoded in decoder.decode_nal(&nal).map_err(backend::video_error)? {
                        if timings.is_empty() {
                            return Err(MediaError::decode(
                                "MSE decoded more pictures than samples",
                            ));
                        }
                        let timing = timings.remove(0);
                        if let Some(frame) = picture(timing, target, decoded)? {
                            pictures.push_back(frame);
                        }
                    }
                }
                video_index += 1;
            } else if view.ended && !flushed {
                for decoded in decoder.flush() {
                    if timings.is_empty() {
                        return Err(MediaError::decode("MSE flush has no sample timestamp"));
                    }
                    let timing = timings.remove(0);
                    if let Some(frame) = picture(timing, target, decoded)? {
                        pictures.push_back(frame);
                    }
                }
                if !timings.is_empty() {
                    return Err(MediaError::decode("MSE did not decode every H.264 sample"));
                }
                flushed = true;
            }
        }

        if shared.lock()?.audio.len() < 16
            && let (Some(input), Some(decoder)) = (&view.audio, audio.as_mut())
            && let Some(packet) = input.packets.get(audio_index)
        {
            let sample = packet.sample;
            // Symphonia packets require owned bytes. PCM is shared after decoding.
            let packet =
                Packet::new_from_slice(0, sample.dts, u64::from(sample.delta), packet.data());
            let decoded = decoder.decode(&packet).map_err(backend::audio_error)?;
            let spec = *decoded.spec();
            let channels = spec.channels.count();
            if !(1..=2).contains(&channels)
                || !(8000..=192000).contains(&spec.rate)
                || decoded.capacity() > 32768
            {
                return Err(MediaError::unsupported("unsupported MSE AAC geometry"));
            }
            let mut samples = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
            samples.copy_interleaved_ref(decoded);
            if !samples.is_empty() {
                let pcm = Pcm {
                    start: sample.pts,
                    rate: spec.rate,
                    channels,
                    samples: Arc::new(samples),
                };
                if pcm.end() > target {
                    let mut state = shared.lock()?;
                    if state.generation == generation {
                        state.audio.push_back(pcm);
                    }
                }
            }
            audio_index += 1;
        }

        {
            let mut state = shared.lock()?;
            if state.generation != generation {
                continue;
            }
            while state.video.len() < 6 {
                let Some(picture) = pictures.pop_front() else {
                    break;
                };
                state.video.push_back(picture);
            }
            state.video_done = view.video.as_ref().is_none_or(|input| {
                view.ended && flushed && video_index == input.packets.len() && pictures.is_empty()
            });
            state.audio_done = view
                .audio
                .as_ref()
                .is_none_or(|input| view.ended && audio_index == input.packets.len());
        }
        nagoya::sleep(WAIT).await;
    }
}
