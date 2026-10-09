//! One nagoya decode task per video element, plus a separate AAC task.
//!
//! The codec libraries remain synchronous. Queues, cancellation, seeks and
//! clock ownership belong to this host.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use blitz_traits::net::Url;
use ps_rust_h264::decoder::{Frame as Picture, OrderedDecoder};
use ps_rust_h264::error::DecodeError;
use ps_rust_h264::nal::{parse_annex_b, parse_avcc_config};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_AAC, CodecParameters, DecoderOptions};
use symphonia::core::formats::Packet;

use super::demux::{Movie, Sample};
use super::output::Output;
use super::source::Source;
use super::{Frame, MediaError, Snapshot, VideoDecoder};
use crate::document_loader::NetProvider;

const VIDEO_QUEUE: usize = 6;
const AUDIO_QUEUE: usize = 16;
const WAIT: Duration = Duration::from_millis(4);

pub(super) struct TimedPicture {
    pub start: f64,
    pub end: f64,
    pub picture: Arc<Picture>,
}

pub(super) struct Pcm {
    pub start: f64,
    pub rate: u32,
    pub channels: usize,
    pub samples: Arc<SampleBuffer<f32>>,
}

impl Pcm {
    pub fn end(&self) -> f64 {
        self.start + self.samples.len() as f64 / self.channels as f64 / f64::from(self.rate)
    }

    pub fn sample(&self, time: f64, channel: usize, output_channels: usize) -> f32 {
        if time < self.start || time >= self.end() {
            return 0.0;
        }
        let offset = (time - self.start) * f64::from(self.rate);
        let index = offset.floor() as usize;
        let fraction = (offset - index as f64) as f32;
        let frames = self.samples.len() / self.channels;
        let next = (index + 1).min(frames - 1);
        let values = self.samples.samples();
        let at = |frame: usize| {
            if output_channels == 1 && self.channels == 2 {
                (values[frame * 2] + values[frame * 2 + 1]) * 0.5
            } else {
                values[frame * self.channels + channel.min(self.channels - 1)]
            }
        };
        at(index) + (at(next) - at(index)) * fraction
    }
}

pub(super) struct State {
    pub movie: Option<Arc<Movie>>,
    pub error: Option<MediaError>,
    pub generation: u64,
    pub target: f64,
    pub position: f64,
    pub audio_cursor: f64,
    pub paused: bool,
    pub muted: bool,
    pub volume: f64,
    pub rate: f64,
    pub seeking: bool,
    pub presented: bool,
    pub shown_end: f64,
    pub video: VecDeque<TimedPicture>,
    pub audio: VecDeque<Pcm>,
    pub video_done: bool,
    pub audio_done: bool,
    last_tick: Instant,
}

impl State {
    fn new() -> Self {
        Self {
            movie: None,
            error: None,
            generation: 0,
            target: 0.0,
            position: 0.0,
            audio_cursor: 0.0,
            paused: true,
            muted: false,
            volume: 1.0,
            rate: 1.0,
            seeking: false,
            presented: false,
            shown_end: 0.0,
            video: VecDeque::new(),
            audio: VecDeque::new(),
            video_done: false,
            audio_done: false,
            last_tick: Instant::now(),
        }
    }

    pub fn duration(&self) -> f64 {
        self.movie.as_ref().map_or(0.0, |movie| movie.duration)
    }

    pub fn runnable(&self) -> bool {
        !self.paused && !self.seeking && self.presented && self.rate > 0.0 && self.error.is_none()
    }

    pub fn video_end(&self) -> f64 {
        if self.video_done {
            self.duration()
        } else {
            self.video.back().map_or(self.shown_end, |frame| frame.end)
        }
    }

    pub fn audio_end(&self) -> f64 {
        if self.audio_done {
            self.duration()
        } else {
            self.audio.back().map_or(self.audio_cursor, Pcm::end)
        }
    }

    pub fn has_audio(&self) -> bool {
        self.movie
            .as_ref()
            .is_some_and(|movie| movie.audio.is_some())
    }

    fn wall_tick(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_tick).as_secs_f64();
        self.last_tick = now;
        if !self.has_audio() && self.runnable() {
            self.position = (self.position + elapsed * self.rate)
                .min(self.video_end())
                .min(self.duration());
        }
    }
}

pub(super) struct Shared {
    pub stopped: Arc<AtomicBool>,
    state: Mutex<State>,
}

impl Shared {
    pub fn lock(&self) -> Result<MutexGuard<'_, State>, MediaError> {
        self.state
            .lock()
            .map_err(|_| MediaError::decode("media decode state was poisoned"))
    }

    pub fn try_lock(&self) -> Option<MutexGuard<'_, State>> {
        self.state.try_lock().ok()
    }

    fn active(&self, generation: u64) -> Result<bool, MediaError> {
        Ok(!self.stopped.load(Ordering::Acquire) && self.lock()?.generation == generation)
    }

    fn fail(&self, generation: u64, error: MediaError) {
        if let Ok(mut state) = self.lock()
            && state.generation == generation
            && state.error.is_none()
            && !self.stopped.load(Ordering::Acquire)
        {
            state.error = Some(error);
        }
    }
}

pub(super) struct Backend {
    shared: Arc<Shared>,
    output: Option<Output>,
    source: Arc<Mutex<Option<Arc<Source>>>>,
}

pub(super) fn open(net: Arc<NetProvider>, url: Url, _preload: &str) -> Backend {
    let shared = Arc::new(Shared {
        stopped: Arc::new(AtomicBool::new(false)),
        state: Mutex::new(State::new()),
    });
    let source = Arc::new(Mutex::new(None));
    let task_shared = Arc::clone(&shared);
    let task_source = Arc::clone(&source);
    let _ = nagoya::spawn(async move {
        let result = initialise(&task_shared, &task_source, net, url).await;
        if let Err(error) = result
            && let Ok(state) = task_shared.lock()
        {
            let generation = state.generation;
            drop(state);
            task_shared.fail(generation, error);
        }
    });
    Backend {
        shared,
        output: None,
        source,
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::Release);
        self.output = None;
    }
}

impl VideoDecoder for Backend {
    fn configure(&mut self, paused: bool, muted: bool, volume: f64, rate: f64) {
        let route_changed = if let Ok(mut state) = self.shared.lock() {
            state.wall_tick();
            let changed = (state.muted || state.volume == 0.0) != (muted || volume == 0.0);
            state.paused = paused;
            state.muted = muted;
            state.volume = volume;
            state.rate = rate;
            if changed || paused {
                state.audio_cursor = state.position;
            }
            changed
        } else {
            false
        };
        if route_changed {
            self.output = None;
        }
        if let Some(output) = self.output.as_mut()
            && let Err(error) = output.configure(paused)
            && let Ok(mut state) = self.shared.lock()
        {
            state.error = Some(error);
        }
    }

    fn seek(&mut self, time: f64) -> Result<(), MediaError> {
        self.output = None;
        let mut state = self.shared.lock()?;
        state.generation = state
            .generation
            .checked_add(1)
            .ok_or_else(|| MediaError::decode("media seek generation overflow"))?;
        let time = state
            .movie
            .as_ref()
            .map_or(time.max(0.0), |movie| time.clamp(0.0, movie.duration));
        state.target = time;
        state.position = time;
        state.audio_cursor = time;
        state.seeking = true;
        state.presented = false;
        state.shown_end = time;
        state.video.clear();
        state.audio.clear();
        state.video_done = false;
        state.audio_done = !state.has_audio();
        state.last_tick = Instant::now();
        Ok(())
    }

    fn poll(&mut self) -> Result<Snapshot, MediaError> {
        if let Some(output) = self.output.as_mut() {
            output.poll(&self.shared)?;
        }

        let (snapshot, needs_output, silent) = {
            let mut state = self.shared.lock()?;
            if let Some(error) = &state.error {
                return Err(error.clone());
            }
            state.wall_tick();
            let mut selected = None;
            if !state.presented {
                selected = state.video.pop_front();
            } else {
                while state
                    .video
                    .front()
                    .is_some_and(|frame| frame.start <= state.position)
                {
                    selected = state.video.pop_front();
                }
            }
            let frame = selected.map(|frame| {
                state.presented = true;
                state.seeking = false;
                state.shown_end = frame.end;
                Frame {
                    width: frame.picture.width,
                    height: frame.picture.height,
                    picture: frame.picture,
                }
            });
            let video_end = state.video_end();
            let end = if state.has_audio() {
                video_end.min(state.audio_end())
            } else {
                video_end
            };
            let waiting = !state.presented
                || state.seeking
                || state.position + 0.001 >= end && state.position < state.duration() - 0.001;
            let buffered = if state.presented && end > state.position {
                vec![[state.position, end.min(state.duration())]]
            } else {
                Vec::new()
            };
            let metadata = state.movie.as_ref().filter(|_| state.presented);
            let snapshot = Snapshot {
                duration: metadata.map(|movie| movie.duration),
                position: state.position,
                width: frame.as_ref().map_or_else(
                    || metadata.map_or(0, |movie| movie.video.width),
                    |frame| frame.width,
                ),
                height: frame.as_ref().map_or_else(
                    || metadata.map_or(0, |movie| movie.video.height),
                    |frame| frame.height,
                ),
                buffered,
                seekable: metadata.map_or_else(Vec::new, |movie| vec![[0.0, movie.duration]]),
                seeking: state.seeking,
                waiting,
                playing: state.runnable() && !waiting,
                enough: state.presented && state.video_done && state.audio_done,
                frame,
                src: String::new(),
            };
            (
                snapshot,
                state.has_audio() && state.presented && !state.paused,
                state.muted || state.volume == 0.0,
            )
        };

        if needs_output && self.output.is_none() {
            self.output = Some(Output::open(Arc::clone(&self.shared), silent)?);
        }
        let mut snapshot = snapshot;
        let source = self
            .source
            .lock()
            .map_err(|_| MediaError::network("media source state was poisoned"))?
            .clone();
        if let Some(source) = source {
            snapshot.src = source.status()?;
        }
        Ok(snapshot)
    }
}

async fn initialise(
    shared: &Arc<Shared>,
    slot: &Mutex<Option<Arc<Source>>>,
    net: Arc<NetProvider>,
    url: Url,
) -> Result<(), MediaError> {
    let source = Source::open(net, url, Arc::clone(&shared.stopped)).await?;
    *slot
        .lock()
        .map_err(|_| MediaError::network("media source state was poisoned"))? =
        Some(Arc::clone(&source));
    let movie = Movie::read(&source).await?;
    {
        let mut state = shared.lock()?;
        state.target = state.target.min(movie.duration);
        state.position = state.target;
        state.audio_cursor = state.target;
        state.audio_done = movie.audio.is_none();
        state.movie = Some(Arc::clone(&movie));
    }
    if movie.audio.is_some() {
        let shared = Arc::clone(shared);
        let source = Arc::clone(&source);
        let movie = Arc::clone(&movie);
        let _ = nagoya::spawn(async move {
            let _ = audio_worker(&shared, &source, &movie).await;
        });
    }
    video_worker(shared, &source, &movie).await
}

async fn video_worker(shared: &Shared, source: &Source, movie: &Movie) -> Result<(), MediaError> {
    loop {
        if shared.stopped.load(Ordering::Acquire) {
            return Ok(());
        }
        let (generation, target) = {
            let state = shared.lock()?;
            (state.generation, state.target)
        };
        if let Err(error) = decode_video(shared, source, movie, generation, target).await {
            shared.fail(generation, error);
        }
        while shared.active(generation)? {
            nagoya::sleep(WAIT).await;
        }
    }
}

async fn decode_video(
    shared: &Shared,
    source: &Source,
    movie: &Movie,
    generation: u64,
    target: f64,
) -> Result<(), MediaError> {
    let table = &movie.video.table.samples;
    let start = table
        .iter()
        .enumerate()
        .filter(|(_, sample)| sample.sync && sample.pts <= target)
        .map(|(index, _)| index)
        .next_back()
        .or_else(|| table.iter().position(|sample| sample.sync))
        .ok_or_else(|| MediaError::decode("the H.264 track has no sync sample"))?;
    let mut display = table[start..].to_vec();
    display.sort_by(|left, right| left.pts.total_cmp(&right.pts));
    let mut display = display.into_iter();
    let config = parse_avcc_config(&movie.video.avcc)
        .map_err(|error| MediaError::decode(format!("H.264 configuration: {error}")))?;
    if config.sps_nals.is_empty() || config.pps_nals.is_empty() {
        return Err(MediaError::decode(
            "H.264 configuration has no parameter sets",
        ));
    }
    let mut decoder = OrderedDecoder::new();
    for nal in config.sps_nals.iter().chain(&config.pps_nals) {
        decoder.decode_nal(nal).map_err(video_error)?;
    }
    let mut annex = Vec::new();
    for sample in &table[start..] {
        if !shared.active(generation)? {
            return Ok(());
        }
        let bytes = source.read(sample.offset, sample.size).await?;
        annex_b(&bytes, config.length_size, &mut annex)?;
        let mut pictures = Vec::new();
        for nal in parse_annex_b(&annex) {
            pictures.extend(decoder.decode_nal(&nal).map_err(video_error)?);
        }
        for picture in pictures {
            let timing = display
                .next()
                .ok_or_else(|| MediaError::decode("too many decoded H.264 pictures"))?;
            if !queue_picture(shared, generation, target, timing, picture).await? {
                return Ok(());
            }
        }
        nagoya::yield_now().await;
    }
    for picture in decoder.flush() {
        let timing = display
            .next()
            .ok_or_else(|| MediaError::decode("too many decoded H.264 pictures"))?;
        if !queue_picture(shared, generation, target, timing, picture).await? {
            return Ok(());
        }
    }
    if display.next().is_some() {
        return Err(MediaError::decode(
            "the H.264 track did not decode every picture",
        ));
    }
    let mut state = shared.lock()?;
    if state.generation == generation {
        if !state.presented && state.video.is_empty() {
            return Err(MediaError::decode(
                "the H.264 track produced no displayable frame",
            ));
        }
        state.video_done = true;
    }
    Ok(())
}

async fn queue_picture(
    shared: &Shared,
    generation: u64,
    target: f64,
    timing: Sample,
    picture: Picture,
) -> Result<bool, MediaError> {
    if timing.end() <= target && target < timing.end() + timing.duration {
        // The last frame remains presentable when seeking exactly to EOF.
        if target < shared.lock()?.duration() {
            return Ok(true);
        }
    } else if timing.end() <= target {
        return Ok(true);
    }
    let width = picture.width as usize;
    let height = picture.height as usize;
    let pixels = width
        .checked_mul(height)
        .filter(|pixels| {
            width >= 2 && height >= 2 && width % 2 == 0 && height % 2 == 0 && *pixels <= 3840 * 2160
        })
        .ok_or_else(|| MediaError::unsupported("unsupported decoded video dimensions"))?;
    if picture.y.len() != pixels || picture.u.len() != pixels / 4 || picture.v.len() != pixels / 4 {
        return Err(MediaError::decode("invalid decoded H.264 planes"));
    }
    let mut frame = Some(TimedPicture {
        start: timing.pts,
        end: timing.end(),
        picture: Arc::new(picture),
    });
    loop {
        if !shared.active(generation)? {
            return Ok(false);
        }
        {
            let mut state = shared.lock()?;
            if state.generation != generation {
                return Ok(false);
            }
            if state.video.len() < VIDEO_QUEUE {
                if let Some(frame) = frame.take() {
                    state.video.push_back(frame);
                }
                return Ok(true);
            }
        }
        nagoya::sleep(WAIT).await;
    }
}

fn annex_b(sample: &[u8], length_size: usize, output: &mut Vec<u8>) -> Result<(), MediaError> {
    if !matches!(length_size, 1 | 2 | 4) {
        return Err(MediaError::decode("invalid H.264 NAL length size"));
    }
    output.clear();
    let mut position = 0_usize;
    while position < sample.len() {
        let prefix = sample
            .get(position..position + length_size)
            .ok_or_else(|| MediaError::decode("truncated H.264 NAL length"))?;
        let length = prefix
            .iter()
            .fold(0_usize, |length, byte| (length << 8) | usize::from(*byte));
        position += length_size;
        let end = position
            .checked_add(length)
            .ok_or_else(|| MediaError::decode("H.264 NAL length overflow"))?;
        let nal = sample
            .get(position..end)
            .filter(|nal| !nal.is_empty() && nal[0] & 0x80 == 0)
            .ok_or_else(|| MediaError::decode("invalid H.264 NAL extent or header"))?;
        output.extend_from_slice(&[0, 0, 0, 1]);
        output.extend_from_slice(nal);
        position = end;
    }
    if output.is_empty() {
        return Err(MediaError::decode("empty H.264 access unit"));
    }
    Ok(())
}

fn video_error(error: DecodeError) -> MediaError {
    match error {
        DecodeError::Unsupported(message) => MediaError::unsupported(message),
        error => MediaError::decode(format!("H.264: {error}")),
    }
}

async fn audio_worker(shared: &Shared, source: &Source, movie: &Movie) -> Result<(), MediaError> {
    loop {
        if shared.stopped.load(Ordering::Acquire) {
            return Ok(());
        }
        let (generation, target) = {
            let state = shared.lock()?;
            (state.generation, state.target)
        };
        if let Err(error) = decode_audio(shared, source, movie, generation, target).await {
            shared.fail(generation, error);
        }
        while shared.active(generation)? {
            nagoya::sleep(WAIT).await;
        }
    }
}

async fn decode_audio(
    shared: &Shared,
    source: &Source,
    movie: &Movie,
    generation: u64,
    target: f64,
) -> Result<(), MediaError> {
    let track = movie
        .audio
        .as_ref()
        .ok_or_else(|| MediaError::decode("the audio track was released"))?;
    let mut parameters = CodecParameters::new();
    parameters
        .for_codec(CODEC_TYPE_AAC)
        .with_sample_rate(track.sample_rate)
        .with_extra_data(track.asc.clone().into_boxed_slice());
    let mut decoder = symphonia::default::get_codecs()
        .make(&parameters, &DecoderOptions::default())
        .map_err(audio_error)?;
    let start = track
        .table
        .samples
        .iter()
        .rposition(|sample| sample.pts <= target)
        .map_or(0, |index| index.saturating_sub(2));
    for sample in &track.table.samples[start..] {
        if !shared.active(generation)? {
            return Ok(());
        }
        let bytes = source.read(sample.offset, sample.size).await?;
        // Symphonia 0.5 packets own their payload. This is its required input
        // copy; the decoded PCM allocation is shared without another copy.
        let packet = Packet::new_from_slice(0, sample.dts, u64::from(sample.delta), &bytes);
        let decoded = decoder.decode(&packet).map_err(audio_error)?;
        let spec = *decoded.spec();
        let channels = spec.channels.count();
        if !(1..=2).contains(&channels)
            || !(8000..=192000).contains(&spec.rate)
            || decoded.capacity() > 32768
        {
            return Err(MediaError::unsupported("unsupported decoded AAC geometry"));
        }
        let mut samples = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        samples.copy_interleaved_ref(decoded);
        if samples.is_empty() {
            continue;
        }
        let pcm = Pcm {
            start: sample.pts,
            rate: spec.rate,
            channels,
            samples: Arc::new(samples),
        };
        if pcm.end() <= target {
            continue;
        }
        let mut pcm = Some(pcm);
        loop {
            if !shared.active(generation)? {
                return Ok(());
            }
            let queued = {
                let mut state = shared.lock()?;
                if state.generation != generation {
                    return Ok(());
                }
                if state.audio.len() < AUDIO_QUEUE {
                    if let Some(pcm) = pcm.take() {
                        state.audio.push_back(pcm);
                    }
                    true
                } else {
                    false
                }
            };
            if queued {
                break;
            }
            nagoya::sleep(WAIT).await;
        }
        nagoya::yield_now().await;
    }
    let mut state = shared.lock()?;
    if state.generation == generation {
        state.audio_done = true;
    }
    Ok(())
}

fn audio_error(error: symphonia::core::errors::Error) -> MediaError {
    match error {
        symphonia::core::errors::Error::Unsupported(message) => MediaError::unsupported(message),
        error => MediaError::decode(format!("AAC: {error}")),
    }
}
