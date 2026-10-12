//! Synchronous I420 encoding with two pending captures and byte-cost pacing.
//!
//! ps-less-avc emits baseline I_PCM IDRs. This adapter sets constraint_set2
//! and level 3.1 in the SPS: the all-intra subset satisfies those constraints,
//! and level 1.0's 64 kbit/s limit would be false for this encoder.
//!
//! Publishing is limited to 64x48, 10 fps and 1 Mbit/s. str0m owns RFC 6184
//! single-NAL/FU-A packetization, the final marker, negotiated PT and SRTP.
//! Its default MTU remains in force. Budgeting assumes at most 1000 payload
//! bytes per packet and charges 194 additional bytes for fragmentation,
//! RTP extensions, authentication, UDP and IPv6. Each IDR carries SPS/PPS.
//! The wire measurement is taken separately at successful UDP sends.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use less_avc::ycbcr_image::{DataPlane, Planes, YCbCrImage};
use less_avc::{BitDepth, LessEncoder};

use crate::media::frame::{Plane, VideoFrame, VideoPixels};
use crate::media::track::{Settings, Track};

pub(super) const PROFILE: &str = "42e01f";

pub(super) struct Encoded {
    pub bytes: Vec<u8>,
    pub timestamp_ms: u64,
    pub width: u32,
    pub height: u32,
    pub wire_bytes: usize,
}

pub(super) struct Sender {
    pub bitrate: u32,
    pub max_frame_rate: f64,
    pub scale: f64,
    capture_due: Instant,
    send_due: Instant,
    pending: VecDeque<Arc<VideoFrame>>,
    last_capture: Option<(u64, i64, u32)>,
    encoder: Option<([u32; 2], LessEncoder)>,
    sps: Vec<u8>,
    pps: Vec<u8>,
}

fn failure(error: impl std::fmt::Display) -> String {
    format!("OperationError: H.264 encode: {error}")
}

fn estimate(width: u32, height: u32) -> usize {
    let macroblocks = width.div_ceil(16) as usize * height.div_ceil(16) as usize;
    let bytes = macroblocks * 578 + 128;
    bytes + bytes.div_ceil(1000) * 194 + 388
}

fn plane(source: &Plane, width: usize, height: usize, block: usize) -> Result<Vec<u8>, String> {
    let stride = width.div_ceil(block) * block;
    let rows = height.div_ceil(block) * block;
    let mut output = vec![0; stride * rows];
    for row in 0..rows {
        let source_row = row.min(height - 1) * source.rows() / height;
        let input = source
            .row(source_row)
            .ok_or("OperationError: invalid camera plane")?;
        for column in 0..stride {
            let source_column = column.min(width - 1) * source.row_bytes() / width;
            output[row * stride + column] = input[source_column];
        }
    }
    Ok(output)
}

impl Sender {
    pub(super) fn new() -> Self {
        let now = Instant::now();
        Self {
            bitrate: 750_000,
            max_frame_rate: 10.0,
            scale: 1.0,
            capture_due: now,
            send_due: now,
            pending: VecDeque::with_capacity(2),
            last_capture: None,
            encoder: None,
            sps: Vec::new(),
            pps: Vec::new(),
        }
    }

    pub(super) fn clear(&mut self) {
        self.pending.clear();
        self.last_capture = None;
    }

    pub(super) fn tick(
        &mut self,
        track: &Track,
        epoch: Instant,
        now: Instant,
    ) -> Result<(Option<Encoded>, Instant, u64), String> {
        let Settings::Video { frame_rate, .. } = track.settings().map_err(|error| failure(format!("{error:?}")))? else {
            return Err("InvalidAccessError: video sender needs a video track".into());
        };
        let fps = frame_rate.min(self.max_frame_rate).min(10.0).max(0.25);
        let period = Duration::from_secs_f64(1.0 / fps);
        let mut dropped = 0;
        if now >= self.capture_due {
            self.capture_due = now + period;
            let frame = track
                .video_at(now.saturating_duration_since(epoch))
                .map_err(|error| failure(format!("{error:?}")))?;
            let identity = (frame.source, frame.presentation.ticks, frame.presentation.rate);
            if self.last_capture != Some(identity) {
                self.last_capture = Some(identity);
                if self.pending.len() == 2 {
                    self.pending.pop_front();
                    dropped += 1;
                }
                self.pending.push_back(Arc::new(frame));
            }
        }

        let mut encoded = None;
        if now >= self.send_due
            && let Some(frame) = self.pending.pop_front()
        {
            if frame.generation.is_current() {
                let unit = self.encode(&frame, fps)?;
                let debt = Duration::from_secs_f64(
                    unit.wire_bytes as f64 * 8.0 / f64::from(self.bitrate),
                );
                // No accumulated credit: lateness cannot cause a catch-up burst.
                self.send_due = now + period.max(debt);
                encoded = Some(unit);
            } else {
                dropped += 1;
            }
        }
        let deadline = if self.pending.is_empty() {
            self.capture_due
        } else {
            self.capture_due.min(self.send_due)
        };
        Ok((encoded, deadline, dropped))
    }

    fn encode(&mut self, frame: &VideoFrame, fps: f64) -> Result<Encoded, String> {
        let VideoPixels::I420 { y, u, v } = frame.pixels() else {
            return Err("NotSupportedError: H.264 publishing requires I420".into());
        };
        let scale = self
            .scale
            .max(f64::from(frame.width) / 64.0)
            .max(f64::from(frame.height) / 48.0)
            .max(1.0);
        let mut width = ((f64::from(frame.width) / scale) as u32 & !1).max(2);
        let mut height = ((f64::from(frame.height) / scale) as u32 & !1).max(2);
        while estimate(width, height) as f64 * 8.0 * fps > f64::from(self.bitrate)
            && width > 16
            && height > 16
        {
            width = ((width / 2) & !1).max(2);
            height = ((height / 2) & !1).max(2);
        }

        let w = width as usize;
        let h = height as usize;
        let luma = plane(y, w, h, 16)?;
        let blue = plane(u, w / 2, h / 2, 8)?;
        let red = plane(v, w / 2, h / 2, 8)?;
        let image = YCbCrImage {
            width,
            height,
            planes: Planes::YCbCr((
                DataPlane {
                    data: &luma,
                    stride: w.div_ceil(16) * 16,
                    bit_depth: BitDepth::Depth8,
                },
                DataPlane {
                    data: &blue,
                    stride: (w / 2).div_ceil(8) * 8,
                    bit_depth: BitDepth::Depth8,
                },
                DataPlane {
                    data: &red,
                    stride: (w / 2).div_ceil(8) * 8,
                    bit_depth: BitDepth::Depth8,
                },
            )),
        };
        let dimensions = [width, height];
        let nal = if self
            .encoder
            .as_ref()
            .is_none_or(|(current, _)| *current != dimensions)
        {
            let (initial, encoder) = LessEncoder::new(&image).map_err(|error| failure(format!("{error:?}")))?;
            let mut sps = initial.sps.to_nal_unit();
            if sps.len() < 4 || sps[0] & 31 != 7 || sps[1] != 66 {
                return Err("OperationError: encoder did not emit baseline H.264".into());
            }
            sps[2] = 0xe0;
            sps[3] = 0x1f;
            self.sps = sps;
            self.pps = initial.pps.to_nal_unit();
            self.encoder = Some((dimensions, encoder));
            initial.frame
        } else {
            self.encoder
                .as_mut()
                .ok_or("OperationError: missing H.264 encoder")?
                .1
                .encode(&image)
                .map_err(|error| failure(format!("{error:?}")))?
        };
        let picture = nal.to_nal_unit();
        if picture.first().is_none_or(|header| header & 31 != 5) {
            return Err("OperationError: encoder did not emit an IDR".into());
        }
        let mut bytes = Vec::with_capacity(self.sps.len() + self.pps.len() + picture.len() + 12);
        let mut wire_bytes = 0;
        for nal in [&self.sps, &self.pps, &picture] {
            bytes.extend_from_slice(&[0, 0, 0, 1]);
            bytes.extend_from_slice(nal);
            wire_bytes += nal.len() + nal.len().div_ceil(1000) * 194;
        }
        if frame.presentation.ticks < 0 {
            return Err("OperationError: negative camera timestamp".into());
        }
        let timestamp_ms = u64::try_from(
            frame.presentation.ticks as u128 * 1000 / u128::from(frame.presentation.rate),
        )
        .map_err(|error| failure(format!("{error:?}")))?;
        Ok(Encoded {
            bytes,
            timestamp_ms,
            width,
            height,
            wire_bytes,
        })
    }
}

