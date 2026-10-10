//! Synchronous G.711 mu-law. No capture, scheduling or transport ownership.

use std::sync::Arc;

pub(super) fn encode(samples: &[f32]) -> Arc<[u8]> {
    samples
        .iter()
        .map(|sample| {
            let sample = (sample.clamp(-1.0, 1.0) * 32767.0).round() as i32;
            let (magnitude, mask) = if sample < 0 {
                (-sample, 0x7f)
            } else {
                (sample, 0xff)
            };
            let biased = magnitude.min(32635) + 132;
            let mut segment = 0;
            while segment < 7 && biased > (255 << segment) {
                segment += 1;
            }
            let mantissa = (biased >> (segment + 3)) & 15;
            ((segment << 4 | mantissa) ^ mask) as u8
        })
        .collect::<Vec<_>>()
        .into()
}

pub(super) fn decode(bytes: &[u8]) -> Arc<[f32]> {
    bytes
        .iter()
        .map(|byte| {
            let value = !byte;
            let magnitude = (((i32::from(value & 15) << 3) + 132) << ((value >> 4) & 7)) - 132;
            let sample = if value & 128 != 0 {
                -magnitude
            } else {
                magnitude
            };
            sample as f32 / 32768.0
        })
        .collect::<Vec<_>>()
        .into()
}
