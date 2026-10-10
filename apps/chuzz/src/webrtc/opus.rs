//! Synchronous Opus coding and bounded receive lookahead.
//!
//! RTP clocks identify missing audio. A received successor can supply FEC;
//! a deadline without a usable successor supplies PLC. Capture, sockets and
//! scheduling belong to the transport task.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rusty_opus::repacketizer::{Repacketizer, samples_per_frame};
use rusty_opus::{Application, OpusDecoder, OpusEncoder, SignalType};

use crate::audio::pcm::Track;

use super::sdp::AudioCodec;

pub(super) const RATE: u32 = 48_000;
pub(super) const FRAME: usize = 960;
const CHANNELS: usize = 2;
const MAX_FRAMES: usize = 5760;
const MAX_PACKET: usize = 1275 * 48 + 2;
const PACKETS: usize = 12;
const BUFFERS: usize = 72;
const MAX_LOSS: usize = FRAME * 10;
// Decode before presentation, while retaining the rest of PLAYOUT for
// successor arrival and FEC. Transport wakes on this deadline independently
// of its capture clock.
const DECODE_LEAD: Duration = Duration::from_millis(20);
pub(super) const PLAYOUT: Duration = Duration::from_millis(80);

fn failure(error: rusty_opus::Error) -> String {
    format!("OperationError: Opus: {error}")
}

pub(super) struct Sender {
    encoder: Option<OpusEncoder>,
    channels: usize,
    pcm: Vec<f32>,
    packet: [u8; 1500],
    pub bitrate: u32,
    pub cbr: bool,
}

impl Sender {
    pub(super) fn new() -> Self {
        Self {
            encoder: None,
            channels: 0,
            pcm: Vec::with_capacity(FRAME * CHANNELS),
            packet: [0; 1500],
            bitrate: 32_000,
            cbr: false,
        }
    }

    pub(super) fn encode(
        &mut self,
        input: &Track,
        start: Duration,
        codec: &AudioCodec,
    ) -> Result<Arc<[u8]>, String> {
        let channels = if codec.stereo && input.channels() == 2 {
            2
        } else {
            1
        };
        if self.channels != channels || self.encoder.is_none() {
            self.encoder =
                Some(OpusEncoder::new(RATE as i32, channels, Application::Voip).map_err(failure)?);
            self.channels = channels;
        }
        self.pcm.resize(FRAME * channels, 0.0);
        input.read_into(start, RATE, channels as u16, &mut self.pcm);
        let encoder = self
            .encoder
            .as_mut()
            .ok_or("OperationError: missing Opus encoder")?;
        encoder.bitrate_bps = self.bitrate.min(codec.bitrate.unwrap_or(510_000)) as i32;
        encoder.use_cbr = self.cbr || codec.cbr;
        encoder.use_inband_fec = codec.fec;
        encoder.packet_loss_perc = if codec.fec { 10 } else { 0 };
        encoder.use_dtx = false;
        encoder.signal_type = Some(SignalType::Voice);
        let length = encoder
            .encode(&self.pcm, FRAME, &mut self.packet)
            .map_err(failure)?;
        // The writer retains packet ownership. The codec's scratch buffer is
        // immediately reusable; no PCM or encoded bytes cross the JS bridge.
        Ok(Arc::from(&self.packet[..length]))
    }
}

/// Read only the packet duration needed to bound decoder output. The decoder
/// validates the complete packet before its samples become shared PCM.
fn packet_frames(packet: &[u8]) -> Result<usize, String> {
    if packet.is_empty() || packet.len() > MAX_PACKET {
        return Err("OperationError: invalid Opus packet length".into());
    }
    let toc = packet[0];
    let config = toc >> 3;
    let frame = if config >= 16 {
        120usize << (config & 3)
    } else if config >= 12 {
        480usize << (config & 1)
    } else if config & 3 == 3 {
        2880
    } else {
        480usize << (config & 3)
    };
    let count = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => usize::from(
            *packet
                .get(1)
                .ok_or("OperationError: truncated Opus frame count")?
                & 63,
        ),
    };
    let frames = frame * count;
    if count == 0 || count > 48 || frames > MAX_FRAMES {
        return Err("OperationError: Opus packet exceeds 120 ms".into());
    }
    Ok(frames)
}

struct Packet {
    timestamp: u64,
    frames: usize,
    bytes: Arc<[u8]>,
}

pub(super) struct Decoded {
    pub position: u64,
    pub samples: Arc<[f32]>,
    pub concealed_frames: u64,
    pub fec_attempts: u64,
    pub errors: u64,
}

pub(super) struct Receiver {
    decoder: OpusDecoder,
    repacketizer: Repacketizer,
    fec_packet: [u8; 1276],
    packets: VecDeque<Packet>,
    buffers: VecDeque<Arc<[f32]>>,
    expected: Option<u64>,
    due: Option<Instant>,
    position: u64,
    frame: usize,
    lost: usize,
    fec: bool,
}

impl Receiver {
    pub(super) fn new(fec: bool, position: u64) -> Result<Self, String> {
        Ok(Self {
            decoder: OpusDecoder::new(RATE as i32, CHANNELS).map_err(failure)?,
            repacketizer: Repacketizer::new(),
            fec_packet: [0; 1276],
            packets: VecDeque::with_capacity(PACKETS),
            buffers: VecDeque::with_capacity(BUFFERS),
            expected: None,
            due: None,
            position,
            frame: FRAME,
            lost: 0,
            fec,
        })
    }

    pub(super) fn enqueue(&mut self, timestamp: u64, bytes: Arc<[u8]>) -> Result<(), String> {
        let frames = packet_frames(&bytes)?;
        if self.expected.is_some_and(|expected| timestamp < expected)
            || self
                .packets
                .iter()
                .any(|packet| packet.timestamp == timestamp)
        {
            return Ok(());
        }
        if self.packets.len() == PACKETS {
            return Err("OperationError: Opus receive lookahead is full".into());
        }
        let index = self
            .packets
            .iter()
            .position(|packet| packet.timestamp > timestamp)
            .unwrap_or(self.packets.len());
        self.packets.insert(
            index,
            Packet {
                timestamp,
                frames,
                bytes,
            },
        );
        Ok(())
    }

    pub(super) fn deadline(&self) -> Option<Instant> {
        self.due
    }

    fn buffer(&mut self, samples: usize) -> Arc<[f32]> {
        if let Some(index) = self
            .buffers
            .iter()
            .position(|buffer| buffer.len() == samples && Arc::strong_count(buffer) == 1)
        {
            return self.buffers.remove(index).expect("existing PCM buffer");
        }
        if self.buffers.len() == BUFFERS {
            self.buffers.pop_front();
        }
        vec![0.0; samples].into()
    }

    pub(super) fn render(
        &mut self,
        now: Instant,
        epoch: Instant,
        mut emit: impl FnMut(Decoded) -> Result<(), String>,
    ) -> Result<(), String> {
        if self.expected.is_none() {
            let Some(first) = self.packets.front() else {
                return Ok(());
            };
            self.decoder = OpusDecoder::new(RATE as i32, CHANNELS).map_err(failure)?;
            self.expected = Some(first.timestamp);
            self.frame = first.frames;
            let position = ((now.saturating_duration_since(epoch) + PLAYOUT).as_secs_f64()
                * f64::from(RATE)) as u64;
            self.position = self.position.max(position);
            // The decode deadline belongs to the same sample clock as the
            // PCM position. Do not conceal 60 ms ahead of presentation and
            // then discard a successor that arrived within the playout budget.
            self.due = Some(
                epoch + Duration::from_secs_f64(self.position as f64 / f64::from(RATE))
                    - DECODE_LEAD,
            );
            self.lost = 0;
        }

        // Bound catch-up work even after a delayed host wakeup.
        for _ in 0..8 {
            let Some(expected) = self.expected else {
                break;
            };
            let Some(due) = self.due else {
                break;
            };
            if now < due {
                break;
            }
            while self
                .packets
                .front()
                .is_some_and(|packet| packet.timestamp < expected)
            {
                self.packets.pop_front();
            }

            let normal = self
                .packets
                .front()
                .is_some_and(|packet| packet.timestamp == expected);
            if !normal && self.lost >= MAX_LOSS {
                // A paused sender must not create unlimited synthetic audio.
                // A later packet starts a new bounded playout anchor. A ready
                // packet at the expected timestamp can still resume this one.
                self.expected = None;
                self.due = None;
                break;
            }

            let (frames, bytes, fec) = if normal {
                let packet = self.packets.pop_front().expect("ready Opus packet");
                (packet.frames, packet.bytes, false)
            } else if let Some(next) = self.packets.front() {
                let gap = next.timestamp - expected;
                let frames = self
                    .frame
                    .min(FRAME)
                    .min(gap.min(MAX_FRAMES as u64) as usize);
                // Only the immediately preceding frame can use this packet's
                // LBRR. Older missing frames advance the decoder through PLC.
                // The fork's LBRR entry point is mono; stereo loss uses PLC.
                let fec = self.fec
                    && (480..=FRAME).contains(&frames)
                    && gap == frames as u64
                    && next.bytes[0] & 4 == 0
                    && samples_per_frame(next.bytes[0], RATE as i32) as usize == frames;
                (frames, Arc::clone(&next.bytes), fec)
            } else {
                (self.frame.min(FRAME), Arc::from([]), false)
            };
            if frames == 0 || !frames.is_multiple_of(120) {
                self.expected = None;
                self.due = None;
                break;
            }

            let mut buffer = self.buffer(frames * CHANNELS);
            let output =
                Arc::get_mut(&mut buffer).ok_or("OperationError: leased Opus output buffer")?;
            let result = if normal {
                self.decoder.decode(&bytes, frames, output)
            } else if fec {
                // CBR SILK commonly carries one frame with code-3 padding.
                // decode_fec accepts only code 0, so strip framing through
                // the codec's validated repacketizer before attempting LBRR.
                // The successor remains queued for normal decoding.
                self.repacketizer.reset();
                self.repacketizer
                    .cat(&bytes)
                    .and_then(|()| self.repacketizer.out_range_into(0, 1, &mut self.fec_packet))
                    .and_then(|length| {
                        // This API uses LBRR when present and PLC otherwise.
                        // An attempt does not prove redundancy was present.
                        self.decoder
                            .decode_fec(&self.fec_packet[..length], frames, output)
                    })
            } else {
                self.decoder.decode(&[], frames, output)
            };
            let errors = u64::from(result.is_err());
            let concealed = !normal || result.is_err();
            let decoded = match result {
                Ok(decoded) => decoded,
                Err(_) => self.decoder.decode(&[], frames, output).map_err(failure)?,
            };
            if decoded != frames || output.iter().any(|sample| !sample.is_finite()) {
                return Err("OperationError: invalid Opus decoded frame".into());
            }
            emit(Decoded {
                position: self.position,
                samples: Arc::clone(&buffer),
                concealed_frames: if concealed { frames as u64 } else { 0 },
                fec_attempts: u64::from(fec),
                errors,
            })?;
            self.buffers.push_back(buffer);
            self.position += frames as u64;
            self.expected = Some(expected + frames as u64);
            self.due = Some(due + Duration::from_secs_f64(frames as f64 / f64::from(RATE)));
            if concealed {
                self.lost += frames;
            } else {
                self.lost = 0;
                self.frame = frames;
            }
        }
        Ok(())
    }
}
