//! Decode str0m's assembled Annex B access units without MP4 demuxing.
//!
//! str0m owns mode-1 single NAL, STAP-A and FU-A reassembly. The negotiated
//! constrained-baseline profile excludes B pictures, so flushing at the
//! access-unit boundary preserves its RTP timestamp without reorder latency.

use std::sync::Arc;

use ps_rust_h264::decoder::OrderedDecoder;
use ps_rust_h264::nal::parse_annex_b;

use crate::media::frame::{Generation, MediaTime, Plane, VideoFrame, VideoPixels};

const ACCESS_UNIT_LIMIT: usize = 512 * 1024;

pub(super) struct Receiver {
    decoder: OrderedDecoder,
    waiting_idr: bool,
    previous_timestamp: Option<u64>,
}

pub(super) struct Decoded {
    pub frames: Vec<VideoFrame>,
    pub keyframe: bool,
}

fn failure(error: impl std::fmt::Display) -> String {
    format!("OperationError: H.264 decode: {error}")
}

fn contains(annex: &[u8], kind: u8) -> bool {
    annex
        .windows(4)
        .any(|bytes| bytes[..3] == [0, 0, 1] && bytes[3] & 31 == kind)
}

impl Receiver {
    pub(super) fn new() -> Self {
        Self {
            decoder: OrderedDecoder::with_max_depth(1),
            waiting_idr: true,
            previous_timestamp: None,
        }
    }

    pub(super) fn decode(
        &mut self,
        annex: &[u8],
        source: u64,
        timestamp: u64,
        generation: Generation,
    ) -> Result<Decoded, String> {
        if annex.is_empty() || annex.len() > ACCESS_UNIT_LIMIT {
            return Err("OperationError: H.264 access unit exceeds its limit".into());
        }
        let keyframe = contains(annex, 5);
        if self.waiting_idr && !keyframe {
            return Ok(Decoded {
                frames: Vec::new(),
                keyframe: false,
            });
        }
        let mut pictures = Vec::new();
        for nal in parse_annex_b(annex) {
            pictures.extend(self.decoder.decode_nal(&nal).map_err(failure)?);
        }
        pictures.extend(self.decoder.flush());
        if pictures.len() > 1 {
            return Err("OperationError: multiple pictures in one RTP access unit".into());
        }
        let duration = self
            .previous_timestamp
            .and_then(|previous| timestamp.checked_sub(previous))
            .filter(|ticks| (1..=90_000).contains(ticks))
            .unwrap_or(9000);
        let mut frames = Vec::with_capacity(pictures.len());
        for picture in pictures {
            let width = picture.width;
            let height = picture.height;
            let w = width as usize;
            let h = height as usize;
            if !(2..=1920).contains(&width)
                || !(2..=1080).contains(&height)
                || !width.is_multiple_of(2)
                || !height.is_multiple_of(2)
                || picture.y.len() != w * h
                || picture.u.len() != w * h / 4
                || picture.v.len() != w * h / 4
            {
                return Err("OperationError: unsupported H.264 picture dimensions".into());
            }
            let y = Plane::new(Arc::from(picture.y), 0, w, w, h)
                .map_err(|error| failure(error.message))?;
            let u = Plane::new(Arc::from(picture.u), 0, w / 2, w / 2, h / 2)
                .map_err(|error| failure(error.message))?;
            let v = Plane::new(Arc::from(picture.v), 0, w / 2, w / 2, h / 2)
                .map_err(|error| failure(error.message))?;
            frames.push(
                VideoFrame::new(
                    source,
                    [width, height],
                    MediaTime::new(i64::try_from(timestamp).map_err(failure)?, 90_000)
                        .map_err(|error| failure(error.message))?,
                    MediaTime::new(duration as i64, 90_000)
                        .map_err(|error| failure(error.message))?,
                    generation.clone(),
                    VideoPixels::I420 { y, u, v },
                )
                .map_err(|error| failure(error.message))?,
            );
        }
        if !frames.is_empty() {
            self.waiting_idr = false;
            self.previous_timestamp = Some(timestamp);
        }
        Ok(Decoded { frames, keyframe })
    }
}
