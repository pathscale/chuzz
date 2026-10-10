//! Document-owned CPU canvases. Descriptors cross JSON; pixels cross native
//! ArrayBuffers. Native premultiplied tiles stay in Rust; straight RGBA is lazy.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyrender::{Paint, PaintScene};
use anyrender_vello_cpu::{ImageCache, VelloCpuScenePainter};
use blitz_dom::kurbo::{Affine, BezPath, Cap, Join, Rect, Shape, Stroke};
use blitz_dom::node::RasterImageData;
use blitz_script::boa_engine::object::builtins::{
    JsArray, JsArrayBuffer, JsUint8Array, JsUint8ClampedArray,
};
use blitz_script::boa_engine::{
    Context, JsNativeError, JsResult, JsString, JsValue, NativeFunction,
};
use parley::fontique::{Collection, CollectionOptions, SourceCache};
use peniko::{
    BlendMode, Blob, Color, Compose, Extend, Fill, Gradient, ImageAlphaType, ImageBrush, ImageData,
    ImageFormat, ImageQuality, ImageSampler, Mix,
};
use serde::Deserialize;

use super::{Bridge, lock};

mod backing;
mod effects;
mod paths;
mod text;

type Result<T> = std::result::Result<T, String>;

const IMAGE_LIMIT: usize = 64 * 1024 * 1024;
const DOCUMENT_LIMIT: usize = 512 * 1024 * 1024;
const COMMAND_LIMIT: usize = 8192;
// Elements in the current path. A page that strokes in a loop without beginPath grows one path across frames;
// Chrome keeps going (slowly), so this only stops what would otherwise exhaust memory.
const PATH_LIMIT: usize = 1 << 20;
const STACK_LIMIT: usize = 256;

fn identity() -> [f64; 6] {
    [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]
}

fn missing(member: &str) -> String {
    format!("not implemented in chuzz: CanvasRenderingContext2D.{member}")
}

fn numbers(values: &[f64], count: usize) -> Result<()> {
    if values.len() != count || values.iter().any(|n| !n.is_finite() || n.abs() > 1.0e7) {
        return Err("invalid or oversized canvas coordinates in chuzz".to_owned());
    }
    Ok(())
}

fn byte_count(width: u32, height: u32) -> Result<usize> {
    let length = u64::from(width) * u64::from(height) * 4;
    if length > IMAGE_LIMIT as u64 || width > u16::MAX as u32 || height > u16::MAX as u32 {
        return Err("canvas allocation limit reached in chuzz".to_owned());
    }
    Ok(length as usize)
}

fn rule(value: f64) -> Fill {
    if value == 1.0 {
        Fill::EvenOdd
    } else {
        Fill::NonZero
    }
}

fn color(value: &str) -> Result<Color> {
    value
        .parse::<peniko::color::DynamicColor>()
        .map(|color| color.to_alpha_color::<peniko::color::Srgb>())
        .map_err(|_| "invalid canvas colour".to_owned())
}

#[derive(Clone)]
struct Fonts(Arc<Mutex<parley::FontContext>>);

fn default_fonts() -> parley::FontContext {
    parley::FontContext {
        collection: Collection::new(CollectionOptions {
            shared: true,
            system_fonts: cfg!(feature = "system-fonts"),
        }),
        source_cache: SourceCache::new_shared(),
    }
}

pub(super) fn from_html(
    html: &str,
    mut config: blitz_dom::DocumentConfig,
) -> blitz_script::ScriptDocument {
    let mut fonts = config.font_ctx.take().unwrap_or_else(default_fonts);
    // Fontique shares registrations, aliases and generic-family changes with
    // every clone. Blitz's later @font-face loads reach the canvas too.
    fonts.collection.make_shared();
    fonts.source_cache.make_shared();
    config.font_ctx = Some(fonts.clone());
    let mut document = blitz_script::ScriptDocument::from_html(html, config);
    document.with_js_context(|context| {
        context.insert_data(Fonts(Arc::new(Mutex::new(fonts))));
    });
    document
}

#[derive(Clone)]
struct Host {
    canvases: Arc<Mutex<HashMap<u64, Canvas>>>,
    sources: Arc<Mutex<HashMap<u64, ImageData>>>,
    image_cache: Arc<Mutex<ImageCache>>,
    transferred: Arc<Mutex<u64>>,
    fonts: Fonts,
    bridge: Bridge,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    action: String,
    #[serde(default)]
    id: u64,
    #[serde(default)]
    width: u32,
    #[serde(default)]
    height: u32,
    #[serde(default)]
    commands: Vec<Command>,
    #[serde(default)]
    serial: u64,
    source: Option<super::external::Source>,
    #[serde(default)]
    n: Vec<f64>,
    #[serde(default)]
    value: String,
    #[serde(default)]
    text: String,
    state: Option<DrawState>,
    #[serde(default = "font_base")]
    font_base: f64,
    #[serde(default = "font_base")]
    root_base: f64,
}

fn font_base() -> f64 {
    16.0
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Command {
    op: String,
    #[serde(default)]
    n: Vec<f64>,
    #[serde(default = "identity")]
    matrix: [f64; 6],
    state: Option<DrawState>,
    brush: Option<Brush>,
    #[serde(default)]
    text: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DrawState {
    matrix: [f64; 6],
    alpha: f32,
    composite: String,
    width: f64,
    cap: String,
    join: String,
    miter: f64,
    dash: Vec<f64>,
    dash_offset: f64,
    font: String,
    align: String,
    baseline: String,
    direction: String,
    smoothing: bool,
    shadow_blur: f64,
    shadow_color: String,
    shadow_offset_x: f64,
    shadow_offset_y: f64,
    filter: Vec<effects::FilterOp>,
}

impl DrawState {
    fn stroke(&self) -> Result<Stroke> {
        numbers(&self.matrix, 6)?;
        if !self.width.is_finite()
            || self.width <= 0.0
            || self.width > 1.0e7
            || !self.miter.is_finite()
            || self.miter <= 0.0
            || self.miter > 1.0e7
            || !self.dash_offset.is_finite()
            || self.dash.len() > 1024
            || self
                .dash
                .iter()
                .any(|n| !n.is_finite() || *n < 0.0 || *n > 1.0e7)
        {
            return Err("invalid canvas stroke".to_owned());
        }
        let cap = match self.cap.as_str() {
            "butt" => Cap::Butt,
            "round" => Cap::Round,
            "square" => Cap::Square,
            _ => return Err("invalid canvas lineCap".to_owned()),
        };
        let join = match self.join.as_str() {
            "round" => Join::Round,
            "bevel" => Join::Bevel,
            "miter" => Join::Miter,
            _ => return Err("invalid canvas lineJoin".to_owned()),
        };
        let mut stroke = Stroke::new(self.width)
            .with_caps(cap)
            .with_join(join)
            .with_miter_limit(self.miter);
        if self.dash.iter().any(|n| *n > 0.0) {
            stroke = stroke.with_dashes(self.dash_offset, &self.dash);
        }
        Ok(stroke)
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Image {
    index: usize,
    width: u32,
    height: u32,
    offset: usize,
    length: usize,
    #[serde(default)]
    video: Option<u64>,
    #[serde(default)]
    canvas: Option<u64>,
    #[serde(default)]
    bitmap: Option<u64>,
    #[serde(default)]
    native: Option<u64>,
    #[serde(skip)]
    resolved: Option<ImageData>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Brush {
    kind: String,
    #[serde(default)]
    color: String,
    #[serde(default)]
    n: Vec<f64>,
    #[serde(default = "identity")]
    matrix: [f64; 6],
    #[serde(default)]
    stops: Vec<(f32, String)>,
    image: Option<Image>,
    #[serde(default)]
    repetition: String,
}

fn raster_image(image: &RasterImageData) -> ImageData {
    ImageData {
        data: image.data.clone(),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width: image.width,
        height: image.height,
    }
}

fn image_data(source: &Image, inputs: &[Arc<Vec<u8>>]) -> Result<ImageData> {
    if source.length != byte_count(source.width, source.height)? {
        return Err("invalid canvas image length".to_owned());
    }
    if let Some(image) = &source.resolved {
        if source.offset != 0
            || image.width != source.width
            || image.height != source.height
            || image.data.len() != source.length
        {
            return Err("invalid native canvas image descriptor".to_owned());
        }
        return Ok(image.clone());
    }
    let bytes = inputs
        .get(source.index)
        .ok_or("invalid canvas image buffer")?;
    let end = source
        .offset
        .checked_add(source.length)
        .ok_or("canvas image range overflow")?;
    let view = bytes
        .get(source.offset..end)
        .ok_or("invalid canvas image range")?;
    let data = if source.offset == 0 && end == bytes.len() {
        Arc::clone(bytes)
    } else {
        Arc::new(view.to_vec())
    };
    Ok(ImageData {
        data: Blob::new(data),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width: source.width,
        height: source.height,
    })
}

/// The pixel-aligned part of `source` covering `x0..x1` by `y0..y1`, as a
/// resolved image, and where it sits in the source. The whole image comes back
/// unchanged when the rectangle covers it.
fn crop_source(
    source: &Image,
    inputs: &[Arc<Vec<u8>>],
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
) -> Result<(Image, (f64, f64))> {
    let left = x0.floor() as u32;
    let top = y0.floor() as u32;
    let right = (x1.ceil() as u32).min(source.width);
    let bottom = (y1.ceil() as u32).min(source.height);
    if left == 0 && top == 0 && right == source.width && bottom == source.height {
        return Ok((source.clone(), (0.0, 0.0)));
    }
    let data = image_data(source, inputs)?;
    let bytes = data.data.data();
    let (width, height) = (right - left, bottom - top);
    let row = width as usize * 4;
    let mut cropped = Vec::with_capacity(row * height as usize);
    for y in top..bottom {
        let start = (y as usize * source.width as usize + left as usize) * 4;
        cropped.extend_from_slice(
            bytes
                .get(start..start + row)
                .ok_or("invalid canvas image range")?,
        );
    }
    let length = cropped.len();
    let image = Image {
        index: 0,
        width,
        height,
        offset: 0,
        length,
        video: None,
        canvas: None,
        bitmap: None,
        native: None,
        resolved: Some(ImageData {
            data: Blob::new(Arc::new(cropped)),
            format: data.format,
            alpha_type: data.alpha_type,
            width,
            height,
        }),
    };
    Ok((image, (f64::from(left), f64::from(top))))
}

fn image_brush(data: ImageData, smoothing: bool, x: Extend, y: Extend) -> Paint {
    Paint::Image(ImageBrush {
        image: data,
        sampler: ImageSampler {
            x_extend: x,
            y_extend: y,
            quality: if smoothing {
                ImageQuality::Medium
            } else {
                ImageQuality::Low
            },
            alpha: 1.0,
        },
    })
}

fn resolve_brush(
    brush: &Brush,
    inputs: &[Arc<Vec<u8>>],
    smoothing: bool,
) -> Result<(Paint, Affine, Option<BezPath>)> {
    numbers(&brush.matrix, 6)?;
    let transform = Affine::new(brush.matrix);
    let mut clip = None;
    let paint = match brush.kind.as_str() {
        "color" => Paint::Solid(color(&brush.color)?),
        "linear" | "radial" => {
            let count = if brush.kind == "linear" { 4 } else { 6 };
            numbers(&brush.n, count)?;
            if brush.stops.len() > 4096 {
                return Err("canvas gradient stop limit reached in chuzz".to_owned());
            }
            if brush.stops.is_empty() {
                return Ok((Paint::Solid(Color::TRANSPARENT), transform, None));
            }
            let mut stops = brush
                .stops
                .iter()
                .map(|(offset, value)| {
                    if !offset.is_finite() || !(0.0..=1.0).contains(offset) {
                        return Err("invalid canvas gradient stop".to_owned());
                    }
                    Ok((*offset, color(value)?))
                })
                .collect::<Result<Vec<_>>>()?;
            stops.sort_by(|a, b| a.0.total_cmp(&b.0));
            let n = &brush.n;
            let gradient = if brush.kind == "linear" {
                if n[0] == n[2] && n[1] == n[3] {
                    return Ok((Paint::Solid(Color::TRANSPARENT), transform, None));
                }
                Gradient::new_linear((n[0], n[1]), (n[2], n[3]))
            } else {
                if n[2] < 0.0 || n[5] < 0.0 {
                    return Err("negative canvas gradient radius".to_owned());
                }
                if n[0] == n[3] && n[1] == n[4] && n[2] == n[5] {
                    return Ok((Paint::Solid(Color::TRANSPARENT), transform, None));
                }
                Gradient::new_two_point_radial((n[0], n[1]), n[2] as f32, (n[3], n[4]), n[5] as f32)
            };
            Paint::Gradient(gradient.with_stops(stops.as_slice()))
        }
        "pattern" => {
            let source = brush
                .image
                .as_ref()
                .ok_or("canvas pattern requires pixels")?;
            let data = image_data(source, inputs)?;
            let (repeat_x, repeat_y) = match brush.repetition.as_str() {
                "repeat" => (true, true),
                "repeat-x" => (true, false),
                "repeat-y" => (false, true),
                "no-repeat" => (false, false),
                _ => return Err("invalid canvas pattern repetition".to_owned()),
            };
            if !repeat_x || !repeat_y {
                let rect = Rect::new(
                    if repeat_x { -1.0e7 } else { 0.0 },
                    if repeat_y { -1.0e7 } else { 0.0 },
                    if repeat_x { 1.0e7 } else { source.width as f64 },
                    if repeat_y {
                        1.0e7
                    } else {
                        source.height as f64
                    },
                );
                clip = Some(transform * rect.to_path(0.1));
            }
            image_brush(
                data,
                smoothing,
                if repeat_x {
                    Extend::Repeat
                } else {
                    Extend::Pad
                },
                if repeat_y {
                    Extend::Repeat
                } else {
                    Extend::Pad
                },
            )
        }
        _ => return Err(missing("fillStyle")),
    };
    Ok((paint, transform, clip))
}

#[derive(Clone)]
struct Clip {
    path: Arc<BezPath>,
    rule: Fill,
}

struct Canvas {
    width: u32,
    height: u32,
    backing: backing::Backing,
    image: Option<ImageData>,
    image_cache: Arc<Mutex<ImageCache>>,
    path: paths::Path,
    clips: Vec<Clip>,
    saves: Vec<usize>,
}

impl Drop for Canvas {
    fn drop(&mut self) {
        self.invalidate();
    }
}

impl Canvas {
    fn new(width: u32, height: u32, image_cache: Arc<Mutex<ImageCache>>) -> Result<Self> {
        byte_count(width, height)?;
        Ok(Self {
            width,
            height,
            backing: backing::Backing::new(width, height),
            image: None,
            image_cache,
            path: paths::Path::default(),
            clips: Vec::new(),
            saves: Vec::new(),
        })
    }

    fn invalidate(&mut self) {
        if let Some(image) = self.image.take() {
            lock(&self.image_cache).remove(image.data.id());
        }
    }

    fn source_image(&mut self) -> ImageData {
        self.flush();
        let pixels = self.backing.snapshot();
        self.image
            .get_or_insert_with(|| ImageData {
                data: Blob::new(pixels as Arc<dyn AsRef<[u8]> + Send + Sync>),
                format: ImageFormat::Rgba8,
                alpha_type: ImageAlphaType::Alpha,
                width: self.width,
                height: self.height,
            })
            .clone()
    }

    fn flush(&mut self) {
        self.backing.flush();
    }

    fn draw(
        &mut self,
        path: &BezPath,
        state: &DrawState,
        brush: &Brush,
        inputs: &[Arc<Vec<u8>>],
        stroke: bool,
        fill: Fill,
        clear: bool,
    ) -> Result<()> {
        let style = state.stroke()?;
        if !state.alpha.is_finite() || !(0.0..=1.0).contains(&state.alpha) {
            return Err("invalid canvas globalAlpha".to_owned());
        }
        if path.elements().is_empty()
            || self
                .clips
                .iter()
                .any(|clip| clip.path.elements().is_empty())
        {
            return Ok(());
        }
        let (paint, paint_transform, extra_clip) = resolve_brush(brush, inputs, state.smoothing)?;
        let blend = if clear {
            BlendMode::new(Mix::Normal, Compose::DestOut)
        } else {
            match state.composite.as_str() {
                "source-over" => BlendMode::new(Mix::Normal, Compose::SrcOver),
                "lighter" => BlendMode::new(Mix::Normal, Compose::Plus),
                "multiply" => BlendMode::new(Mix::Multiply, Compose::SrcOver),
                "destination-out" => BlendMode::new(Mix::Normal, Compose::DestOut),
                _ => {
                    return Err(missing(&format!(
                        "globalCompositeOperation.{}",
                        state.composite
                    )));
                }
            }
        };
        let transform = if stroke {
            Affine::new(state.matrix)
        } else {
            Affine::IDENTITY
        };
        if transform.determinant().abs() < 1.0e-20 || paint_transform.determinant().abs() < 1.0e-20
        {
            return Ok(());
        }
        if !clear {
            if let Some(effect) = effects::prepare(state)? {
                return self.draw_effect(
                    path,
                    state,
                    &style,
                    &paint,
                    paint_transform,
                    &extra_clip,
                    transform,
                    stroke,
                    fill,
                    blend,
                    &effect,
                );
            }
        }
        let shape = transform.inverse() * path.clone();
        let clips = self.clips.clone();
        let bounds = Rect::new(0.0, 0.0, self.width as f64, self.height as f64);
        let mut area = path.bounding_box().inflate(1.0, 1.0);
        if stroke {
            // Include caps, joins, miters and nonuniform transforms. The
            // extra pixel covers antialiasing and path flattening tolerance.
            let matrix = transform.as_coeffs();
            let radius = state.width * state.miter.max(2.0);
            area = area.inflate(
                radius * (matrix[0].abs() + matrix[2].abs()),
                radius * (matrix[1].abs() + matrix[3].abs()),
            );
        }
        for clip in &clips {
            area = area.intersect(clip.path.bounding_box().inflate(1.0, 1.0));
        }
        if let Some(clip) = &extra_clip {
            area = area.intersect(clip.bounding_box().inflate(1.0, 1.0));
        }
        let drawn = self
            .backing
            .paint(area, &self.image_cache, |offset, painter| {
                // The composition layer encloses the clip layers.
                // Destination-out acts on the tile's existing canvas pixels.
                painter.push_layer(
                    blend,
                    if clear { 1.0 } else { state.alpha },
                    offset,
                    &bounds,
                    None,
                    None,
                );
                for clip in &clips {
                    painter.render_ctx.set_fill_rule(clip.rule);
                    painter.push_clip_layer(offset, clip.path.as_ref());
                }
                if let Some(clip) = &extra_clip {
                    painter.render_ctx.set_fill_rule(Fill::NonZero);
                    painter.push_clip_layer(offset, clip);
                }
                if stroke {
                    painter.stroke(
                        &style,
                        offset * transform,
                        paint.as_ref(),
                        Some(transform.inverse() * paint_transform),
                        &shape,
                    );
                } else {
                    painter.fill(
                        fill,
                        offset * transform,
                        paint.as_ref(),
                        Some(paint_transform),
                        &shape,
                    );
                }
                for _ in 0..clips.len() + usize::from(extra_clip.is_some()) {
                    painter.pop_layer();
                }
                painter.pop_layer();
            });
        if drawn {
            self.invalidate();
        }
        Ok(())
    }

    fn draw_effect(
        &mut self,
        path: &BezPath,
        state: &DrawState,
        style: &Stroke,
        paint: &Paint,
        paint_transform: Affine,
        extra_clip: &Option<BezPath>,
        transform: Affine,
        stroke: bool,
        fill: Fill,
        blend: BlendMode,
        effect: &effects::Prepared,
    ) -> Result<()> {
        let mut area = path.bounding_box().inflate(1.0, 1.0);
        if stroke {
            let matrix = transform.as_coeffs();
            let radius = state.width * state.miter.max(2.0);
            area = area.inflate(
                radius * (matrix[0].abs() + matrix[2].abs()),
                radius * (matrix[1].abs() + matrix[3].abs()),
            );
        }
        if let Some(clip) = extra_clip {
            area = area.intersect(clip.bounding_box().inflate(1.0, 1.0));
        }
        let canvas = Rect::new(0.0, 0.0, self.width as f64, self.height as f64);
        if ![area.x0, area.y0, area.x1, area.y1]
            .iter()
            .all(|value| value.is_finite())
        {
            area = canvas;
        }
        let mut visible = canvas;
        for clip in &self.clips {
            visible = visible.intersect(clip.path.bounding_box().inflate(1.0, 1.0));
        }
        let margin = effects::margin(effect);
        area = area.intersect(visible.inflate(margin, margin));
        if area.x1 <= area.x0 || area.y1 <= area.y0 {
            return Ok(());
        }
        let raster = self.raster_layer(
            path,
            style,
            paint,
            paint_transform,
            extra_clip,
            transform,
            stroke,
            fill,
            area,
        )?;
        match effects::apply(raster, effect)? {
            effects::Outcome::Separate { shadow, shape } => {
                self.composite(&shadow, state, blend)?;
                self.composite(&shape, state, blend)?;
            }
            effects::Outcome::Filtered { layer } => {
                self.composite(&layer, state, blend)?;
            }
        }
        Ok(())
    }

    fn raster_layer(
        &mut self,
        path: &BezPath,
        style: &Stroke,
        paint: &Paint,
        paint_transform: Affine,
        extra_clip: &Option<BezPath>,
        transform: Affine,
        stroke: bool,
        fill: Fill,
        area: Rect,
    ) -> Result<effects::Layer> {
        let x0 = effects::bounded(area.x0.floor())?;
        let y0 = effects::bounded(area.y0.floor())?;
        let x1 = effects::bounded(area.x1.ceil())?;
        let y1 = effects::bounded(area.y1.ceil())?;
        if x1 <= x0 || y1 <= y0 {
            return Ok(effects::Layer::empty());
        }
        let width = (i64::from(x1) - i64::from(x0)) as usize;
        let height = (i64::from(y1) - i64::from(y0)) as usize;
        effects::check_extent(width, height)?;
        let mut painter = VelloCpuScenePainter::new(width as u16, height as u16)
            .with_image_cache(Arc::clone(&self.image_cache));
        let offset = Affine::translate((-(f64::from(x0)), -(f64::from(y0))));
        if let Some(clip) = extra_clip {
            painter.render_ctx.set_fill_rule(Fill::NonZero);
            painter.push_clip_layer(offset, clip);
        }
        let shape = transform.inverse() * path.clone();
        if stroke {
            painter.stroke(
                style,
                offset * transform,
                paint.as_ref(),
                Some(transform.inverse() * paint_transform),
                &shape,
            );
        } else {
            painter.fill(
                fill,
                offset * transform,
                paint.as_ref(),
                Some(paint_transform),
                &shape,
            );
        }
        if extra_clip.is_some() {
            painter.pop_layer();
        }
        let pixmap = painter.finish_shared();
        effects::Layer::from_premul(pixmap.as_ref(), x0, y0, width, height)
    }

    /// Premultiplied layer bytes are sampled 1:1 in device space. The tile
    /// translation is the path transform, so the brush translation is the
    /// layer origin and is not multiplied by the current matrix.
    fn composite(
        &mut self,
        layer: &effects::Layer,
        state: &DrawState,
        blend: BlendMode,
    ) -> Result<()> {
        if layer.is_empty() {
            return Ok(());
        }
        let bytes = Arc::new(layer.premul_bytes());
        let image = ImageData {
            data: Blob::new(Arc::clone(&bytes) as Arc<dyn AsRef<[u8]> + Send + Sync>),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::AlphaPremultiplied,
            width: layer.width as u32,
            height: layer.height as u32,
        };
        let id = image.data.id();
        let rect = Rect::new(
            f64::from(layer.x),
            f64::from(layer.y),
            f64::from(layer.x) + layer.width as f64,
            f64::from(layer.y) + layer.height as f64,
        );
        let mut area = rect;
        for clip in &self.clips {
            area = area.intersect(clip.path.bounding_box().inflate(1.0, 1.0));
        }
        let clips = self.clips.clone();
        let canvas_width = self.width as f64;
        let canvas_height = self.height as f64;
        let alpha = state.alpha;
        let paint_transform = Affine::translate((f64::from(layer.x), f64::from(layer.y)));
        let drawn = self
            .backing
            .paint(area, &self.image_cache, |offset, painter| {
                let bounds = Rect::new(0.0, 0.0, canvas_width, canvas_height);
                painter.push_layer(blend, alpha, offset, &bounds, None, None);
                for clip in &clips {
                    painter.render_ctx.set_fill_rule(clip.rule);
                    painter.push_clip_layer(offset, clip.path.as_ref());
                }
                painter.fill(
                    Fill::NonZero,
                    offset,
                    image_brush(image.clone(), false, Extend::Pad, Extend::Pad).as_ref(),
                    Some(paint_transform),
                    &rect,
                );
                for _ in 0..clips.len() {
                    painter.pop_layer();
                }
                painter.pop_layer();
            });
        lock(&self.image_cache).remove(id);
        if drawn {
            self.invalidate();
        }
        Ok(())
    }

    fn put(&mut self, command: &Command, inputs: &[Arc<Vec<u8>>]) -> Result<()> {
        numbers(&command.n, 6)?;
        let source = command
            .brush
            .as_ref()
            .and_then(|brush| brush.image.as_ref())
            .ok_or("putImageData requires pixels")?;
        let image = image_data(source, inputs)?;
        let n = &command.n;
        let x0 = n[2].min(n[2] + n[4]).max(0.0).ceil() as i64;
        let y0 = n[3].min(n[3] + n[5]).max(0.0).ceil() as i64;
        let x1 = n[2].max(n[2] + n[4]).min(source.width as f64).floor() as i64;
        let y1 = n[3].max(n[3] + n[5]).min(source.height as f64).floor() as i64;
        let dx = n[0].trunc() as i64;
        let dy = n[1].trunc() as i64;
        self.flush();
        self.invalidate();
        for y in y0.max(-dy)..y1.min(self.height as i64 - dy) {
            let left = x0.max(-dx);
            let right = x1.min(self.width as i64 - dx);
            if right <= left {
                continue;
            }
            let from = ((y * source.width as i64 + left) * 4) as usize;
            let count = ((right - left) * 4) as usize;
            self.backing.write_row(
                (left + dx) as u32,
                (y + dy) as u32,
                &image.data.data()[from..from + count],
            );
        }
        Ok(())
    }

    fn execute(&mut self, command: &Command, inputs: &[Arc<Vec<u8>>], fonts: &Fonts) -> Result<()> {
        numbers(&command.matrix, 6)?;
        if self
            .path
            .command(&command.op, &command.n, Affine::new(command.matrix))?
        {
            if self.path.path.elements().len() > PATH_LIMIT {
                return Err("canvas path limit reached in chuzz".to_owned());
            }
            return Ok(());
        }
        match command.op.as_str() {
            "save" => {
                if self.saves.len() >= STACK_LIMIT {
                    return Err("canvas save limit reached in chuzz".to_owned());
                }
                self.saves.push(self.clips.len());
                return Ok(());
            }
            "restore" => {
                if let Some(count) = self.saves.pop() {
                    self.clips.truncate(count);
                }
                return Ok(());
            }
            "clip" => {
                numbers(&command.n, 1)?;
                if self.clips.len() >= STACK_LIMIT {
                    return Err("canvas clip limit reached in chuzz".to_owned());
                }
                self.clips.push(Clip {
                    path: Arc::new(self.path.path.clone()),
                    rule: rule(command.n[0]),
                });
                return Ok(());
            }
            "putImageData" => return self.put(command, inputs),
            _ => {}
        }
        let state = command
            .state
            .as_ref()
            .ok_or("canvas drawing requires state")?;
        let brush = command
            .brush
            .as_ref()
            .ok_or("canvas drawing requires a brush")?;
        let matrix = Affine::new(state.matrix);
        let mut fill = Fill::NonZero;
        let mut stroke = false;
        let mut clear = false;
        let mut image_paint = None;
        let path = match command.op.as_str() {
            "fill" => {
                numbers(&command.n, 1)?;
                fill = rule(command.n[0]);
                self.path.path.clone()
            }
            "stroke" => {
                numbers(&command.n, 0)?;
                stroke = true;
                self.path.path.clone()
            }
            "fillRect" | "strokeRect" | "clearRect" => {
                numbers(&command.n, 4)?;
                stroke = command.op == "strokeRect";
                clear = command.op == "clearRect";
                matrix * paths::rectangle(&command.n)
            }
            "fillText" | "strokeText" => {
                numbers(&command.n, 3)?;
                stroke = command.op == "strokeText";
                let shaped = text::shape(&command.text, &state.font, &fonts.0)?;
                let scale = if command.n[2] > 0.0 && shaped.width > command.n[2] {
                    command.n[2] / shaped.width
                } else {
                    1.0
                };
                let offset = shaped.offset(state)?;
                matrix
                    * Affine::translate((command.n[0], command.n[1]))
                    * Affine::scale_non_uniform(scale, 1.0)
                    * Affine::translate(offset)
                    * shaped.path
            }
            "drawImage" => {
                numbers(&command.n, 8)?;
                let source = brush.image.as_ref().ok_or("drawImage requires pixels")?;
                let n = &command.n;
                if n[2] <= 0.0 || n[3] <= 0.0 || n[6] <= 0.0 || n[7] <= 0.0 {
                    return Ok(());
                }
                let x0 = n[0].max(0.0);
                let y0 = n[1].max(0.0);
                let x1 = (n[0] + n[2]).min(source.width as f64);
                let y1 = (n[1] + n[3]).min(source.height as f64);
                if x1 <= x0 || y1 <= y0 {
                    return Ok(());
                }
                let image_matrix = matrix
                    * Affine::translate((n[4], n[5]))
                    * Affine::scale_non_uniform(n[6] / n[2], n[7] / n[3])
                    * Affine::translate((-n[0], -n[1]));
                // Sample only the source rectangle: a smoothed, scaled sub-rect
                // must clamp at its own edges, not filter in the pixels around
                // it. Nearest-neighbour or unscaled copies never sample outside
                // the rectangle, so they keep the whole source and its cached
                // conversion.
                let scaled = (n[6] - n[2]).abs() > 1.0e-9 || (n[7] - n[3]).abs() > 1.0e-9;
                let (crop, origin) = if state.smoothing && scaled {
                    crop_source(source, inputs, x0, y0, x1, y1)?
                } else {
                    (source.clone(), (0.0, 0.0))
                };
                let actual = Brush {
                    kind: "pattern".to_owned(),
                    color: String::new(),
                    n: Vec::new(),
                    matrix: (image_matrix * Affine::translate(origin)).as_coeffs(),
                    stops: Vec::new(),
                    image: Some(crop),
                    repetition: "no-repeat".to_owned(),
                };
                image_paint = Some(actual);
                image_matrix * Rect::new(x0, y0, x1, y1).to_path(0.1)
            }
            member => return Err(missing(member)),
        };
        let clear_brush = Brush {
            kind: "color".to_owned(),
            color: "#ffffff".to_owned(),
            n: Vec::new(),
            matrix: identity(),
            stops: Vec::new(),
            image: None,
            repetition: String::new(),
        };
        self.draw(
            &path,
            state,
            if clear {
                &clear_brush
            } else {
                image_paint.as_ref().unwrap_or(brush)
            },
            inputs,
            stroke,
            fill,
            clear,
        )
    }

    fn read(&mut self, n: &[f64]) -> Result<Vec<u8>> {
        numbers(n, 4)?;
        let width = n[2] as u32;
        let height = n[3] as u32;
        let mut output = vec![0; byte_count(width, height)?];
        self.flush();
        let x = n[0].trunc() as i64;
        let y = n[1].trunc() as i64;
        for row in 0..height as i64 {
            let source_y = y + row;
            if source_y < 0 || source_y >= self.height as i64 {
                continue;
            }
            let left = 0.max(-x);
            let right = (width as i64).min(self.width as i64 - x);
            if right <= left {
                continue;
            }
            let to = ((row * width as i64 + left) * 4) as usize;
            let count = ((right - left) * 4) as usize;
            self.backing.read_row(
                (x + left) as u32,
                source_y as u32,
                &mut output[to..to + count],
            );
        }
        Ok(output)
    }
}

/// `__chuzzEncodePng(width, height, rgba)`: the PNG bytes behind
/// `HTMLCanvasElement.toBlob` and `toDataURL`, from straight RGBA pixels.
fn encode_png(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let failure = |message: &str| JsNativeError::typ().with_message(message.to_owned());
    let width = args.first().cloned().unwrap_or_default().to_u32(context)?;
    let height = args.get(1).cloned().unwrap_or_default().to_u32(context)?;
    let object = args
        .get(2)
        .and_then(JsValue::as_object)
        .ok_or_else(|| failure("expected canvas pixels"))?;
    let buffer = JsArrayBuffer::from_object(object.clone())?;
    let pixels = buffer
        .data()
        .ok_or_else(|| failure("detached canvas pixels"))?;
    if width == 0
        || height == 0
        || pixels.len() != byte_count(width, height).map_err(|e| failure(&e))?
    {
        return Err(failure("canvas pixels do not match the canvas size").into());
    }
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| failure(&e.to_string()))?;
        writer
            .write_image_data(&pixels)
            .map_err(|e| failure(&e.to_string()))?;
        writer.finish().map_err(|e| failure(&e.to_string()))?;
    }
    Ok(JsUint8Array::from_iter(png, context)?.into())
}

fn native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = context
        .get_data::<Host>()
        .cloned()
        .ok_or_else(|| JsNativeError::typ().with_message("canvas host is unavailable"))?;
    let json = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    if json.len() > 8 * 1024 * 1024 {
        return Err(JsNativeError::typ()
            .with_message("canvas descriptor limit reached in chuzz")
            .into());
    }
    let mut request: Request = serde_json::from_str(&json)
        .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
    if request.action == "color" {
        return Ok(JsValue::from(color(&request.value).is_ok()));
    }
    if request.action == "font" {
        let font = text::parse_font(&request.value, request.font_base, request.root_base)
            .map_err(|error| JsNativeError::typ().with_message(error))?;
        return Ok(JsString::from(font).into());
    }
    if request.action == "stats" {
        let cache = lock(&host.image_cache);
        let value = serde_json::json!({
            "transferredBytes": *lock(&host.transferred),
            "cacheBytes": cache.retained_bytes(),
            "cacheLimit": cache.byte_limit(),
            "cacheEntries": cache.len(),
            "cacheHits": cache.hits(),
            "cacheMisses": cache.misses(),
        });
        return Ok(JsString::from(value.to_string()).into());
    }
    if request.action == "release" {
        lock(&host.canvases).remove(&request.id);
        if let Some(image) = lock(&host.sources).remove(&request.id) {
            lock(&host.image_cache).remove(image.data.id());
        }
        host.bridge.state().images.paint.remove(&request.id);
        return Ok(JsValue::undefined());
    }
    if request.action == "source" {
        if request.id == 0 || request.id > 9_007_199_254_740_991 {
            return Err(JsNativeError::typ()
                .with_message("invalid native canvas source handle")
                .into());
        }
        let source = request
            .source
            .take()
            .ok_or_else(|| JsNativeError::typ().with_message("missing native canvas source"))?;
        let image = match source {
            super::external::Source::Element { handle } => {
                blitz_script::raster_image_for_attribute(
                    context,
                    "data-chuzz-gpu-source",
                    &handle.to_string(),
                )
                .map(Arc::new)
                .ok_or_else(|| JsNativeError::typ().with_message("image is not decoded"))?
            }
            source => super::readback::image(&host.bridge, source, request.serial, context)?
                .map_err(|error| JsNativeError::typ().with_message(error.message))?,
        };
        let size = byte_count(image.width, image.height)
            .map_err(|error| JsNativeError::typ().with_message(error))?;
        if size != image.data.len() {
            return Err(JsNativeError::typ()
                .with_message("invalid native canvas source pixels")
                .into());
        }
        {
            let mut sources = lock(&host.sources);
            let previous = sources.get(&request.id).map_or(0, |image| image.data.len());
            let allocated = sources
                .values()
                .map(|image| image.data.len())
                .sum::<usize>();
            if (!sources.contains_key(&request.id) && sources.len() >= 8192)
                || allocated.saturating_sub(previous).saturating_add(size) > DOCUMENT_LIMIT / 3
            {
                return Err(JsNativeError::typ()
                    .with_message("native canvas source allocation limit reached")
                    .into());
            }
            if let Some(previous) = sources.insert(request.id, raster_image(&image))
                && previous.data.id() != image.data.id()
            {
                lock(&host.image_cache).remove(previous.data.id());
            }
        }
        return Ok(JsArray::from_iter(
            [JsValue::from(image.width), JsValue::from(image.height)],
            context,
        )
        .into());
    }
    let array = args
        .get(1)
        .and_then(JsValue::as_object)
        .ok_or_else(|| JsNativeError::typ().with_message("expected canvas byte buffers"))?;
    let array = JsArray::from_object(array)?;
    let count = array.length(context)?;
    if count > 1024 || request.commands.len() > COMMAND_LIMIT {
        return Err(JsNativeError::typ()
            .with_message("canvas command limit reached in chuzz")
            .into());
    }
    let mut inputs = Vec::with_capacity(count as usize);
    let mut input_size = 0usize;
    for index in 0..count {
        let value = array.get(index, context)?;
        let object = value
            .as_object()
            .ok_or_else(|| JsNativeError::typ().with_message("expected canvas ArrayBuffer"))?;
        let buffer = JsArrayBuffer::from_object(object)?;
        let bytes = buffer
            .data()
            .ok_or_else(|| JsNativeError::typ().with_message("detached canvas image"))?;
        input_size = input_size.saturating_add(bytes.len());
        if input_size > IMAGE_LIMIT {
            return Err(JsNativeError::typ()
                .with_message("canvas transfer limit reached in chuzz")
                .into());
        }
        inputs.push(Arc::new(bytes.to_vec()));
    }
    {
        let mut transferred = lock(&host.transferred);
        *transferred = transferred.saturating_add(input_size as u64);
    }
    for command in &mut request.commands {
        let Some(image) = command
            .brush
            .as_mut()
            .and_then(|brush| brush.image.as_mut())
        else {
            continue;
        };
        let references = [image.video, image.canvas, image.bitmap, image.native]
            .iter()
            .filter(|handle| handle.is_some())
            .count();
        if references > 1 {
            return Err(JsNativeError::typ()
                .with_message("ambiguous native canvas image source")
                .into());
        }
        if let Some(handle) = image.video.take() {
            let frame = crate::video::take_frame(context, handle)
                .map_err(|error| JsNativeError::typ().with_message(error))?;
            image.resolved = Some(raster_image(&frame.image));
        } else if let Some(handle) = image.bitmap {
            let bitmap = host
                .bridge
                .state()
                .images
                .bitmaps
                .get(&handle)
                .cloned()
                .ok_or_else(|| {
                    JsNativeError::typ().with_message("ImageBitmap is closed or invalid")
                })?;
            image.resolved = Some(raster_image(&bitmap));
        } else if let Some(handle) = image.native {
            image.resolved = Some(lock(&host.sources).get(&handle).cloned().ok_or_else(|| {
                JsNativeError::typ().with_message("native canvas source was released")
            })?);
        }
        if image.resolved.is_some() {
            image_data(image, &inputs).map_err(|error| JsNativeError::typ().with_message(error))?;
        }
    }
    let result = (|| -> Result<JsValue> {
        let size = byte_count(request.width, request.height)?;
        let mut canvases = lock(&host.canvases);
        let previous = canvases
            .get(&request.id)
            .map_or(0, |canvas| canvas.backing.byte_len());
        let allocated = canvases
            .values()
            .map(|canvas| canvas.backing.byte_len())
            .sum::<usize>();
        if request.id == 0
            || request.id > 9_007_199_254_740_991
            || (!canvases.contains_key(&request.id) && canvases.len() >= 8192)
            || allocated.saturating_sub(previous).saturating_add(size) > DOCUMENT_LIMIT / 3
        {
            return Err("canvas document allocation limit reached in chuzz".to_owned());
        }
        if request.action == "reset"
            || canvases.get(&request.id).is_none_or(|canvas| {
                canvas.width != request.width || canvas.height != request.height
            })
        {
            canvases.insert(
                request.id,
                Canvas::new(request.width, request.height, Arc::clone(&host.image_cache))?,
            );
        }
        for command in &mut request.commands {
            if let Some(image) = command
                .brush
                .as_mut()
                .and_then(|brush| brush.image.as_mut())
                && let Some(source) = image.canvas
            {
                // Resolve at this command, not at the beginning of the batch.
                // For self-draw this flushes the preceding commands and pins
                // the old allocation while execute draws the new contents.
                image.resolved = Some(
                    canvases
                        .get_mut(&source)
                        .ok_or("native source canvas was released")?
                        .source_image(),
                );
                image_data(image, &inputs)?;
            }
            canvases
                .get_mut(&request.id)
                .ok_or("invalid canvas")?
                .execute(command, &inputs, &host.fonts)?;
        }
        let canvas = canvases.get_mut(&request.id).ok_or("invalid canvas")?;
        match request.action.as_str() {
            "reset" | "submit" => Ok(JsValue::undefined()),
            "snapshot" => {
                let pixels = canvas.backing.snapshot();
                JsUint8ClampedArray::from_iter(pixels.iter().copied(), context)
                    .map(Into::into)
                    .map_err(|error| error.to_string())
            }
            "read" => {
                let bytes = canvas.read(&request.n)?;
                JsUint8ClampedArray::from_iter(bytes, context)
                    .map(Into::into)
                    .map_err(|error| error.to_string())
            }
            "paint" => {
                let pixels = canvas.backing.snapshot();
                let image = Arc::new(RasterImageData::new(canvas.width, canvas.height, pixels));
                host.bridge.state().images.paint.insert(request.id, image);
                Ok(JsValue::undefined())
            }
            "measure" => {
                let state = request.state.as_ref().ok_or("measureText requires state")?;
                let shaped = text::shape(&request.text, &state.font, &host.fonts.0)?;
                let metrics = shaped.metrics(state)?;
                serde_json::to_string(&metrics)
                    .map(|value| JsString::from(value).into())
                    .map_err(|error| error.to_string())
            }
            member => Err(missing(member)),
        }
    })();
    result.map_err(|error| JsNativeError::typ().with_message(error).into())
}

pub(super) fn install(document: &mut blitz_script::ScriptDocument, bridge: Bridge) {
    document.with_js_context(|context| {
        let fonts = context
            .get_data::<Fonts>()
            .cloned()
            .unwrap_or_else(|| Fonts(Arc::new(Mutex::new(default_fonts()))));
        let image_cache = Arc::clone(&bridge.state().images.image_cache);
        context.insert_data(Host {
            canvases: Arc::new(Mutex::new(HashMap::new())),
            sources: Arc::new(Mutex::new(HashMap::new())),
            image_cache,
            transferred: Arc::new(Mutex::new(0)),
            fonts,
            bridge,
        });
        context
            .register_global_builtin_callable(
                JsString::from("__chuzzCanvas2D"),
                2,
                NativeFunction::from_fn_ptr(native),
            )
            .expect("canvas command bridge");
        context
            .register_global_builtin_callable(
                JsString::from("__chuzzEncodePng"),
                3,
                NativeFunction::from_fn_ptr(encode_png),
            )
            .expect("canvas PNG encoder");
    });
}
