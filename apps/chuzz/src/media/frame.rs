//! Shared media ownership contracts. Transport and demux remain separate.
//!
//! Timestamps retain their original clock rate. Consumers explicitly map them
//! onto the document timeline. Byte ranges and planes lease immutable storage.

use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaTime {
    pub ticks: i64,
    pub rate: u32,
}

impl MediaTime {
    pub fn new(ticks: i64, rate: u32) -> Result<Self, Error> {
        if rate == 0 {
            return Err(Error::state("a media clock must have a positive rate"));
        }
        Ok(Self { ticks, rate })
    }

    pub fn seconds(self) -> f64 {
        self.ticks as f64 / f64::from(self.rate)
    }
}

/// A completion may be published only while this generation remains current.
#[derive(Clone, Debug)]
pub struct Generation {
    state: Arc<AtomicU64>,
    value: u64,
}

impl Generation {
    pub fn is_current(&self) -> bool {
        self.state.load(Ordering::Acquire) == self.value
    }
}

#[derive(Debug)]
pub struct Cancellation(Arc<AtomicU64>);

impl Default for Cancellation {
    fn default() -> Self {
        Self(Arc::new(AtomicU64::new(1)))
    }
}

impl Cancellation {
    pub fn generation(&self) -> Generation {
        Generation {
            state: Arc::clone(&self.0),
            value: self.0.load(Ordering::Acquire),
        }
    }

    pub fn cancel(&self) {
        self.0.fetch_add(1, Ordering::AcqRel);
    }
}

#[derive(Clone, Debug)]
pub struct ByteRange {
    bytes: Arc<[u8]>,
    range: Range<usize>,
}

impl ByteRange {
    pub fn new(bytes: Arc<[u8]>, range: Range<usize>) -> Result<Self, Error> {
        if range.start > range.end || range.end > bytes.len() {
            return Err(Error::state("a media byte range exceeds its allocation"));
        }
        Ok(Self { bytes, range })
    }

    pub fn data(&self) -> &[u8] {
        &self.bytes[self.range.clone()]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    H264,
    Opus,
    Aac,
}

#[derive(Clone, Debug)]
pub struct CodecConfiguration {
    pub codec: Codec,
    /// AVCC for H.264, ASC for AAC; Opus configuration is supplied by signaling.
    pub bytes: ByteRange,
}

/// An already assembled codec access unit, never an MP4 box or RTP fragment.
#[derive(Clone, Debug)]
pub struct AccessUnit {
    pub configuration: Arc<CodecConfiguration>,
    pub bytes: ByteRange,
    pub presentation: MediaTime,
    pub decode: MediaTime,
    pub duration: MediaTime,
    pub keyframe: bool,
    pub generation: Generation,
}

#[derive(Clone, Debug)]
pub struct Plane {
    bytes: Arc<[u8]>,
    offset: usize,
    stride: usize,
    row_bytes: usize,
    rows: usize,
}

impl Plane {
    pub fn new(
        bytes: Arc<[u8]>,
        offset: usize,
        stride: usize,
        row_bytes: usize,
        rows: usize,
    ) -> Result<Self, Error> {
        let end = rows
            .checked_sub(1)
            .and_then(|rows| rows.checked_mul(stride))
            .and_then(|length| offset.checked_add(length))
            .and_then(|end| end.checked_add(row_bytes));
        if row_bytes == 0 || stride < row_bytes || end.is_none_or(|end| end > bytes.len()) {
            return Err(Error::state("invalid media plane extent"));
        }
        Ok(Self {
            bytes,
            offset,
            stride,
            row_bytes,
            rows,
        })
    }

    pub fn row(&self, index: usize) -> Option<&[u8]> {
        if index >= self.rows {
            return None;
        }
        let start = self.offset + index * self.stride;
        Some(&self.bytes[start..start + self.row_bytes])
    }

    pub fn row_bytes(&self) -> usize {
        self.row_bytes
    }

    pub fn rows(&self) -> usize {
        self.rows
    }
}

#[derive(Clone, Debug)]
pub enum VideoPixels {
    /// Eight-bit, limited-range BT.601 planes.
    I420 { y: Plane, u: Plane, v: Plane },
    /// Eight-bit, unpremultiplied sRGB RGBA.
    Rgba(Plane),
}

#[derive(Clone, Debug)]
pub struct VideoFrame {
    pub source: u64,
    pub width: u32,
    pub height: u32,
    pub presentation: MediaTime,
    pub duration: MediaTime,
    pub generation: Generation,
    pixels: VideoPixels,
}

impl VideoFrame {
    pub fn new(
        source: u64,
        dimensions: [u32; 2],
        presentation: MediaTime,
        duration: MediaTime,
        generation: Generation,
        pixels: VideoPixels,
    ) -> Result<Self, Error> {
        let [width, height] = dimensions;
        let w = width as usize;
        let h = height as usize;
        if w == 0
            || h == 0
            || w.checked_mul(h).is_none_or(|pixels| pixels > 3840 * 2160)
            || presentation.rate == 0
            || duration.rate == 0
            || duration.ticks < 0
        {
            return Err(Error::state("invalid shared video frame"));
        }
        let valid = match &pixels {
            VideoPixels::I420 { y, u, v } => {
                y.row_bytes() == w
                    && y.rows() == h
                    && u.row_bytes() == w.div_ceil(2)
                    && u.rows() == h.div_ceil(2)
                    && v.row_bytes() == u.row_bytes()
                    && v.rows() == u.rows()
            }
            VideoPixels::Rgba(plane) => plane.row_bytes() == w * 4 && plane.rows() == h,
        };
        if !valid {
            return Err(Error::state("video dimensions do not match the planes"));
        }
        Ok(Self {
            source,
            width,
            height,
            presentation,
            duration,
            generation,
            pixels,
        })
    }

    pub fn pixels(&self) -> &VideoPixels {
        &self.pixels
    }
}
