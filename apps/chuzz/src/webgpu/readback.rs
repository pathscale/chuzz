use blitz_dom::node::RasterImageData;

use std::sync::atomic::AtomicU64;

#[path = "readback_script.rs"]
mod script;

pub(super) use script::image;

use blitz_control_protocol::{CapturedImage, DebugError};

use super::buffer::map_read;
use super::external::Source;
use super::resources::{ALLOCATION_LIMIT, checked};
use super::*;

pub(super) type ImageResult = Result<Arc<RasterImageData>, GpuError>;

#[derive(Default)]
pub(super) struct Completion {
    result: Mutex<Option<ImageResult>>,
    wake: nagoya::sync::Notify,
}

impl Completion {
    pub(super) fn finish(&self, result: ImageResult) {
        if cfg!(feature = "reactor-witness")
            && std::env::var("CHUZZ_REACTOR_DEFECT").as_deref() == Ok("canvas-readback-drop")
        {
            return;
        }
        let mut current = lock(&self.result);
        if current.is_none() {
            *current = Some(result);
            drop(current);
            self.wake.notify_one();
        }
    }

    fn wait(&self) -> ImageResult {
        nagoya::block_on(nagoya::timeout(DEADLINE, async {
            loop {
                if let Some(result) = lock(&self.result).take() {
                    return result;
                }
                self.wake.notified().await;
            }
        }))
        .unwrap_or_else(|_| Err(GpuError::internal("canvas readback timed out in chuzz")))
    }
}

/// Resolve ordered native image work without allocating page pixel memory.
pub(super) fn read_image(
    bridge: &Bridge,
    source: Source,
    id: u64,
    snapshot: impl Fn(u64) -> Option<Arc<RasterImageData>>,
) -> Result<ImageResult, GpuError> {
    if !matches!(
        &source,
        // Texture: a WebGPU canvas read in the same task as its render(), before the frame is presented.
        Source::Canvas { .. }
            | Source::Element { .. }
            | Source::Bitmap { .. }
            | Source::Texture { .. }
            | Source::Video { .. }
    ) {
        return Err(GpuError::validation(
            "invalid synchronous canvas image source",
        ));
    }
    if !(1..=9_007_199_254_740_991).contains(&id) {
        return Err(GpuError::validation("invalid canvas readback handle"));
    }
    let incoming = {
        let mut state = bridge.state();
        if state.incoming.len() + state.in_flight.len() >= REQUEST_LIMIT
            || state.in_flight.contains_key(&id)
            || state.incoming.iter().any(|request| request.id() == id)
        {
            return Err(GpuError::validation(
                "canvas readback request limit reached in chuzz",
            ));
        }
        std::mem::take(&mut state.incoming)
    };
    for request in incoming {
        bridge.prepare_image_request(request, &snapshot);
    }
    let completion = Arc::new(Completion::default());
    bridge.prepare_image_request(
        Request::Readback {
            id,
            source,
            completion: Arc::clone(&completion),
        },
        snapshot,
    );

    // All preceding GPU and image work stays on the ordered Nagoya worker.
    // No DOM borrow crosses this wait, and no page jobs run reentrantly.
    Ok(completion.wait())
}

impl Bridge {
    pub(crate) fn install_canvas_readback(&self, document: &mut blitz_script::ScriptDocument) {
        script::install(self, document);
    }
}

const CAPTURE_WAIT: Duration = Duration::from_secs(8);

pub(super) fn next_capture_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1u64 << 62);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

enum ParsedTarget {
    Texture(String),
    Canvas,
}

fn parse_capture_target(target: &str) -> Result<ParsedTarget, GpuError> {
    if let Some(label) = target.strip_prefix("texture:") {
        if !label.is_empty() {
            return Ok(ParsedTarget::Texture(label.to_owned()));
        }
    } else if let Some(id) = target.strip_prefix("canvas:#")
        && !id.is_empty()
        && !id.chars().any(char::is_whitespace)
    {
        return Ok(ParsedTarget::Canvas);
    }
    Err(GpuError::validation(format!(
        "unknown WebGPU capture target: {target}"
    )))
}

fn as_debug(error: GpuError) -> DebugError {
    DebugError {
        code: error.kind.to_owned(),
        message: error.message,
    }
}

fn aligned_bytes_per_row(row: u32) -> Result<u32, GpuError> {
    row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        .checked_mul(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        .ok_or_else(|| GpuError::validation("WebGPU capture row overflow"))
}

#[derive(Clone, Copy)]
enum StoredPixels {
    Unorm8 { bgra: bool },
    Float16,
}

fn stored_pixels(format: wgpu::TextureFormat) -> Result<StoredPixels, GpuError> {
    let name = format!("{format:?}").to_ascii_lowercase();
    if name.contains("depth") {
        return Err(GpuError::validation("depth textures cannot be captured"));
    }
    match format {
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => {
            Ok(StoredPixels::Unorm8 { bgra: false })
        }
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => {
            Ok(StoredPixels::Unorm8 { bgra: true })
        }
        wgpu::TextureFormat::Rgba16Float => Ok(StoredPixels::Float16),
        other => Err(GpuError::validation(format!(
            "WebGPU texture format {other:?} cannot be captured"
        ))),
    }
}

/// IEEE 754 binary16 to f32 (the `f16` primitive is not stable).
fn f32_from_f16_bits(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let mantissa = f32::from(bits & 0x03ff);
    match exponent {
        0 => sign * mantissa * 2f32.powi(-24),
        0x1f if mantissa == 0.0 => sign * f32::INFINITY,
        0x1f => f32::NAN,
        _ => sign * (1.0 + mantissa / 1024.0) * 2f32.powi(exponent - 15),
    }
}

fn unorm8_from_f16(bits: u16) -> u8 {
    let value = f32_from_f16_bits(bits);
    if !value.is_finite() {
        return 0;
    }
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn straight_alpha(pixel: &mut [u8; 4]) {
    let alpha = u32::from(pixel[3]);
    if alpha == 0 {
        pixel[..3].fill(0);
    } else if alpha < 255 {
        for channel in &mut pixel[..3] {
            *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
        }
    }
}

enum CanvasAlpha {
    Opaque,
    Premultiplied,
}

pub(super) struct CaptureSlot {
    result: Mutex<Option<Result<CapturedImage, DebugError>>>,
    wake: nagoya::sync::Notify,
}

impl CaptureSlot {
    pub(super) fn finish(&self, result: Result<CapturedImage, DebugError>) {
        let mut current = lock(&self.result);
        if current.is_none() {
            *current = Some(result);
            drop(current);
            self.wake.notify_one();
        }
    }

    fn take(&self) -> Option<Result<CapturedImage, DebugError>> {
        lock(&self.result).take()
    }
}

struct CaptureCancel {
    slot: Arc<CaptureSlot>,
    committed: bool,
}

impl Drop for CaptureCancel {
    fn drop(&mut self) {
        if !self.committed {
            self.slot.finish(Err(DebugError {
                code: "internal".to_owned(),
                message: "WebGPU capture timed out in chuzz".to_owned(),
            }));
        }
    }
}

impl Bridge {
    pub(super) fn capture_and_wait(&self, target: &str) -> Result<CapturedImage, DebugError> {
        parse_capture_target(target).map_err(as_debug)?;
        let id = next_capture_id();
        let slot = Arc::new(CaptureSlot {
            result: Mutex::new(None),
            wake: nagoya::sync::Notify::new(),
        });
        self.enqueue(Request::Capture {
            id,
            target: target.to_owned(),
            slot: Arc::clone(&slot),
        });
        nagoya::block_on(nagoya::timeout(CAPTURE_WAIT, async {
            loop {
                if let Some(result) = slot.take() {
                    return result;
                }
                slot.wake.notified().await;
            }
        }))
        .unwrap_or_else(|_| {
            Err(DebugError {
                code: "internal".to_owned(),
                message: "WebGPU capture timed out in chuzz".to_owned(),
            })
        })
    }

    pub(super) async fn capture_on_worker(&self, target: &str, slot: Arc<CaptureSlot>) {
        let mut cancel = CaptureCancel {
            slot: Arc::clone(&slot),
            committed: false,
        };
        let result = self.read_capture_target(target).await;
        slot.finish(result);
        cancel.committed = true;
    }

    async fn read_capture_target(&self, target: &str) -> Result<CapturedImage, DebugError> {
        match parse_capture_target(target).map_err(as_debug)? {
            ParsedTarget::Texture(label) => self.read_labelled_texture(target, &label).await,
            ParsedTarget::Canvas => self.read_canvas(target).await,
        }
    }

    async fn read_labelled_texture(
        &self,
        target: &str,
        label: &str,
    ) -> Result<CapturedImage, DebugError> {
        let devices = self.state().devices.values().cloned().collect::<Vec<_>>();
        let mut found = Vec::new();
        for device in devices {
            let matched = {
                let resources = device.resources();
                let mut matched = Vec::new();
                for (&handle, entry) in &resources.textures {
                    if device.inspection_label(handle) == label {
                        matched.push(Arc::clone(&entry.texture));
                    }
                }
                matched
            };
            for texture in matched {
                found.push((Arc::clone(&device), texture));
            }
        }
        let (device, texture) = match found.len() {
            0 => {
                return Err(as_debug(GpuError::validation(format!(
                    "unknown WebGPU capture target: {target}"
                ))));
            }
            1 => found.pop().unwrap(),
            _ => {
                return Err(as_debug(GpuError::validation(format!(
                    "more than one WebGPU texture is labelled {label:?}"
                ))));
            }
        };
        device.read_texture_rgba(texture, None).await
    }

    async fn read_canvas(&self, target: &str) -> Result<CapturedImage, DebugError> {
        let devices = self.state().devices.values().cloned().collect::<Vec<_>>();
        let mut found = Vec::new();
        for device in &devices {
            for handle in device.canvases_named(target) {
                found.push((Arc::clone(device), handle));
            }
        }
        let (device, canvas) = match found.len() {
            0 => {
                return Err(as_debug(GpuError::validation(format!(
                    "unknown WebGPU capture target: {target}"
                ))));
            }
            1 => found.pop().unwrap(),
            _ => {
                return Err(as_debug(GpuError::validation(format!(
                    "more than one WebGPU canvas matches {target}"
                ))));
            }
        };
        let (generation, alpha, live) = {
            let resources = device.resources();
            let Some(configuration) = resources.canvases.get(&canvas) else {
                return Err(as_debug(GpuError::validation(format!(
                    "unknown WebGPU capture target: {target}"
                ))));
            };
            let alpha = if configuration.alpha_mode == "opaque" {
                CanvasAlpha::Opaque
            } else {
                CanvasAlpha::Premultiplied
            };
            let mut live = Vec::new();
            for entry in resources.textures.values() {
                if entry.canvas == Some((canvas, configuration.generation)) {
                    live.push(Arc::clone(&entry.texture));
                }
            }
            (configuration.generation, alpha, live)
        };
        if live.len() > 1 {
            return Err(as_debug(GpuError::validation(format!(
                "more than one WebGPU canvas texture matches {target}"
            ))));
        }
        if let Some(texture) = live.into_iter().next() {
            return device.read_texture_rgba(texture, Some(alpha)).await;
        }
        // Paint deliveries are drained by the renderer. The image cache retains
        // the last presented straight RGBA pixels after that delivery, even
        // when the application does not submit another frame.
        let image = {
            let state = self.state();
            let image = state
                .images
                .canvases
                .get(&canvas)
                .filter(|(image_generation, _)| *image_generation == generation);
            let Some((_, image)) = image else {
                return Err(as_debug(GpuError::validation(format!(
                    "canvas {target} has no texture to capture"
                ))));
            };
            Arc::clone(image)
        };
        CapturedImage::from_rgba(image.width, image.height, image.data.data())
    }
}

impl DeviceConnection {
    async fn read_texture_rgba(
        &self,
        texture: Arc<wgpu::Texture>,
        alpha: Option<CanvasAlpha>,
    ) -> Result<CapturedImage, DebugError> {
        if self.lost.load(Ordering::Acquire) {
            return Err(as_debug(GpuError::validation(
                "GPUDevice is lost or destroyed",
            )));
        }
        let layout = stored_pixels(texture.format()).map_err(as_debug)?;
        if texture.dimension() != wgpu::TextureDimension::D2 || texture.depth_or_array_layers() != 1
        {
            return Err(as_debug(GpuError::validation(
                "only a 2D texture can be captured",
            )));
        }
        if texture.sample_count() != 1 {
            return Err(as_debug(GpuError::validation(
                "multisampled textures cannot be captured",
            )));
        }
        if !texture.usage().contains(wgpu::TextureUsages::COPY_SRC) {
            return Err(as_debug(GpuError::validation(
                "texture cannot be captured without COPY_SRC",
            )));
        }
        let width = texture.width();
        let height = texture.height();
        if width == 0 || height == 0 {
            return Err(as_debug(GpuError::validation(
                "WebGPU texture has no pixels to capture",
            )));
        }
        let bpp = match layout {
            StoredPixels::Unorm8 { .. } => 4,
            StoredPixels::Float16 => 8,
        };
        let row = width
            .checked_mul(bpp)
            .ok_or_else(|| as_debug(GpuError::validation("WebGPU capture row overflow")))?;
        let padded = aligned_bytes_per_row(row).map_err(as_debug)?;
        let bytes = u64::from(padded)
            .checked_mul(u64::from(height))
            .ok_or_else(|| as_debug(GpuError::validation("WebGPU capture row overflow")))?;
        if bytes == 0 || bytes > ALLOCATION_LIMIT {
            return Err(as_debug(GpuError::validation(
                "WebGPU capture is larger than the readback limit",
            )));
        }
        let buffer = checked(&self.device, || {
            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("chuzz gpu capture"),
                size: bytes,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("chuzz gpu capture"),
                });
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(padded),
                        rows_per_image: Some(height),
                    },
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
            self.submit_gpu([encoder.finish()]);
            Arc::new(buffer)
        })
        .await
        .map_err(as_debug)?;
        let readback = map_read(&self.device, Arc::clone(&buffer), 0..bytes)
            .await
            .map_err(as_debug)?;
        buffer.unmap();
        if readback.len() as u64 != bytes {
            return Err(as_debug(GpuError::internal(
                "WebGPU capture readback was shorter than the texture",
            )));
        }
        let pixels = (width as usize)
            .checked_mul(height as usize)
            .and_then(|count| count.checked_mul(4))
            .ok_or_else(|| as_debug(GpuError::validation("WebGPU capture row overflow")))?;
        let mut rgba = Vec::with_capacity(pixels);
        let bgra = matches!(layout, StoredPixels::Unorm8 { bgra: true });
        for source in readback.chunks_exact(padded as usize) {
            let row_bytes = &source[..row as usize];
            match layout {
                StoredPixels::Unorm8 { .. } => {
                    for pixel in row_bytes.as_chunks::<4>().0 {
                        let mut pixel = [pixel[0], pixel[1], pixel[2], pixel[3]];
                        if bgra {
                            pixel.swap(0, 2);
                        }
                        apply_canvas_alpha(&mut pixel, alpha.as_ref());
                        rgba.extend_from_slice(&pixel);
                    }
                }
                StoredPixels::Float16 => {
                    for pixel in row_bytes.as_chunks::<8>().0 {
                        let mut out = [0u8; 4];
                        for channel in 0..4 {
                            let bits =
                                u16::from_le_bytes([pixel[channel * 2], pixel[channel * 2 + 1]]);
                            out[channel] = unorm8_from_f16(bits);
                        }
                        apply_canvas_alpha(&mut out, alpha.as_ref());
                        rgba.extend_from_slice(&out);
                    }
                }
            }
        }
        if rgba.len() != pixels {
            return Err(as_debug(GpuError::internal(
                "WebGPU capture readback was shorter than the texture",
            )));
        }
        CapturedImage::from_rgba(width, height, &rgba)
    }
}

fn apply_canvas_alpha(pixel: &mut [u8; 4], alpha: Option<&CanvasAlpha>) {
    match alpha {
        Some(CanvasAlpha::Opaque) => pixel[3] = 255,
        Some(CanvasAlpha::Premultiplied) => straight_alpha(pixel),
        None => {}
    }
}
