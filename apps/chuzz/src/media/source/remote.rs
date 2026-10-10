//! A receiver retains one immutable decoded frame shared by all track clones.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::media::frame::{Generation, Plane, VideoFrame, VideoPixels};
use crate::media::track::{Settings, Track};
use crate::media::{Error, Host, TRACK_LIMIT, allocate};

use super::Source;

#[derive(Default)]
pub struct Video {
    ended: AtomicBool,
    frame: Mutex<Option<VideoFrame>>,
}

impl Video {
    pub fn ended(&self) -> bool {
        self.ended.load(Ordering::Acquire)
    }

    pub fn end(&self) {
        self.ended.store(true, Ordering::Release);
        if let Ok(mut frame) = self.frame.lock() {
            *frame = None;
        }
    }

    pub fn ready(&self) -> bool {
        !self.ended()
            && self.frame.lock().is_ok_and(|frame| {
                frame
                    .as_ref()
                    .is_some_and(|frame| frame.generation.is_current())
            })
    }

    pub fn dimensions(&self) -> Option<[u32; 2]> {
        self.frame
            .lock()
            .ok()?
            .as_ref()
            .map(|frame| [frame.width, frame.height])
    }

    pub fn publish(&self, frame: VideoFrame) -> Result<(), Error> {
        if self.ended() || !frame.generation.is_current() {
            return Err(Error::state("remote video frame was cancelled"));
        }
        let mut current = self
            .frame
            .lock()
            .map_err(|_| Error::state("remote video state was poisoned"))?;
        if self.ended() {
            return Err(Error::state("remote video source has ended"));
        }
        *current = Some(frame);
        Ok(())
    }

    pub fn frame(&self, generation: Generation, silent: bool) -> Result<VideoFrame, Error> {
        let mut frame = self
            .frame
            .lock()
            .map_err(|_| Error::state("remote video state was poisoned"))?
            .as_ref()
            .filter(|frame| frame.generation.is_current())
            .cloned()
            .ok_or_else(|| Error::state("remote video has no decoded frame"))?;
        if self.ended() || !generation.is_current() {
            return Err(Error::state("remote video source has ended"));
        }
        frame.generation = generation;
        if !silent {
            return Ok(frame);
        }
        let width = frame.width as usize;
        let height = frame.height as usize;
        let chroma_width = width.div_ceil(2);
        let chroma_height = height.div_ceil(2);
        let y = Plane::new(
            Arc::from(vec![16; width * height]),
            0,
            width,
            width,
            height,
        )?;
        let u = Plane::new(
            Arc::from(vec![128; chroma_width * chroma_height]),
            0,
            chroma_width,
            chroma_width,
            chroma_height,
        )?;
        VideoFrame::new(
            frame.source,
            [frame.width, frame.height],
            frame.presentation,
            frame.duration,
            frame.generation,
            VideoPixels::I420 {
                y,
                v: u.clone(),
                u,
            },
        )
    }
}

impl Host {
    pub fn video_receiver(&self, video: Arc<Video>) -> Result<Value, Error> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| Error::state("capture host state was poisoned"))?;
        if store.closed {
            return Err(Error::state("capture document has closed"));
        }
        if store.tracks.len() >= TRACK_LIMIT {
            return Err(Error::named(
                "QuotaExceededError",
                "too many live capture tracks",
            ));
        }
        let source = Arc::new(Source::from_video(video, self.origin)?);
        let track = Arc::new(Track::new(
            allocate()?,
            source,
            json!({}),
            Settings::Video {
                width: 64,
                height: 48,
                frame_rate: 10.0,
            },
        ));
        let descriptor = track.descriptor()?;
        store.tracks.insert(track.id, track);
        Ok(descriptor)
    }
}

