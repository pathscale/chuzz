//! Incremental fragmented MP4 indexing. Payloads remain shared byte extents.

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;

use mp4parse::{AudioCodecSpecific, SampleEntry, TrackType, VideoCodecSpecific};

use super::MediaError;
use super::demux::Sample;

const MAX_BOX: usize = 16 * 1024 * 1024;
const MAX_SAMPLES: usize = 1_000_000;

#[derive(Clone, PartialEq)]
pub(super) enum Codec {
    Video {
        avcc: Vec<u8>,
        width: u32,
        height: u32,
    },
    Audio {
        asc: Vec<u8>,
        rate: u32,
    },
}

#[derive(Clone, PartialEq)]
pub(super) struct Track {
    pub id: u32,
    pub scale: u64,
    pub codec: Codec,
}

#[derive(Clone)]
pub(super) struct Encoded {
    pub track: u32,
    pub sample: Sample,
    pub decode_time: f64,
    pub bytes: Arc<Vec<u8>>,
}

impl Encoded {
    pub fn data(&self) -> &[u8] {
        let offset = self.sample.offset as usize;
        &self.bytes[offset..offset + self.sample.size]
    }
}

#[derive(Clone, Copy)]
struct BoxRef {
    kind: [u8; 4],
    start: usize,
    header: usize,
    end: usize,
}

impl BoxRef {
    fn body(self, bytes: &[u8]) -> &[u8] {
        &bytes[self.start + self.header..self.end]
    }
}

fn bad(message: &str) -> MediaError {
    MediaError::decode(message)
}

fn box_at(bytes: &[u8], start: usize) -> Result<Option<BoxRef>, MediaError> {
    let Some(head) = bytes.get(start..).filter(|head| head.len() >= 8) else {
        return Ok(None);
    };
    let mut reader = Reader::new(head);
    let size = reader.u32()?;
    let kind = reader.u32()?.to_be_bytes();
    let (size, header) = match size {
        0 => return Err(bad("size-zero boxes are invalid in the MSE byte stream")),
        1 if head.len() < 16 => return Ok(None),
        1 => (reader.u64()?, 16),
        size => (u64::from(size), 8),
    };
    if size < header as u64 || size > MAX_BOX as u64 {
        return Err(bad("invalid or oversized MSE MP4 box"));
    }
    let end = start
        .checked_add(size as usize)
        .ok_or_else(|| bad("MP4 box offset overflow"))?;
    if end > bytes.len() {
        return Ok(None);
    }
    Ok(Some(BoxRef {
        kind,
        start,
        header,
        end,
    }))
}

fn boxes(bytes: &[u8]) -> Result<Vec<BoxRef>, MediaError> {
    let mut result = Vec::new();
    let mut start = 0;
    while start < bytes.len() {
        if result.len() >= 4096 {
            return Err(bad("too many MP4 child boxes"));
        }
        let item = box_at(bytes, start)?.ok_or_else(|| bad("truncated MP4 child box"))?;
        start = item.end;
        result.push(item);
    }
    Ok(result)
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N], MediaError> {
        let end = self
            .position
            .checked_add(N)
            .ok_or_else(|| bad("MP4 field offset overflow"))?;
        let bytes = self
            .bytes
            .get(self.position..end)
            .ok_or_else(|| bad("truncated MP4 field"))?;
        self.position = end;
        bytes.try_into().map_err(|_| bad("invalid MP4 field"))
    }

    fn u32(&mut self) -> Result<u32, MediaError> {
        Ok(u32::from_be_bytes(self.take()?))
    }

    fn u64(&mut self) -> Result<u64, MediaError> {
        Ok(u64::from_be_bytes(self.take()?))
    }

    fn finish(&self) -> Result<(), MediaError> {
        if self.position == self.bytes.len() {
            Ok(())
        } else {
            Err(bad("unexpected MP4 fields"))
        }
    }
}

#[derive(Clone, Copy)]
struct Defaults {
    description: u32,
    duration: u32,
    size: u32,
    flags: u32,
}

#[derive(Clone)]
struct PendingSample {
    track: u32,
    sample: Sample,
    decode_time: f64,
}

#[derive(Clone, Default)]
pub(super) struct Fragments {
    pub tracks: Vec<Arc<Track>>,
    defaults: HashMap<u32, Defaults>,
    next_dts: HashMap<u32, u64>,
    pending: Arc<Vec<u8>>,
    position: u64,
    ftyp: Vec<u8>,
    fragment: Option<Vec<PendingSample>>,
}

impl Fragments {
    pub fn pending_bytes(&self) -> usize {
        self.pending.len()
    }

    pub fn fragment_pending(&self) -> bool {
        self.fragment.is_some()
    }

    pub fn abort(&mut self) {
        self.position = self.position.saturating_add(self.pending.len() as u64);
        self.pending = Arc::new(Vec::new());
        self.fragment = None;
    }

    pub fn append(&mut self, bytes: Arc<Vec<u8>>) -> Result<Vec<Encoded>, MediaError> {
        let input = if self.pending.is_empty() {
            bytes
        } else {
            let size = self
                .pending
                .len()
                .checked_add(bytes.len())
                .filter(|size| *size <= MAX_BOX * 2)
                .ok_or_else(|| bad("MSE partial segment exceeds its limit"))?;
            let mut joined = Vec::with_capacity(size);
            joined.extend_from_slice(&self.pending);
            joined.extend_from_slice(&bytes);
            Arc::new(joined)
        };
        let mut cursor = 0;
        let mut output = Vec::new();
        let mut count = 0;
        while let Some(item) = box_at(&input, cursor)? {
            count += 1;
            if count > 4096 {
                return Err(bad("too many top-level MP4 boxes"));
            }
            let body = item.body(&input);
            match &item.kind {
                b"ftyp" => {
                    if self.fragment.is_some() || item.end - item.start > 65536 {
                        return Err(bad("invalid MSE initialization segment"));
                    }
                    self.ftyp = input[item.start..item.end].to_vec();
                }
                b"moov" => {
                    if self.fragment.is_some() || self.ftyp.is_empty() {
                        return Err(bad("MSE moov must follow ftyp"));
                    }
                    self.initialise(&input[item.start..item.end], body)?;
                }
                b"moof" => {
                    if self.fragment.is_some() || self.tracks.is_empty() {
                        return Err(bad("MSE moof without initialization or mdat"));
                    }
                    let base = self
                        .position
                        .checked_add(item.start as u64)
                        .ok_or_else(|| bad("MSE stream offset overflow"))?;
                    self.fragment = Some(self.moof(body, base)?);
                }
                b"mdat" => {
                    let samples = self
                        .fragment
                        .take()
                        .ok_or_else(|| bad("MSE mdat without moof"))?;
                    let lower = self.position + (item.start + item.header) as u64;
                    let upper = self.position + item.end as u64;
                    for mut packet in samples {
                        let end = packet
                            .sample
                            .offset
                            .checked_add(packet.sample.size as u64)
                            .ok_or_else(|| bad("MP4 sample extent overflow"))?;
                        if packet.sample.offset < lower || end > upper {
                            return Err(bad("fragment sample is outside its mdat"));
                        }
                        packet.sample.offset -= self.position;
                        output.push(Encoded {
                            track: packet.track,
                            sample: packet.sample,
                            decode_time: packet.decode_time,
                            bytes: Arc::clone(&input),
                        });
                    }
                }
                b"free" | b"skip" | b"styp" | b"sidx" | b"emsg" | b"prft" => {}
                _ => return Err(bad("unsupported MSE top-level MP4 box")),
            }
            cursor = item.end;
        }
        self.position = self
            .position
            .checked_add(cursor as u64)
            .ok_or_else(|| bad("MSE stream offset overflow"))?;
        self.pending = if cursor == 0 {
            input
        } else if cursor == input.len() {
            Arc::new(Vec::new())
        } else {
            Arc::new(input[cursor..].to_vec())
        };
        Ok(output)
    }

    fn initialise(&mut self, moov: &[u8], body: &[u8]) -> Result<(), MediaError> {
        let mut metadata = Vec::with_capacity(self.ftyp.len() + moov.len());
        metadata.extend_from_slice(&self.ftyp);
        metadata.extend_from_slice(moov);
        let context = mp4parse::read_mp4(&mut Cursor::new(metadata))
            .map_err(|error| bad(&format!("MSE initialization: {error:?}")))?;
        if context.mvex.is_none() {
            return Err(bad("MSE requires fragmented MP4 initialization"));
        }
        let mut tracks = Vec::new();
        for track in &context.tracks {
            let id = track.track_id.ok_or_else(|| bad("missing MP4 track ID"))?;
            let scale = track
                .timescale
                .filter(|scale| scale.0 != 0)
                .ok_or_else(|| bad("invalid MP4 track timescale"))?
                .0;
            let descriptions = &track
                .stsd
                .as_ref()
                .ok_or_else(|| bad("missing MP4 sample description"))?
                .descriptions;
            if descriptions.len() != 1 {
                return Err(bad("MSE requires one sample description per track"));
            }
            let codec = match (&track.track_type, descriptions.first()) {
                (TrackType::Video, Some(SampleEntry::Video(entry))) => {
                    let VideoCodecSpecific::AVCConfig(avcc) = &entry.codec_specific else {
                        return Err(bad("MSE supports only H.264 video"));
                    };
                    if !entry.protection_info.is_empty()
                        || !matches!(avcc.get(1), Some(66 | 77 | 100))
                        || entry.width < 2
                        || entry.height < 2
                        || entry.width % 2 != 0
                        || entry.height % 2 != 0
                        || u64::from(entry.width) * u64::from(entry.height) > 3840 * 2160
                    {
                        return Err(bad("unsupported or encrypted MSE video"));
                    }
                    Codec::Video {
                        avcc: avcc.to_vec(),
                        width: u32::from(entry.width),
                        height: u32::from(entry.height),
                    }
                }
                (TrackType::Audio, Some(SampleEntry::Audio(entry))) => {
                    let AudioCodecSpecific::ES_Descriptor(esds) = &entry.codec_specific else {
                        return Err(bad("MSE supports only AAC-LC audio"));
                    };
                    if !entry.protection_info.is_empty()
                        || esds.audio_object_type != Some(2)
                        || esds.extended_audio_object_type.is_some()
                        || esds.decoder_specific_data.is_empty()
                        || esds.decoder_specific_data.len() > 1024
                    {
                        return Err(bad("unsupported or encrypted MSE audio"));
                    }
                    Codec::Audio {
                        asc: esds.decoder_specific_data.to_vec(),
                        rate: esds
                            .audio_sample_rate
                            .filter(|rate| (8000..=192000).contains(rate))
                            .ok_or_else(|| bad("unsupported AAC sample rate"))?,
                    }
                }
                _ => return Err(bad("unsupported MSE track")),
            };
            if tracks.iter().any(|other: &Arc<Track>| other.id == id) {
                return Err(bad("duplicate MP4 track ID"));
            }
            tracks.push(Arc::new(Track { id, scale, codec }));
        }
        if tracks.is_empty() || tracks.len() > 2 {
            return Err(bad("MSE requires one or two supported tracks"));
        }
        let mut defaults = HashMap::new();
        for child in boxes(body)? {
            if child.kind != *b"mvex" {
                continue;
            }
            let mvex = child.body(body);
            for child in boxes(mvex)? {
                if child.kind != *b"trex" {
                    continue;
                }
                let mut reader = Reader::new(child.body(mvex));
                if reader.u32()? != 0 {
                    return Err(bad("invalid trex version or flags"));
                }
                let id = reader.u32()?;
                let value = Defaults {
                    description: reader.u32()?,
                    duration: reader.u32()?,
                    size: reader.u32()?,
                    flags: reader.u32()?,
                };
                reader.finish()?;
                if value.description != 1 || defaults.insert(id, value).is_some() {
                    return Err(bad("invalid or duplicate trex"));
                }
            }
        }
        if tracks.iter().any(|track| !defaults.contains_key(&track.id)) {
            return Err(bad("missing trex defaults"));
        }
        if !self.tracks.is_empty() && self.tracks != tracks {
            return Err(bad("changing MSE codec configuration is unsupported"));
        }
        self.tracks = tracks;
        self.defaults = defaults;
        self.next_dts.clear();
        Ok(())
    }

    fn moof(&mut self, body: &[u8], moof_base: u64) -> Result<Vec<PendingSample>, MediaError> {
        let mut result = Vec::new();
        let mut implicit_base = moof_base;
        for child in boxes(body)? {
            if child.kind != *b"traf" {
                continue;
            }
            let traf = child.body(body);
            let children = boxes(traf)?;
            if children
                .iter()
                .any(|child| matches!(&child.kind, b"senc" | b"saiz" | b"saio"))
            {
                return Err(bad("encrypted fragments are unsupported"));
            }
            let headers: Vec<_> = children
                .iter()
                .filter(|child| child.kind == *b"tfhd")
                .collect();
            if headers.len() != 1 {
                return Err(bad("traf requires exactly one tfhd"));
            }
            let mut header = Reader::new(headers[0].body(traf));
            let flags = header.u32()?;
            if flags & !0x03003b != 0 {
                return Err(bad("unsupported tfhd version or flags"));
            }
            let id = header.u32()?;
            let track = self
                .tracks
                .iter()
                .find(|track| track.id == id)
                .cloned()
                .ok_or_else(|| bad("unknown fragment track"))?;
            let mut defaults = *self
                .defaults
                .get(&id)
                .ok_or_else(|| bad("missing fragment defaults"))?;
            let base = if flags & 1 != 0 {
                header.u64()?
            } else if flags & 0x020000 != 0 {
                moof_base
            } else {
                implicit_base
            };
            if flags & 2 != 0 {
                defaults.description = header.u32()?;
            }
            if flags & 8 != 0 {
                defaults.duration = header.u32()?;
            }
            if flags & 16 != 0 {
                defaults.size = header.u32()?;
            }
            if flags & 32 != 0 {
                defaults.flags = header.u32()?;
            }
            header.finish()?;
            if defaults.description != 1 || flags & 0x010000 != 0 {
                return Err(bad(
                    "unsupported fragment sample description or empty duration",
                ));
            }
            let times: Vec<_> = children
                .iter()
                .filter(|child| child.kind == *b"tfdt")
                .collect();
            if times.len() > 1 {
                return Err(bad("duplicate tfdt"));
            }
            let mut dts = if let Some(time) = times.first() {
                let mut reader = Reader::new(time.body(traf));
                let value = match reader.u32()? {
                    0 => u64::from(reader.u32()?),
                    0x01000000 => reader.u64()?,
                    _ => return Err(bad("invalid tfdt version or flags")),
                };
                reader.finish()?;
                value
            } else {
                *self
                    .next_dts
                    .get(&id)
                    .ok_or_else(|| bad("first fragment requires tfdt"))?
            };
            let mut offset = base;
            for child in children.iter().filter(|child| child.kind == *b"trun") {
                let mut reader = Reader::new(child.body(traf));
                let full = reader.u32()?;
                let version = full >> 24;
                let flags = full & 0x00ffffff;
                if version > 1 || flags & !0x000f05 != 0 || flags & 4 != 0 && flags & 0x400 != 0 {
                    return Err(bad("invalid trun version or flags"));
                }
                let count = reader.u32()? as usize;
                if count > MAX_SAMPLES.saturating_sub(result.len()) {
                    return Err(bad("too many fragment samples"));
                }
                if flags & 1 != 0 {
                    offset = base
                        .checked_add_signed(i64::from(reader.u32()? as i32))
                        .ok_or_else(|| bad("invalid trun data offset"))?;
                }
                let first_flags = if flags & 4 != 0 {
                    Some(reader.u32()?)
                } else {
                    None
                };
                for index in 0..count {
                    let delta = if flags & 0x100 != 0 {
                        reader.u32()?
                    } else {
                        defaults.duration
                    };
                    let size = if flags & 0x200 != 0 {
                        reader.u32()?
                    } else {
                        defaults.size
                    };
                    let sample_flags = if flags & 0x400 != 0 {
                        reader.u32()?
                    } else if index == 0 {
                        first_flags.unwrap_or(defaults.flags)
                    } else {
                        defaults.flags
                    };
                    let composition = if flags & 0x800 != 0 {
                        let value = reader.u32()?;
                        if version == 0 {
                            i64::from(value)
                        } else {
                            i64::from(value as i32)
                        }
                    } else {
                        0
                    };
                    if delta == 0 || size == 0 || size > 4 * 1024 * 1024 {
                        return Err(bad("invalid fragment sample duration or size"));
                    }
                    let scale = track.scale as f64;
                    result.push(PendingSample {
                        track: id,
                        sample: Sample {
                            offset,
                            size: size as usize,
                            dts,
                            delta,
                            pts: (dts as f64 + composition as f64) / scale,
                            duration: f64::from(delta) / scale,
                            sync: sample_flags & 0x10000 == 0,
                        },
                        decode_time: dts as f64 / scale,
                    });
                    offset = offset
                        .checked_add(u64::from(size))
                        .ok_or_else(|| bad("fragment data offset overflow"))?;
                    dts = dts
                        .checked_add(u64::from(delta))
                        .ok_or_else(|| bad("fragment timestamp overflow"))?;
                }
                reader.finish()?;
            }
            self.next_dts.insert(id, dts);
            implicit_base = offset;
        }
        if result.is_empty() {
            return Err(bad("fragment has no playable samples"));
        }
        Ok(result)
    }
}
