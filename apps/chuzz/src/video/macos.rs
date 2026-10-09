//! AVFoundation demuxing and system H.264/HEVC decoding.
//!
//! AVAssetReader consumes a private local file, so every remote byte first
//! passes through Chuzz's networking and cookie policy. Only the video track
//! is added to the reader. No AVPlayer, native view or independent clock is
//! involved.

use std::ffi::{CString, c_void};
use std::fs::{DirBuilder, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::PathBuf;
use std::ptr::{self, NonNull};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send};
use objc2_core_media::{CMSampleBuffer, CMTime, CMTimeRange};

use super::{Frame, MediaError, VideoDecoder};

const BGRA: u32 = u32::from_be_bytes(*b"BGRA");
const MAX_FRAME_PIXELS: usize = 16 * 1024 * 1024;
static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

#[link(name = "Foundation", kind = "framework")]
unsafe extern "C" {}

#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {
    static AVMediaTypeVideo: *const AnyObject;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(value: *const c_void);
}

#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {
    fn CMFormatDescriptionGetMediaSubType(description: *const c_void) -> u32;
    fn CMSampleBufferGetImageBuffer(sample: *const CMSampleBuffer) -> *mut c_void;
    fn CMSampleBufferGetPresentationTimeStamp(sample: *const CMSampleBuffer) -> CMTime;
}

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    static kCVPixelBufferPixelFormatTypeKey: *const AnyObject;
    fn CVPixelBufferLockBaseAddress(buffer: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(buffer: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddress(buffer: *mut c_void) -> *mut c_void;
    fn CVPixelBufferGetWidth(buffer: *mut c_void) -> usize;
    fn CVPixelBufferGetHeight(buffer: *mut c_void) -> usize;
    fn CVPixelBufferGetBytesPerRow(buffer: *mut c_void) -> usize;
    fn CVPixelBufferGetDataSize(buffer: *mut c_void) -> usize;
    fn CVPixelBufferGetPixelFormatType(buffer: *mut c_void) -> u32;
}

/// Decoder input is temporary data, not persisted browser state. The private
/// directory prevents another user from reading downloaded media. Creation
/// uses filesystem exclusivity; the counter is a filename discriminator.
struct MovieFile {
    directory: PathBuf,
    path: PathBuf,
}

impl MovieFile {
    fn create(bytes: &[u8]) -> Result<Self, MediaError> {
        let root = crate::cookie_store::profile_directory().join("media");
        std::fs::create_dir_all(&root)
            .map_err(|error| MediaError::decode(format!("media directory: {error}")))?;

        let directory = loop {
            let sequence = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
            let candidate = root.join(format!("video-{}-{sequence}", std::process::id()));
            match DirBuilder::new().mode(0o700).create(&candidate) {
                Ok(()) => break candidate,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(MediaError::decode(format!("media directory: {error}")));
                }
            }
        };
        let movie = Self {
            path: directory.join("movie.mp4"),
            directory,
        };
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&movie.path)
            .map_err(|error| MediaError::decode(format!("media file: {error}")))?;
        file.write_all(bytes)
            .map_err(|error| MediaError::decode(format!("media write: {error}")))?;
        Ok(movie)
    }
}

impl Drop for MovieFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_dir(&self.directory);
    }
}

struct Sample(NonNull<CMSampleBuffer>);

impl Drop for Sample {
    fn drop(&mut self) {
        // SAFETY: copyNextSampleBuffer transfers one owned CF reference.
        unsafe { CFRelease(self.0.as_ptr().cast()) };
    }
}

struct LockedBuffer(*mut c_void);

impl Drop for LockedBuffer {
    fn drop(&mut self) {
        // SAFETY: This guard is created only after a successful read-only lock.
        unsafe {
            CVPixelBufferUnlockBaseAddress(self.0, 1);
        }
    }
}

pub(super) struct Decoder {
    reader: Retained<AnyObject>,
    output: Retained<AnyObject>,
    asset: Retained<AnyObject>,
    track: Retained<AnyObject>,
    duration: f64,
    // Declared last so all reader and asset references drop before the file.
    _movie: MovieFile,
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: reader is an owned AVAssetReader, used on the document thread.
        unsafe {
            let _: () = msg_send![&*self.reader, cancelReading];
        }
    }
}

fn string(value: &str) -> Result<Retained<AnyObject>, MediaError> {
    let value = CString::new(value)
        .map_err(|_| MediaError::decode("a media path contains a NUL byte"))?;
    // SAFETY: NSString copies the valid UTF-8 bytes during this call.
    let string: Option<Retained<AnyObject>> =
        unsafe { msg_send![class!(NSString), stringWithUTF8String: value.as_ptr()] };
    string.ok_or_else(|| MediaError::decode("could not create a media path"))
}

pub(super) fn open(bytes: &[u8]) -> Result<Decoder, MediaError> {
    autoreleasepool(|_| {
        let movie = MovieFile::create(bytes)?;
        let path = movie
            .path
            .to_str()
            .ok_or_else(|| MediaError::decode("the media path is not UTF-8"))?;
        let path = string(path)?;

        // SAFETY: Each retained object below is produced by the named system
        // class. Nullable factories are checked before their objects are used.
        unsafe {
            let url: Option<Retained<AnyObject>> =
                msg_send![class!(NSURL), fileURLWithPath: &*path];
            let url = url.ok_or_else(|| MediaError::decode("could not create a media URL"))?;
            let asset: Option<Retained<AnyObject>> = msg_send![
                class!(AVURLAsset),
                URLAssetWithURL: &*url,
                options: ptr::null::<AnyObject>()
            ];
            let asset = asset.ok_or_else(|| MediaError::decode("could not open the media asset"))?;
            let tracks: Retained<AnyObject> =
                msg_send![&*asset, tracksWithMediaType: AVMediaTypeVideo];
            let count: usize = msg_send![&*tracks, count];
            if count == 0 {
                return Err(MediaError::unsupported("the asset has no video track"));
            }
            let track: Retained<AnyObject> = msg_send![&*tracks, objectAtIndex: 0usize];
            let descriptions: Retained<AnyObject> = msg_send![&*track, formatDescriptions];
            let count: usize = msg_send![&*descriptions, count];
            if count == 0 {
                return Err(MediaError::unsupported("the video has no format description"));
            }
            for index in 0..count {
                let description: Retained<AnyObject> =
                    msg_send![&*descriptions, objectAtIndex: index];
                let codec = CMFormatDescriptionGetMediaSubType(
                    (&*description as *const AnyObject).cast(),
                );
                if codec != u32::from_be_bytes(*b"avc1")
                    && codec != u32::from_be_bytes(*b"hvc1")
                    && codec != u32::from_be_bytes(*b"hev1")
                {
                    return Err(MediaError::unsupported(
                        "only system H.264 and HEVC decoding is supported",
                    ));
                }
            }
            let duration: CMTime = msg_send![&*asset, duration];
            let duration = duration.seconds();
            if !duration.is_finite() || duration <= 0.0 {
                return Err(MediaError::unsupported(
                    "the video must have a finite positive duration",
                ));
            }
            let (reader, output) = make_reader(&asset, &track, 0.0, duration)?;
            Ok(Decoder {
                reader,
                output,
                asset,
                track,
                duration,
                _movie: movie,
            })
        }
    })
}

fn make_reader(
    asset: &AnyObject,
    track: &AnyObject,
    start: f64,
    duration: f64,
) -> Result<(Retained<AnyObject>, Retained<AnyObject>), MediaError> {
    // SAFETY: asset and track are retained objects from AVURLAsset. The
    // dictionary requests one documented packed BGRA output format.
    unsafe {
        let mut error: *mut AnyObject = ptr::null_mut();
        let reader: Option<Retained<AnyObject>> = msg_send![
            class!(AVAssetReader),
            assetReaderWithAsset: asset,
            error: &mut error as *mut *mut AnyObject
        ];
        let reader = reader.ok_or_else(|| MediaError::decode("could not create a video reader"))?;
        let format: Retained<AnyObject> =
            msg_send![class!(NSNumber), numberWithUnsignedInt: BGRA];
        let settings: Retained<AnyObject> = msg_send![
            class!(NSDictionary),
            dictionaryWithObject: &*format,
            forKey: kCVPixelBufferPixelFormatTypeKey
        ];
        let output: Option<Retained<AnyObject>> = msg_send![
            class!(AVAssetReaderTrackOutput),
            assetReaderTrackOutputWithTrack: track,
            outputSettings: &*settings
        ];
        let output = output.ok_or_else(|| MediaError::decode("could not create video output"))?;
        let allowed: Bool = msg_send![&*reader, canAddOutput: &*output];
        if !allowed.as_bool() {
            return Err(MediaError::unsupported("the system cannot decode this video track"));
        }
        let _: () = msg_send![&*output, setAlwaysCopiesSampleData: Bool::NO];
        let _: () = msg_send![&*reader, addOutput: &*output];

        // A seek to the endpoint still decodes the final frame. AVAssetReader
        // performs codec preroll internally for non-keyframe start times.
        let start = start.clamp(0.0, (duration - 0.001).max(0.0));
        let range = CMTimeRange {
            start: CMTime::with_seconds(start, 1_000_000),
            duration: CMTime::with_seconds(duration - start, 1_000_000),
        };
        let _: () = msg_send![&*reader, setTimeRange: range];
        let started: Bool = msg_send![&*reader, startReading];
        if !started.as_bool() {
            return Err(MediaError::decode("the system video decoder could not start"));
        }
        Ok((reader, output))
    }
}

impl VideoDecoder for Decoder {
    fn duration(&self) -> f64 {
        self.duration
    }

    fn seek(&mut self, time: f64) -> Result<(), MediaError> {
        autoreleasepool(|_| {
            let (reader, output) = make_reader(&self.asset, &self.track, time, self.duration)?;
            // SAFETY: Both readers are owned AVAssetReaders on this thread.
            unsafe {
                let _: () = msg_send![&*self.reader, cancelReading];
            }
            self.reader = reader;
            self.output = output;
            Ok(())
        })
    }

    fn next_frame(&mut self) -> Result<Option<Frame>, MediaError> {
        autoreleasepool(|_| {
            // SAFETY: output is attached to a started reader. The returned
            // sample transfers ownership and its pixel buffer is borrowed
            // until the sample guard drops.
            unsafe {
                let sample: *mut CMSampleBuffer =
                    msg_send![&*self.output, copyNextSampleBuffer];
                let Some(sample) = NonNull::new(sample) else {
                    let status: isize = msg_send![&*self.reader, status];
                    return if status == 2 {
                        Ok(None)
                    } else {
                        Err(MediaError::decode("the system video decoder failed"))
                    };
                };
                let sample = Sample(sample);
                let time = CMSampleBufferGetPresentationTimeStamp(sample.0.as_ptr()).seconds();
                if !time.is_finite() {
                    return Err(MediaError::decode("the video frame has an invalid timestamp"));
                }
                let buffer = CMSampleBufferGetImageBuffer(sample.0.as_ptr());
                if buffer.is_null() || CVPixelBufferGetPixelFormatType(buffer) != BGRA {
                    return Err(MediaError::decode("the decoder did not return a BGRA frame"));
                }
                if CVPixelBufferLockBaseAddress(buffer, 1) != 0 {
                    return Err(MediaError::decode("could not lock the decoded video frame"));
                }
                let _lock = LockedBuffer(buffer);
                let width = CVPixelBufferGetWidth(buffer);
                let height = CVPixelBufferGetHeight(buffer);
                let stride = CVPixelBufferGetBytesPerRow(buffer);
                let pixels = width
                    .checked_mul(height)
                    .filter(|pixels| *pixels > 0 && *pixels <= MAX_FRAME_PIXELS)
                    .ok_or_else(|| MediaError::decode("the decoded frame is too large"))?;
                let row_bytes = width
                    .checked_mul(4)
                    .ok_or_else(|| MediaError::decode("the decoded row is too large"))?;
                let source_size = stride
                    .checked_mul(height)
                    .ok_or_else(|| MediaError::decode("the decoded buffer is too large"))?;
                let base = CVPixelBufferGetBaseAddress(buffer).cast::<u8>();
                if base.is_null()
                    || stride < row_bytes
                    || source_size > CVPixelBufferGetDataSize(buffer)
                {
                    return Err(MediaError::decode("the decoded video buffer is invalid"));
                }
                let source = std::slice::from_raw_parts(base, source_size);
                let mut rgba = vec![0; pixels * 4];
                for row in 0..height {
                    let source = &source[row * stride..row * stride + row_bytes];
                    let destination = &mut rgba[row * row_bytes..(row + 1) * row_bytes];
                    for (source, destination) in source
                        .chunks_exact(4)
                        .zip(destination.chunks_exact_mut(4))
                    {
                        destination.copy_from_slice(&[source[2], source[1], source[0], 255]);
                    }
                }
                Ok(Some(Frame {
                    time: time.max(0.0),
                    width: width as u32,
                    height: height as u32,
                    rgba: Arc::new(rgba),
                }))
            }
        })
    }
}
