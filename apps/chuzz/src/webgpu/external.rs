use std::io::Cursor;

use blitz_dom::Document as _;
use blitz_dom::node::{ImageData, RasterImageData, SpecialElementData};

use super::buffer::map_read;
use super::resources::{ALLOCATION_LIMIT, checked, decode};
use super::*;

#[derive(Default)]
pub(super) struct Images {
    pub bitmaps: HashMap<u64, Arc<RasterImageData>>,
    snapshots: HashMap<u64, Arc<RasterImageData>>,
    pub canvases: HashMap<u64, (u64, Arc<RasterImageData>)>,
    pub paint: HashMap<u64, Arc<RasterImageData>>,
    pub image_cache: Arc<Mutex<anyrender_vello_cpu::ImageCache>>,
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub(super) enum Source {
    Bitmap {
        handle: u64,
    },
    Element {
        handle: u64,
    },
    Video {
        handle: u64,
    },
    Encoded {
        data: u64,
        #[serde(default)]
        node: Option<u64>,
    },
    Pixels {
        width: u32,
        height: u32,
        data: u64,
    },
    Canvas {
        canvas: u64,
        generation: u64,
        width: u32,
        height: u32,
    },
    Texture {
        device: u64,
        texture: u64,
        #[serde(rename = "premultipliedAlpha")]
        premultiplied_alpha: bool,
    },
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BitmapOptions {
    /// Taken out before the remaining options shape the bitmap.
    source: Option<Source>,
    crop: Option<[i32; 4]>,
    flip_y: bool,
    resize_width: Option<u32>,
    resize_height: Option<u32>,
    resize_quality: String,
    read_pixels: bool,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Paint {
    node: u64,
    source: Source,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Copy {
    source: Source,
    source_origin: [u32; 2],
    flip_y: bool,
    texture: u64,
    mip_level: u32,
    origin: [u32; 3],
    aspect: String,
    premultiplied_alpha: bool,
    color_space: String,
    size: [u32; 3],
}

fn byte_count(width: u32, height: u32) -> Result<usize, GpuError> {
    let count = u64::from(width) * u64::from(height) * 4;
    if width == 0 || height == 0 || count > ALLOCATION_LIMIT {
        return Err(GpuError::validation("invalid or oversized image in chuzz"));
    }
    Ok(count as usize)
}

fn might_be_svg(bytes: &[u8]) -> bool {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    bytes[start..].starts_with(b"<")
}

fn svg_pixels(value: f32) -> Result<u32, GpuError> {
    if !value.is_finite() || value <= 0.0 {
        return Err(GpuError::validation("svg image has no drawable size"));
    }
    let rounded = value.round();
    if rounded < 1.0 || rounded > u16::MAX as f32 {
        return Err(GpuError::validation(
            "svg image size is invalid or too large",
        ));
    }
    Ok(rounded as u32)
}

fn unpremultiply(source: &[u8], target: &mut [u8]) {
    for (source, target) in source.chunks_exact(4).zip(target.chunks_exact_mut(4)) {
        let alpha = u32::from(source[3]);
        target[3] = source[3];
        for index in 0..3 {
            target[index] = if alpha == 0 {
                0
            } else {
                ((u32::from(source[index]) * 255 + alpha / 2) / alpha).min(255) as u8
            };
        }
    }
}

fn decode_raster(data: &[u8]) -> Result<Arc<RasterImageData>, GpuError> {
    let make_reader = || {
        image::ImageReader::new(Cursor::new(data))
            .with_guessed_format()
            .map_err(|error| GpuError::validation(error.to_string()))
    };
    let (width, height) = make_reader()?
        .into_dimensions()
        .map_err(|error| GpuError::validation(error.to_string()))?;
    byte_count(width, height)?;
    let mut reader = make_reader()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(ALLOCATION_LIMIT);
    reader.limits(limits);
    let rgba = reader
        .decode()
        .map_err(|error| GpuError::validation(error.to_string()))?
        .into_rgba8();
    Ok(Arc::new(RasterImageData::new(
        rgba.width(),
        rgba.height(),
        Arc::new(rgba.into_raw()),
    )))
}

fn decode_encoded(data: &[u8]) -> Result<Arc<RasterImageData>, GpuError> {
    // One JPEG XL decoder for <img> and script images: blitz-dom's (first keyframe, sRGB, RGBA8).
    if blitz_dom::net::is_jxl(data) {
        let (width, height, rgba) = blitz_dom::net::decode_jxl(data)
            .map_err(|error| GpuError::validation(format!("JPEG XL: {error}")))?;
        return Ok(Arc::new(RasterImageData::new(
            width,
            height,
            Arc::new(rgba),
        )));
    }
    match decode_raster(data) {
        Ok(image) => Ok(image),
        Err(raster_error) => decode_svg(data)?.ok_or(raster_error),
    }
}

/// `image` does not decode SVG. Rasterize with the painter blitz-paint already
/// uses, at usvg's resolved size (root width/height, otherwise the viewBox).
/// The CPU pixmap is premultiplied; ImageBitmap storage is straight RGBA.
#[cfg(any(feature = "javascript", feature = "capture"))]
fn decode_svg(bytes: &[u8]) -> Result<Option<Arc<RasterImageData>>, GpuError> {
    use anyrender_vello_cpu::VelloCpuScenePainter;
    use blitz_dom::kurbo::Affine;

    if !might_be_svg(bytes) {
        return Ok(None);
    }
    let tree =
        match anyrender_svg::usvg::Tree::from_data(bytes, &anyrender_svg::usvg::Options::default())
        {
            Ok(tree) => tree,
            Err(_) => return Ok(None),
        };
    let size = tree.size();
    let width = svg_pixels(size.width())?;
    let height = svg_pixels(size.height())?;
    let count = byte_count(width, height)?;
    let mut painter = VelloCpuScenePainter::new(width as u16, height as u16);
    let transform = Affine::scale_non_uniform(
        f64::from(width) / f64::from(size.width()),
        f64::from(height) / f64::from(size.height()),
    );
    anyrender_svg::render_svg_tree(&mut painter, &tree, transform);
    let pixmap = painter.finish_shared();
    let source = pixmap.as_ref();
    if source.len() != count {
        return Err(GpuError::validation(
            "svg raster size did not match the image",
        ));
    }
    let mut straight = vec![0; count];
    unpremultiply(source, &mut straight);
    Ok(Some(Arc::new(RasterImageData::new(
        width,
        height,
        Arc::new(straight),
    ))))
}

#[cfg(not(any(feature = "javascript", feature = "capture")))]
fn decode_svg(_bytes: &[u8]) -> Result<Option<Arc<RasterImageData>>, GpuError> {
    Ok(None)
}

fn marker_matches(element: &blitz_dom::node::ElementData, marker: &str) -> bool {
    element.attrs().iter().any(|attribute| {
        attribute.name.local.as_ref() == "data-chuzz-gpu-source"
            && attribute.value.as_ref() == marker
    })
}

fn snapshot(document: &blitz_script::ScriptDocument, marker: u64) -> Option<Arc<RasterImageData>> {
    let marker = marker.to_string();
    let document = document.inner();
    document.tree().iter().find_map(|(_, node)| {
        let element = node.element_data()?;
        if !marker_matches(element, &marker) {
            return None;
        }
        // RasterImageData::clone clones its Blob, sharing the decoded pixels.
        element.raster_image_data().cloned().map(Arc::new)
    })
}

fn paint(
    document: &mut blitz_script::ScriptDocument,
    marker: u64,
    image: Arc<RasterImageData>,
) -> bool {
    let marker = marker.to_string();
    let mut document = document.inner_mut();
    let id = document.tree().iter().find_map(|(id, node)| {
        node.element_data()
            .filter(|element| marker_matches(element, &marker))
            .map(|_| id)
    });
    let Some(node) = id.and_then(|id| document.get_node_mut(id)) else {
        return false;
    };
    let Some(element) = node.element_data_mut() else {
        return false;
    };
    element.special_data = SpecialElementData::Image(Box::new(ImageData::Raster((*image).clone())));
    node.cache_mut().clear();
    if let Some(damage) = node.damage() {
        node.insert_damage(damage.union(damage.complement()));
    }
    node.set_dirty_descendants();
    node.mark_ancestors_dirty();
    true
}

impl Bridge {
    pub(super) fn prepare_images(&self, document: &mut blitz_script::ScriptDocument) -> bool {
        let (incoming, images) = {
            let mut state = self.state();
            let live = state
                .in_flight
                .keys()
                .copied()
                .collect::<std::collections::HashSet<_>>();
            state.images.snapshots.retain(|id, _| live.contains(id));
            (
                std::mem::take(&mut state.incoming),
                std::mem::take(&mut state.images.paint),
            )
        };
        let mut changed = false;
        for (marker, image) in images {
            changed |= paint(document, marker, image);
        }
        for request in incoming {
            self.prepare_image_request(request, |marker| snapshot(document, marker));
        }
        changed
    }

    pub(super) fn prepare_image_request(
        &self,
        mut request: Request,
        snapshot: impl Fn(u64) -> Option<Arc<RasterImageData>>,
    ) {
        match &mut request {
            Request::Command { id, command, .. } => {
                if let Some(source) = command.args.get_mut("source") {
                    let kind = source.get("kind").and_then(Value::as_str);
                    let marker = match kind {
                        Some("element") => source.get("handle").and_then(Value::as_u64),
                        Some("encoded") => source.get("node").and_then(Value::as_u64),
                        _ => None,
                    };
                    if let Some(marker) = marker {
                        let image = snapshot(marker);
                        if kind == Some("element") || image.is_some() {
                            if let Some(image) = image {
                                self.state().images.snapshots.insert(*id, image);
                            }
                            *source = json!({"kind": "element", "handle": *id});
                        }
                    }
                }
            }
            Request::Readback {
                id,
                source: Source::Element { handle },
                ..
            } => {
                if let Some(image) = snapshot(*handle) {
                    self.state().images.snapshots.insert(*id, image);
                }
                *handle = *id;
            }
            _ => {}
        }
        self.enqueue(request);
    }

    pub(super) async fn resolve_source(
        &self,
        source: Source,
    ) -> Result<Arc<RasterImageData>, GpuError> {
        match source {
            Source::Video { handle } => {
                let frames = Arc::clone(&self.state().media_frames);
                frames
                    .take(handle)
                    .map(|frame| frame.image)
                    .map_err(GpuError::validation)
            }
            Source::Bitmap { handle } => self
                .state()
                .images
                .bitmaps
                .get(&handle)
                .cloned()
                .ok_or_else(|| GpuError::validation("ImageBitmap is closed or invalid")),
            Source::Element { handle } => self
                .state()
                .images
                .snapshots
                .remove(&handle)
                .ok_or_else(|| GpuError::validation("image source has no decoded pixels")),
            Source::Pixels {
                width,
                height,
                data,
            } => {
                let data = self.byte_table().take_input(data)?;
                if data.len() != byte_count(width, height)? {
                    return Err(GpuError::validation("invalid ImageData byte length"));
                }
                Ok(Arc::new(RasterImageData::new(width, height, data)))
            }
            Source::Encoded { data, node } => {
                let data = self.byte_table().take_input(data)?;
                let _node = node;
                if data.len() as u64 > ALLOCATION_LIMIT {
                    return Err(GpuError::validation("image decode limit reached in chuzz"));
                }
                decode_encoded(data.as_slice())
            }
            Source::Canvas {
                canvas,
                generation,
                width,
                height,
            } => {
                let image = self
                    .state()
                    .images
                    .canvases
                    .get(&canvas)
                    .filter(|(current, image)| {
                        *current == generation && image.width == width && image.height == height
                    })
                    .map(|(_, image)| Arc::clone(image))
                    .ok_or_else(|| {
                        GpuError::validation("canvas image is unavailable or resized")
                    })?;
                Ok(image)
            }
            Source::Texture {
                device,
                texture,
                premultiplied_alpha,
            } => {
                let device = self
                    .state()
                    .devices
                    .get(&device)
                    .cloned()
                    .ok_or_else(|| GpuError::validation("invalid canvas GPUDevice"))?;
                device
                    .read_external_texture(texture, premultiplied_alpha)
                    .await
            }
        }
    }

    pub(super) async fn execute_image(&self, id: u64, command: Command) -> Result<Value, GpuError> {
        match command.op.as_str() {
            "closeImage" => {
                let (image, cache) = {
                    let mut state = self.state();
                    (
                        state.images.bitmaps.remove(&command.handle),
                        Arc::clone(&state.images.image_cache),
                    )
                };
                if let Some(image) = image {
                    lock(&cache).remove(image.data.id());
                }
                Ok(Value::Null)
            }
            "paintImage" => {
                let options: Paint = decode(command.args)?;
                let image = self.resolve_source(options.source).await?;
                self.state().images.paint.insert(options.node, image);
                Ok(Value::Null)
            }
            "createImageBitmap" => {
                let mut options: BitmapOptions = decode(command.args)?;
                let source = options
                    .source
                    .take()
                    .ok_or_else(|| GpuError::validation("createImageBitmap requires a source"))?;
                let source = self.resolve_source(source).await?;
                let image = bitmap(source, &options)?;
                let mut state = self.state();
                if !state.in_flight.get(&id).is_some_and(host::Flight::live) {
                    return Err(GpuError::internal("image request expired in chuzz"));
                }
                let bytes = state
                    .images
                    .bitmaps
                    .values()
                    .map(|image| image.data.len() as u64)
                    .sum::<u64>();
                if command.handle == 0
                    || state.images.bitmaps.contains_key(&command.handle)
                    || state.images.bitmaps.len() >= 8192
                    || bytes.saturating_add(image.data.len() as u64) > 512 * 1024 * 1024
                {
                    return Err(GpuError::validation(
                        "ImageBitmap allocation limit reached in chuzz",
                    ));
                }
                let data = if options.read_pixels {
                    let id = state
                        .bytes
                        .insert(binary::Block::Image(Arc::clone(&image)), false)?;
                    json!({"byteId": id})
                } else {
                    Value::Null
                };
                let value = json!({
                    "handle": command.handle,
                    "width": image.width,
                    "height": image.height,
                    "data": data,
                });
                state.images.bitmaps.insert(command.handle, image);
                Ok(value)
            }
            member => Err(GpuError::validation(format!(
                "not implemented in chuzz: {member}"
            ))),
        }
    }
}

fn bitmap(
    source: Arc<RasterImageData>,
    options: &BitmapOptions,
) -> Result<Arc<RasterImageData>, GpuError> {
    let [mut x, mut y, mut width, mut height] = options.crop.unwrap_or([
        0,
        0,
        i32::try_from(source.width).map_err(|_| GpuError::validation("image width overflow"))?,
        i32::try_from(source.height).map_err(|_| GpuError::validation("image height overflow"))?,
    ]);
    if width < 0 {
        x = x
            .checked_add(width)
            .ok_or_else(|| GpuError::validation("image crop overflow"))?;
        width = width
            .checked_neg()
            .ok_or_else(|| GpuError::validation("image crop overflow"))?;
    }
    if height < 0 {
        y = y
            .checked_add(height)
            .ok_or_else(|| GpuError::validation("image crop overflow"))?;
        height = height
            .checked_neg()
            .ok_or_else(|| GpuError::validation("image crop overflow"))?;
    }
    let width = u32::try_from(width).map_err(|_| GpuError::validation("invalid image crop"))?;
    let height = u32::try_from(height).map_err(|_| GpuError::validation("invalid image crop"))?;
    byte_count(width, height)?;
    let target_width = options.resize_width.unwrap_or_else(|| {
        options.resize_height.map_or(width, |target| {
            ((u64::from(width) * u64::from(target)).div_ceil(u64::from(height)))
                .min(u64::from(u32::MAX)) as u32
        })
    });
    let target_height = options.resize_height.unwrap_or_else(|| {
        options.resize_width.map_or(height, |target| {
            ((u64::from(height) * u64::from(target)).div_ceil(u64::from(width)))
                .min(u64::from(u32::MAX)) as u32
        })
    });
    byte_count(target_width, target_height)?;
    let filter = match options.resize_quality.as_str() {
        "pixelated" => image::imageops::FilterType::Nearest,
        "low" => image::imageops::FilterType::Triangle,
        "medium" => image::imageops::FilterType::CatmullRom,
        "high" => image::imageops::FilterType::Lanczos3,
        _ => {
            return Err(GpuError::validation(
                "invalid ImageBitmapOptions.resizeQuality",
            ));
        }
    };
    if x == 0
        && y == 0
        && width == source.width
        && height == source.height
        && target_width == width
        && target_height == height
        && !options.flip_y
    {
        return Ok(source);
    }
    let mut data = vec![0; byte_count(width, height)?];
    for row in 0..height {
        let source_y = i64::from(y)
            + i64::from(if options.flip_y {
                height - row - 1
            } else {
                row
            });
        if source_y < 0 || source_y >= i64::from(source.height) {
            continue;
        }
        for column in 0..width {
            let source_x = i64::from(x) + i64::from(column);
            if source_x < 0 || source_x >= i64::from(source.width) {
                continue;
            }
            let from = (source_y as usize * source.width as usize + source_x as usize) * 4;
            let to = (row as usize * width as usize + column as usize) * 4;
            data[to..to + 4].copy_from_slice(&source.data.data()[from..from + 4]);
        }
    }
    if target_width != width || target_height != height {
        let image = image::RgbaImage::from_raw(width, height, data)
            .ok_or_else(|| GpuError::validation("invalid bitmap storage"))?;
        data = image::imageops::resize(&image, target_width, target_height, filter).into_raw();
    }
    Ok(Arc::new(RasterImageData::new(
        target_width,
        target_height,
        Arc::new(data),
    )))
}

impl DeviceConnection {
    pub(super) async fn read_external_texture(
        &self,
        handle: u64,
        premultiplied: bool,
    ) -> Result<Arc<RasterImageData>, GpuError> {
        let texture = {
            let resources = self.resources();
            Arc::clone(
                &resources
                    .textures
                    .get(&handle)
                    .ok_or_else(|| GpuError::validation("canvas current texture is unavailable"))?
                    .texture,
            )
        };
        let bgra = match texture.format() {
            wgpu::TextureFormat::Rgba8Unorm => false,
            wgpu::TextureFormat::Bgra8Unorm => true,
            _ => {
                return Err(GpuError::validation(
                    "not implemented in chuzz: external canvas texture format",
                ));
            }
        };
        if !texture.usage().contains(wgpu::TextureUsages::COPY_SRC) {
            return Err(GpuError::validation("canvas texture lacks COPY_SRC"));
        }
        let width = texture.width();
        let height = texture.height();
        byte_count(width, height)?;
        let row = width * 4;
        let padded =
            row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let size = u64::from(padded) * u64::from(height);
        if size > ALLOCATION_LIMIT {
            return Err(GpuError::validation(
                "canvas readback limit reached in chuzz",
            ));
        }
        let buffer = checked(&self.device, || {
            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("chuzz external canvas readback"),
                size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = self.device.create_command_encoder(&Default::default());
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
            self.queue.submit([encoder.finish()]);
            Arc::new(buffer)
        })
        .await?;
        let bytes = map_read(&self.device, Arc::clone(&buffer), 0..size).await?;
        buffer.unmap();
        let mut rgba = Vec::with_capacity(byte_count(width, height)?);
        for row in bytes.chunks_exact(padded as usize) {
            for pixel in row[..width as usize * 4].chunks_exact(4) {
                let mut pixel = [pixel[0], pixel[1], pixel[2], pixel[3]];
                if bgra {
                    pixel.swap(0, 2);
                }
                if premultiplied {
                    let alpha = u32::from(pixel[3]);
                    for channel in &mut pixel[..3] {
                        *channel = if alpha == 0 {
                            0
                        } else {
                            ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8
                        };
                    }
                } else {
                    // Texture sources are live WebGPU canvases. The other
                    // supported alpha mode is opaque, matching present_canvas.
                    pixel[3] = 255;
                }
                rgba.extend_from_slice(&pixel);
            }
        }
        Ok(Arc::new(RasterImageData::new(
            width,
            height,
            Arc::new(rgba),
        )))
    }

    pub(super) async fn copy_external_image(&self, args: Value) -> Result<Value, GpuError> {
        let copy: Copy = decode(args)?;
        if copy.color_space != "srgb" {
            return Err(GpuError::validation(format!(
                "not implemented in chuzz: GPUImageCopyTextureTagged.colorSpace.{}",
                copy.color_space,
            )));
        }
        if copy.aspect != "all" || copy.size[2] != 1 {
            return Err(GpuError::validation(
                "invalid external image aspect or depth",
            ));
        }
        let owner = self
            .owner
            .upgrade()
            .ok_or_else(|| GpuError::internal("image document was destroyed"))?;
        let image = Bridge(owner).resolve_source(copy.source).await?;
        let [width, height, _] = copy.size;
        let [x, y] = copy.source_origin;
        if x > image.width
            || y > image.height
            || width > image.width - x
            || height > image.height - y
        {
            return Err(GpuError::validation(
                "external image source range is out of bounds",
            ));
        }
        let texture = {
            let resources = self.resources();
            Arc::clone(
                &resources
                    .textures
                    .get(&copy.texture)
                    .ok_or_else(|| GpuError::validation("invalid or destroyed GPUTexture"))?
                    .texture,
            )
        };
        let bgra = match texture.format() {
            wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => false,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => true,
            format => {
                return Err(GpuError::validation(format!(
                    "not implemented in chuzz: GPUQueue.copyExternalImageToTexture.format.{format:?}",
                )));
            }
        };
        if texture.dimension() != wgpu::TextureDimension::D2
            || texture.sample_count() != 1
            || !texture
                .usage()
                .contains(wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::RENDER_ATTACHMENT)
            || copy.mip_level >= texture.mip_level_count()
        {
            return Err(GpuError::validation("invalid external image destination"));
        }
        let target_width = (texture.width() >> copy.mip_level).max(1);
        let target_height = (texture.height() >> copy.mip_level).max(1);
        if copy.origin[0] > target_width
            || copy.origin[1] > target_height
            || width > target_width - copy.origin[0]
            || height > target_height - copy.origin[1]
            || copy.origin[2] >= texture.depth_or_array_layers()
        {
            return Err(GpuError::validation(
                "external image destination range is out of bounds",
            ));
        }
        if width == 0 || height == 0 {
            return Ok(Value::Null);
        }
        let converted = if copy.flip_y || copy.premultiplied_alpha || bgra {
            let mut data = Vec::with_capacity(byte_count(width, height)?);
            for row in 0..height {
                let row = y + if copy.flip_y { height - row - 1 } else { row };
                let start = (row as usize * image.width as usize + x as usize) * 4;
                for pixel in image.data.data()[start..start + width as usize * 4].chunks_exact(4) {
                    let mut pixel = [pixel[0], pixel[1], pixel[2], pixel[3]];
                    if copy.premultiplied_alpha {
                        let alpha = u32::from(pixel[3]);
                        for channel in &mut pixel[..3] {
                            *channel = ((u32::from(*channel) * alpha + 127) / 255) as u8;
                        }
                    }
                    if bgra {
                        pixel.swap(0, 2);
                    }
                    data.extend_from_slice(&pixel);
                }
            }
            Some(data)
        } else {
            None
        };
        // Ordinary RGBA8 uploads borrow the decoded Blob, including subrects.
        // Only a flip, alpha conversion or BGRA swizzle allocates another image.
        let (data, offset, bytes_per_row) = match &converted {
            Some(data) => (data.as_slice(), 0, width * 4),
            None => (
                image.data.data(),
                (u64::from(y) * u64::from(image.width) + u64::from(x)) * 4,
                image.width * 4,
            ),
        };
        checked(&self.device, || {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: copy.mip_level,
                    origin: wgpu::Origin3d {
                        x: copy.origin[0],
                        y: copy.origin[1],
                        z: copy.origin[2],
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout {
                    offset,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(height),
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
            self.queue.submit([]);
        })
        .await?;
        Ok(Value::Null)
    }
}
