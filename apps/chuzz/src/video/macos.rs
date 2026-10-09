//! AVFoundation audio playback and H.264/HEVC video output.
//!
//! AVPlayer owns one media clock for both tracks. Video frames are copied
//! from AVPlayerItemVideoOutput into Blitz; no native view is created.
//! The asset URL is a private loopback capability backed by Chuzz networking.

use std::ffi::{CStr, CString, c_void};
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use blitz_traits::net::Url;
use block2::RcBlock;
use objc2::encode::{Encode, Encoding, RefEncode};
use objc2::rc::{Allocated, Retained, autoreleasepool};
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send};
use objc2_core_media::{CMTime, CMTimeRange};

use super::source::Source;
use super::{Frame, MediaError, Snapshot, VideoDecoder};
use crate::document_loader::NetProvider;

const BGRA: u32 = u32::from_be_bytes(*b"BGRA");
const MAX_FRAME_PIXELS: usize = 16 * 1024 * 1024;

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

#[repr(C)]
#[derive(Clone, Copy)]
struct Size {
    width: f64,
    height: f64,
}

// SAFETY: CGFloat is double on supported 64-bit macOS targets. These fields
// and their order match the CoreGraphics CGSize ABI.
unsafe impl Encode for Size {
    const ENCODING: Encoding = Encoding::Struct("CGSize", &[Encoding::Double, Encoding::Double]);
}

// SAFETY: A reference to Size has the corresponding pointer encoding.
unsafe impl RefEncode for Size {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Self::ENCODING);
}

struct PixelBuffer(*mut c_void);

impl Drop for PixelBuffer {
    fn drop(&mut self) {
        // SAFETY: copyPixelBufferForItemTime transfers a +1 CF reference.
        unsafe { CFRelease(self.0) };
    }
}

struct LockedBuffer(*mut c_void);

impl Drop for LockedBuffer {
    fn drop(&mut self) {
        // SAFETY: This guard follows a successful read-only buffer lock.
        unsafe {
            CVPixelBufferUnlockBaseAddress(self.0, 1);
        }
    }
}

pub(super) struct Decoder {
    player: Retained<AnyObject>,
    item: Retained<AnyObject>,
    output: Retained<AnyObject>,
    source: Source,
    checked: bool,
    paused: bool,
    rate: f64,
    applied_rate: f32,
    pending_seek: Option<Arc<AtomicU8>>,
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: All three objects are owned AVFoundation objects. Pending
        // completion blocks capture only their own Arc<AtomicU8>.
        unsafe {
            let _: () = msg_send![&*self.player, pause];
            let _: () = msg_send![&*self.item, cancelPendingSeeks];
            let _: () = msg_send![&*self.item, removeOutput: &*self.output];
            let _: () =
                msg_send![&*self.player, replaceCurrentItemWithPlayerItem: ptr::null::<AnyObject>()];
        }
    }
}

fn string(value: &str) -> Result<Retained<AnyObject>, MediaError> {
    let value = CString::new(value)
        .map_err(|_| MediaError::decode("a media URL contains a NUL byte"))?;
    // SAFETY: NSString copies the UTF-8 bytes during this call.
    let string: Option<Retained<AnyObject>> =
        unsafe { msg_send![class!(NSString), stringWithUTF8String: value.as_ptr()] };
    string.ok_or_else(|| MediaError::decode("could not create a media URL"))
}

pub(super) fn open(
    net: Arc<NetProvider>,
    url: Url,
    preload: &str,
) -> Result<Decoder, MediaError> {
    autoreleasepool(|_| {
        let source = Source::new(net, url)?;
        let text = string(&source.url)?;
        // SAFETY: Nullable factories are checked. Output settings specify
        // packed BGRA; the player never owns a window or a native view.
        unsafe {
            let url: Option<Retained<AnyObject>> =
                msg_send![class!(NSURL), URLWithString: &*text];
            let url = url.ok_or_else(|| MediaError::decode("could not create a media URL"))?;
            let item: Option<Retained<AnyObject>> =
                msg_send![class!(AVPlayerItem), playerItemWithURL: &*url];
            let item = item.ok_or_else(|| MediaError::decode("could not create a player item"))?;
            let format: Retained<AnyObject> =
                msg_send![class!(NSNumber), numberWithUnsignedInt: BGRA];
            let settings: Retained<AnyObject> = msg_send![
                class!(NSDictionary),
                dictionaryWithObject: &*format,
                forKey: kCVPixelBufferPixelFormatTypeKey
            ];
            let output: Allocated<AnyObject> =
                msg_send![class!(AVPlayerItemVideoOutput), alloc];
            let output: Option<Retained<AnyObject>> =
                msg_send![output, initWithPixelBufferAttributes: &*settings];
            let output =
                output.ok_or_else(|| MediaError::decode("could not create video output"))?;
            let _: () = msg_send![&*output, setSuppressesPlayerRendering: Bool::YES];
            let _: () = msg_send![&*item, addOutput: &*output];
            let forward = if preload == "metadata" { 0.1_f64 } else { 5.0_f64 };
            let _: () = msg_send![&*item, setPreferredForwardBufferDuration: forward];
            let player: Option<Retained<AnyObject>> =
                msg_send![class!(AVPlayer), playerWithPlayerItem: &*item];
            let player =
                player.ok_or_else(|| MediaError::decode("could not create a media player"))?;
            let _: () = msg_send![&*player, setAutomaticallyWaitsToMinimizeStalling: Bool::YES];
            let _: () = msg_send![&*player, setActionAtItemEnd: 1isize];
            let _: () = msg_send![&*player, setVolume: 0.0_f32];
            Ok(Decoder {
                player,
                item,
                output,
                source,
                checked: false,
                paused: true,
                rate: 1.0,
                applied_rate: 0.0,
                pending_seek: None,
            })
        }
    })
}

fn check_video(item: &AnyObject) -> Result<(), MediaError> {
    // SAFETY: This runs only once the item reports ReadyToPlay.
    unsafe {
        let asset: Retained<AnyObject> = msg_send![item, asset];
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
            let codec =
                CMFormatDescriptionGetMediaSubType((&*description as *const AnyObject).cast());
            if !matches!(
                codec,
                value if value == u32::from_be_bytes(*b"avc1")
                    || value == u32::from_be_bytes(*b"hvc1")
                    || value == u32::from_be_bytes(*b"hev1")
            ) {
                return Err(MediaError::unsupported(
                    "only system H.264 and HEVC decoding is supported",
                ));
            }
        }
        Ok(())
    }
}

fn ranges(array: &AnyObject) -> Vec<[f64; 2]> {
    // SAFETY: AVPlayerItem's time range arrays contain NSValue CMTimeRanges.
    unsafe {
        let count: usize = msg_send![array, count];
        let mut result = Vec::with_capacity(count.min(1024));
        for index in 0..count.min(1024) {
            let value: Retained<AnyObject> = msg_send![array, objectAtIndex: index];
            let range: CMTimeRange = msg_send![&*value, timeRangeValue];
            let start = range.start.seconds();
            let end = start + range.duration.seconds();
            if start.is_finite() && end.is_finite() && start >= 0.0 && end > start {
                result.push([start, end]);
            }
        }
        result.sort_by(|left, right| left[0].total_cmp(&right[0]));
        let mut merged: Vec<[f64; 2]> = Vec::with_capacity(result.len());
        for range in result {
            if let Some(previous) = merged.last_mut()
                && range[0] <= previous[1]
            {
                previous[1] = previous[1].max(range[1]);
            } else {
                merged.push(range);
            }
        }
        merged
    }
}

fn frame(output: &AnyObject, time: CMTime) -> Result<Option<Frame>, MediaError> {
    // SAFETY: The output belongs to the ready player item. The copied buffer
    // stays owned until after its read-only lock and byte slice are released.
    unsafe {
        let available: Bool = msg_send![output, hasNewPixelBufferForItemTime: time];
        if !available.as_bool() {
            return Ok(None);
        }
        let buffer: *mut c_void = msg_send![
            output,
            copyPixelBufferForItemTime: time,
            itemTimeForDisplay: ptr::null_mut::<CMTime>()
        ];
        if buffer.is_null() {
            return Ok(None);
        }
        let buffer = PixelBuffer(buffer);
        if CVPixelBufferGetPixelFormatType(buffer.0) != BGRA {
            return Err(MediaError::decode("the decoder did not return a BGRA frame"));
        }
        if CVPixelBufferLockBaseAddress(buffer.0, 1) != 0 {
            return Err(MediaError::decode("could not lock the decoded video frame"));
        }
        let _lock = LockedBuffer(buffer.0);
        let width = CVPixelBufferGetWidth(buffer.0);
        let height = CVPixelBufferGetHeight(buffer.0);
        let stride = CVPixelBufferGetBytesPerRow(buffer.0);
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
        let base = CVPixelBufferGetBaseAddress(buffer.0).cast::<u8>();
        if base.is_null()
            || stride < row_bytes
            || source_size > CVPixelBufferGetDataSize(buffer.0)
        {
            return Err(MediaError::decode("the decoded video buffer is invalid"));
        }
        let source = std::slice::from_raw_parts(base, source_size);
        let mut rgba = vec![0; pixels * 4];
        for row in 0..height {
            let source = &source[row * stride..row * stride + row_bytes];
            let destination = &mut rgba[row * row_bytes..(row + 1) * row_bytes];
            for (source, destination) in source.chunks_exact(4).zip(destination.chunks_exact_mut(4)) {
                destination.copy_from_slice(&[source[2], source[1], source[0], 255]);
            }
        }
        Ok(Some(Frame {
            width: width as u32,
            height: height as u32,
            rgba: Arc::new(rgba),
        }))
    }
}

impl VideoDecoder for Decoder {
    fn configure(&mut self, paused: bool, muted: bool, volume: f64, rate: f64) {
        self.paused = paused;
        self.rate = rate;
        // SAFETY: player is an owned AVPlayer on the document thread.
        unsafe {
            let volume = if muted { 0.0 } else { volume.clamp(0.0, 1.0) as f32 };
            let _: () = msg_send![&*self.player, setVolume: volume];
            if paused {
                let _: () = msg_send![&*self.player, pause];
                self.applied_rate = 0.0;
            }
        }
    }

    fn seek(&mut self, time: f64) -> Result<(), MediaError> {
        let state = Arc::new(AtomicU8::new(0));
        let completion_state = Arc::clone(&state);
        let completion = RcBlock::new(move |finished: Bool| {
            completion_state.store(if finished.as_bool() { 1 } else { 2 }, Ordering::Release);
        });
        // SAFETY: AVPlayer copies the completion block. It captures only
        // thread-safe atomic state, independently for each seek request.
        unsafe {
            let _: () = msg_send![&*self.player, pause];
            let _: () = msg_send![&*self.item, cancelPendingSeeks];
            let zero = CMTime::with_seconds(0.0, 1_000_000);
            let _: () = msg_send![
                &*self.player,
                seekToTime: CMTime::with_seconds(time, 1_000_000),
                toleranceBefore: zero,
                toleranceAfter: zero,
                completionHandler: &*completion
            ];
        }
        self.applied_rate = 0.0;
        self.pending_seek = Some(state);
        Ok(())
    }

    fn poll(&mut self) -> Result<Snapshot, MediaError> {
        autoreleasepool(|_| {
            let src = self.source.status()?;
            // SAFETY: All messages target retained AVFoundation objects.
            unsafe {
                let status: isize = msg_send![&*self.item, status];
                if status == 2 {
                    let error: Option<Retained<AnyObject>> = msg_send![&*self.item, error];
                    let message = if let Some(error) = error {
                        let text: Retained<AnyObject> = msg_send![&*error, localizedDescription];
                        let bytes: *const std::ffi::c_char = msg_send![&*text, UTF8String];
                        if bytes.is_null() {
                            "the system media player failed".to_owned()
                        } else {
                            CStr::from_ptr(bytes).to_string_lossy().into_owned()
                        }
                    } else {
                        "the system media player failed".to_owned()
                    };
                    return Err(MediaError::decode(message));
                }
                let mut seeking = false;
                if let Some(pending) = self.pending_seek.as_ref() {
                    match pending.load(Ordering::Acquire) {
                        0 => seeking = true,
                        1 => self.pending_seek = None,
                        _ => return Err(MediaError::decode("the system media seek failed")),
                    }
                }
                if status != 1 {
                    return Ok(Snapshot {
                        src,
                        seeking,
                        waiting: !self.paused && self.rate > 0.0,
                        ..Snapshot::default()
                    });
                }
                if !self.checked {
                    check_video(&self.item)?;
                    self.checked = true;
                }
                let duration: CMTime = msg_send![&*self.item, duration];
                let duration = duration.seconds();
                if !duration.is_finite() || duration <= 0.0 {
                    return Err(MediaError::unsupported(
                        "the video must have a finite positive duration",
                    ));
                }
                let desired = if self.paused || seeking { 0.0 } else { self.rate as f32 };
                if desired != self.applied_rate {
                    let _: () = msg_send![&*self.player, setRate: desired];
                    self.applied_rate = desired;
                }
                let time: CMTime = msg_send![&*self.player, currentTime];
                let position = time.seconds();
                let position = if position.is_finite() {
                    position.clamp(0.0, duration)
                } else {
                    0.0
                };
                let size: Size = msg_send![&*self.item, presentationSize];
                let width = if size.width.is_finite() && size.width > 0.0 {
                    size.width.min(u32::MAX as f64) as u32
                } else {
                    0
                };
                let height = if size.height.is_finite() && size.height > 0.0 {
                    size.height.min(u32::MAX as f64) as u32
                } else {
                    0
                };
                let loaded: Retained<AnyObject> = msg_send![&*self.item, loadedTimeRanges];
                let seekable: Retained<AnyObject> = msg_send![&*self.item, seekableTimeRanges];
                let likely: Bool = msg_send![&*self.item, isPlaybackLikelyToKeepUp];
                let empty: Bool = msg_send![&*self.item, isPlaybackBufferEmpty];
                let control: isize = msg_send![&*self.player, timeControlStatus];
                let waiting = !self.paused
                    && self.rate > 0.0
                    && !seeking
                    && (control == 1 || empty.as_bool());
                Ok(Snapshot {
                    duration: Some(duration),
                    position,
                    width,
                    height,
                    buffered: ranges(&loaded),
                    seekable: ranges(&seekable),
                    seeking,
                    waiting,
                    playing: !self.paused && !seeking && control == 2,
                    enough: likely.as_bool(),
                    frame: if seeking { None } else { frame(&self.output, time)? },
                    src,
                })
            }
        })
    }
}
