//! MP4 metadata and sample indexing. Sample payloads stay in the source.

use std::io::Cursor;
use std::sync::Arc;

use mp4parse::{AudioCodecSpecific, SampleEntry, TimeOffsetVersion, TrackType, VideoCodecSpecific};

use super::MediaError;
use super::source::{MAX_READ_BYTES, Source};

const MAX_SAMPLES: usize = 1_000_000;
const MAX_SAMPLE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy)]
pub(super) struct Sample {
    pub offset: u64,
    pub size: usize,
    pub dts: u64,
    pub delta: u32,
    pub pts: f64,
    pub duration: f64,
    pub sync: bool,
}

impl Sample {
    pub fn end(self) -> f64 {
        self.pts + self.duration
    }
}

pub(super) struct Table {
    pub samples: Vec<Sample>,
    pub end: f64,
}

pub(super) struct VideoTrack {
    pub table: Table,
    pub avcc: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub(super) struct AudioTrack {
    pub table: Table,
    pub asc: Vec<u8>,
    pub sample_rate: u32,
}

pub(super) struct Movie {
    pub video: VideoTrack,
    pub audio: Option<AudioTrack>,
    pub duration: f64,
}

impl Movie {
    pub async fn read(source: &Source) -> Result<Arc<Self>, MediaError> {
        let mut offset = 0_u64;
        let mut ftyp = None;
        let mut moov = None;
        for _ in 0..4096 {
            if offset >= source.length {
                break;
            }
            let available = (source.length - offset).min(16) as usize;
            let head = source.read(offset, available).await?;
            if head.len() < 8 {
                return Err(MediaError::decode("truncated MP4 box header"));
            }
            let short = u32::from_be_bytes(
                head[..4]
                    .try_into()
                    .map_err(|_| MediaError::decode("invalid MP4 box header"))?,
            );
            let (size, header_size) = match short {
                0 => (source.length - offset, 8),
                1 => {
                    if head.len() < 16 {
                        return Err(MediaError::decode("truncated extended MP4 box header"));
                    }
                    (
                        u64::from_be_bytes(
                            head[8..16]
                                .try_into()
                                .map_err(|_| MediaError::decode("invalid MP4 box size"))?,
                        ),
                        16,
                    )
                }
                size => (u64::from(size), 8),
            };
            let next = offset
                .checked_add(size)
                .filter(|next| *next <= source.length && size >= header_size)
                .ok_or_else(|| MediaError::decode("invalid MP4 box extent"))?;
            match &head[4..8] {
                b"ftyp" if ftyp.is_none() => {
                    if size > 64 * 1024 {
                        return Err(MediaError::unsupported("oversized MP4 file-type box"));
                    }
                    ftyp = Some(source.read(offset, size as usize).await?);
                }
                b"moov" if moov.is_none() => {
                    if size > MAX_READ_BYTES as u64 {
                        return Err(MediaError::unsupported("MP4 metadata exceeds 16 MiB"));
                    }
                    moov = Some(source.read(offset, size as usize).await?);
                }
                b"moof" => {
                    return Err(MediaError::unsupported(
                        "fragmented MP4 playback is not supported",
                    ));
                }
                _ => {}
            }
            if ftyp.is_some() && moov.is_some() {
                break;
            }
            offset = next;
        }
        let ftyp = ftyp.ok_or_else(|| MediaError::unsupported("the source has no MP4 ftyp"))?;
        let moov = moov.ok_or_else(|| MediaError::unsupported("the source has no MP4 moov"))?;
        let mut metadata = Vec::with_capacity(ftyp.len() + moov.len());
        metadata.extend_from_slice(&ftyp);
        metadata.extend_from_slice(&moov);
        let context = mp4parse::read_mp4(&mut Cursor::new(metadata))
            .map_err(|error| MediaError::decode(format!("MP4 metadata: {error:?}")))?;
        if context.mvex.is_some() {
            return Err(MediaError::unsupported(
                "fragmented MP4 playback is not supported",
            ));
        }

        let video = context
            .tracks
            .iter()
            .find(|track| track.track_type == TrackType::Video)
            .ok_or_else(|| MediaError::unsupported("the MP4 has no video track"))?;
        let entry = description(video)?;
        let SampleEntry::Video(entry) = entry else {
            return Err(MediaError::unsupported("invalid video sample description"));
        };
        if !entry.protection_info.is_empty() {
            return Err(MediaError::unsupported("encrypted video is not supported"));
        }
        let VideoCodecSpecific::AVCConfig(config) = &entry.codec_specific else {
            return Err(MediaError::unsupported(
                "only H.264 video is supported; HEVC and AV1 are unsupported",
            ));
        };
        if !matches!(config.get(1), Some(66 | 77 | 100)) {
            return Err(MediaError::unsupported(
                "only 8-bit Baseline, Main and High H.264 profiles are supported",
            ));
        }
        let video = VideoTrack {
            table: index(video, &context, source.length)?,
            avcc: config.to_vec(),
            width: u32::from(entry.width),
            height: u32::from(entry.height),
        };

        let audio = context
            .tracks
            .iter()
            .find(|track| track.track_type == TrackType::Audio)
            .map(|track| {
                let SampleEntry::Audio(entry) = description(track)? else {
                    return Err(MediaError::unsupported("invalid audio sample description"));
                };
                if !entry.protection_info.is_empty() {
                    return Err(MediaError::unsupported("encrypted audio is not supported"));
                }
                let AudioCodecSpecific::ES_Descriptor(esds) = &entry.codec_specific else {
                    return Err(MediaError::unsupported("only AAC-LC audio is supported"));
                };
                if esds.audio_object_type != Some(2)
                    || esds.extended_audio_object_type.is_some()
                    || esds.decoder_specific_data.is_empty()
                    || esds.decoder_specific_data.len() > 1024
                {
                    return Err(MediaError::unsupported("only AAC-LC audio is supported"));
                }
                let sample_rate = esds
                    .audio_sample_rate
                    .filter(|rate| (8000..=192000).contains(rate))
                    .ok_or_else(|| MediaError::unsupported("unsupported AAC sample rate"))?;
                Ok(AudioTrack {
                    table: index(track, &context, source.length)?,
                    asc: esds.decoder_specific_data.to_vec(),
                    sample_rate,
                })
            })
            .transpose()?;
        let duration = audio.as_ref().map_or(video.table.end, |audio| {
            video.table.end.max(audio.table.end)
        });
        if !duration.is_finite() || duration <= 0.0 {
            return Err(MediaError::decode("the MP4 has no playable duration"));
        }
        Ok(Arc::new(Self {
            video,
            audio,
            duration,
        }))
    }
}

fn description(track: &mp4parse::Track) -> Result<&SampleEntry, MediaError> {
    let descriptions = &track
        .stsd
        .as_ref()
        .ok_or_else(|| MediaError::decode("missing MP4 sample descriptions"))?
        .descriptions;
    if descriptions.len() != 1 {
        return Err(MediaError::unsupported(
            "changing MP4 sample descriptions are unsupported",
        ));
    }
    descriptions
        .first()
        .ok_or_else(|| MediaError::decode("missing MP4 sample description"))
}

fn index(
    track: &mp4parse::Track,
    context: &mp4parse::MediaContext,
    length: u64,
) -> Result<Table, MediaError> {
    let scale = track
        .timescale
        .filter(|scale| scale.0 != 0)
        .ok_or_else(|| MediaError::decode("invalid MP4 track timescale"))?
        .0 as f64;
    let media_time = track.media_time.map_or(0.0, |time| time.0 as f64 / scale);
    let empty = if let Some(empty) = track.empty_duration {
        let scale = context
            .timescale
            .filter(|scale| scale.0 != 0)
            .ok_or_else(|| MediaError::decode("invalid MP4 movie timescale"))?;
        empty.0 as f64 / scale.0 as f64
    } else {
        0.0
    };
    let stts = track
        .stts
        .as_ref()
        .ok_or_else(|| MediaError::decode("missing MP4 sample times"))?;
    let stsz = track
        .stsz
        .as_ref()
        .ok_or_else(|| MediaError::decode("missing MP4 sample sizes"))?;
    let stsc = track
        .stsc
        .as_ref()
        .ok_or_else(|| MediaError::decode("missing MP4 sample-to-chunk table"))?;
    let stco = track
        .stco
        .as_ref()
        .ok_or_else(|| MediaError::decode("missing MP4 chunk offsets"))?;

    let mut count = 0_usize;
    for run in stts.samples.iter() {
        if run.sample_delta == 0 || run.sample_count == 0 {
            return Err(MediaError::decode("invalid MP4 timing run"));
        }
        count = count
            .checked_add(run.sample_count as usize)
            .filter(|count| *count <= MAX_SAMPLES)
            .ok_or_else(|| MediaError::unsupported("MP4 has too many samples"))?;
    }
    if count == 0
        || stco.offsets.is_empty()
        || stsc.samples.first().is_none_or(|run| run.first_chunk != 1)
        || stsz.sample_size == 0 && stsz.sample_sizes.len() != count
    {
        return Err(MediaError::decode("inconsistent MP4 sample tables"));
    }
    let mut previous_chunk = 0;
    for run in stsc.samples.iter() {
        if run.first_chunk <= previous_chunk
            || run.first_chunk as usize > stco.offsets.len()
            || run.samples_per_chunk == 0
            || run.sample_description_index != 1
        {
            return Err(MediaError::decode("invalid MP4 chunk run"));
        }
        previous_chunk = run.first_chunk;
    }

    let mut composition = vec![0_i64; count];
    if let Some(ctts) = &track.ctts {
        let mut cursor = 0_usize;
        for run in ctts.samples.iter() {
            let end = cursor
                .checked_add(run.sample_count as usize)
                .ok_or_else(|| MediaError::decode("MP4 composition count overflow"))?;
            let offset = match run.time_offset {
                TimeOffsetVersion::Version0(offset) => i64::from(offset),
                TimeOffsetVersion::Version1(offset) => i64::from(offset),
            };
            composition
                .get_mut(cursor..end)
                .ok_or_else(|| MediaError::decode("invalid MP4 composition table"))?
                .fill(offset);
            cursor = end;
        }
        if cursor != count {
            return Err(MediaError::decode("inconsistent MP4 composition table"));
        }
    }
    if let Some(stss) = &track.stss {
        let mut previous = 0;
        for sample in stss.samples.iter().copied() {
            if sample <= previous || sample as usize > count {
                return Err(MediaError::decode("invalid MP4 sync-sample table"));
            }
            previous = sample;
        }
    }

    let mut samples = Vec::with_capacity(count);
    let mut dts = 0_u64;
    for run in stts.samples.iter() {
        for _ in 0..run.sample_count {
            let sample = samples.len();
            let pts = (dts as f64 + composition[sample] as f64) / scale + empty - media_time;
            samples.push(Sample {
                offset: 0,
                size: 0,
                dts,
                delta: run.sample_delta,
                pts,
                duration: f64::from(run.sample_delta) / scale,
                sync: track
                    .stss
                    .as_ref()
                    .is_none_or(|stss| stss.samples.binary_search(&(sample as u32 + 1)).is_ok()),
            });
            dts = dts
                .checked_add(u64::from(run.sample_delta))
                .ok_or_else(|| MediaError::decode("MP4 sample timestamp overflow"))?;
        }
    }

    let mut sample_index = 0_usize;
    let mut run_index = 0_usize;
    for (chunk_index, chunk_offset) in stco.offsets.iter().copied().enumerate() {
        while run_index + 1 < stsc.samples.len()
            && stsc.samples[run_index + 1].first_chunk as usize <= chunk_index + 1
        {
            run_index += 1;
        }
        let mut offset = chunk_offset;
        for _ in 0..stsc.samples[run_index].samples_per_chunk {
            let size = if stsz.sample_size == 0 {
                *stsz
                    .sample_sizes
                    .get(sample_index)
                    .ok_or_else(|| MediaError::decode("missing MP4 sample size"))?
            } else {
                stsz.sample_size
            } as usize;
            if size == 0 || size > MAX_SAMPLE_BYTES {
                return Err(MediaError::unsupported("an MP4 sample exceeds 4 MiB"));
            }
            let next = offset
                .checked_add(size as u64)
                .filter(|next| *next <= length)
                .ok_or_else(|| MediaError::decode("invalid MP4 sample extent"))?;
            let sample = samples
                .get_mut(sample_index)
                .ok_or_else(|| MediaError::decode("too many MP4 chunk samples"))?;
            sample.offset = offset;
            sample.size = size;
            sample_index += 1;
            offset = next;
        }
    }
    if sample_index != count {
        return Err(MediaError::decode("missing MP4 chunk samples"));
    }
    let end = samples
        .iter()
        .map(|sample| sample.end())
        .fold(0.0, f64::max);
    Ok(Table { samples, end })
}
