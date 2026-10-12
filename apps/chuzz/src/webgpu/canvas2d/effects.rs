//! Shadow and CSS filter rasters for one drawing operation.
//!
//! Pixels are premultiplied RGBA in 0..255. A canvas shadow uses
//! sigma = shadowBlur / 2. CSS `blur()` and `drop-shadow()` use the length
//! itself as sigma. Offsets are device pixels.

use peniko::Color;
use serde::Deserialize;

use super::{DrawState, IMAGE_LIMIT, Result, color as parse_colour};

const MAX_RADIUS: usize = 512;

#[derive(Clone, Deserialize)]
#[serde(tag = "fn", rename_all = "kebab-case")]
pub(super) enum FilterOp {
    Blur {
        px: f64,
    },
    DropShadow {
        x: f64,
        y: f64,
        blur: f64,
        color: String,
    },
    Brightness {
        amount: f64,
    },
    Contrast {
        amount: f64,
    },
    Grayscale {
        amount: f64,
    },
    Invert {
        amount: f64,
    },
    Opacity {
        amount: f64,
    },
    Saturate {
        amount: f64,
    },
    Sepia {
        amount: f64,
    },
    HueRotate {
        deg: f64,
    },
}

#[derive(Clone, Copy)]
pub(super) struct Shadow {
    pub offset_x: f64,
    pub offset_y: f64,
    pub sigma: f64,
    pub color: Color,
}

pub(super) struct Prepared {
    pub shadow: Option<Shadow>,
    pub filter: Vec<FilterOp>,
}

pub(super) enum Outcome {
    Separate { shadow: Option<Layer>, shape: Layer },
}

#[derive(Clone)]
pub(super) struct Layer {
    pub x: i32,
    pub y: i32,
    pub width: usize,
    pub height: usize,
    pixels: Vec<f32>,
}

pub(super) fn prepare(state: &DrawState) -> Result<Option<Prepared>> {
    if !state.shadow_blur.is_finite()
        || !(0.0..=1.0e6).contains(&state.shadow_blur)
        || !state.shadow_offset_x.is_finite()
        || state.shadow_offset_x.abs() > 1.0e6
        || !state.shadow_offset_y.is_finite()
        || state.shadow_offset_y.abs() > 1.0e6
    {
        return Err("invalid canvas shadow".to_owned());
    }
    let shadow_color = parse_colour(&state.shadow_color)?;
    if state.filter.len() > 64 {
        return Err("canvas filter extent reached in chuzz".to_owned());
    }
    for op in &state.filter {
        validate(op)?;
    }
    let active = shadow_color.components[3] > 0.0
        && (state.shadow_blur > 0.0
            || state.shadow_offset_x != 0.0
            || state.shadow_offset_y != 0.0);
    if !active && state.filter.is_empty() {
        return Ok(None);
    }
    let shadow = if active {
        Some(Shadow {
            offset_x: state.shadow_offset_x,
            offset_y: state.shadow_offset_y,
            sigma: state.shadow_blur * 0.5,
            color: shadow_color,
        })
    } else {
        None
    };
    Ok(Some(Prepared {
        shadow,
        filter: state.filter.clone(),
    }))
}

pub(super) fn margin(effect: &Prepared) -> f64 {
    let mut margin = 0.0;
    if let Some(shadow) = &effect.shadow {
        margin += spread(shadow.offset_x, shadow.offset_y, shadow.sigma);
    }
    for op in &effect.filter {
        margin += match op {
            FilterOp::Blur { px } => spread(0.0, 0.0, *px),
            FilterOp::DropShadow { x, y, blur, .. } => spread(*x, *y, *blur),
            _ => 0.0,
        };
    }
    margin
}

pub(super) fn check_extent(width: usize, height: usize) -> Result<()> {
    if width > u16::MAX as usize
        || height > u16::MAX as usize
        || width.saturating_mul(height).saturating_mul(4) > IMAGE_LIMIT
    {
        return Err("canvas filter extent reached in chuzz".to_owned());
    }
    Ok(())
}

pub(super) fn bounded(value: f64) -> Result<i32> {
    if !value.is_finite() || value < i32::MIN as f64 || value > i32::MAX as f64 {
        return Err("canvas filter extent reached in chuzz".to_owned());
    }
    Ok(value as i32)
}

pub(super) fn apply(mut shape: Layer, effect: &Prepared) -> Result<Outcome> {
    for op in &effect.filter {
        shape = apply_op(shape, op)?;
    }
    let shadow = effect
        .shadow
        .map(|shadow| make_shadow(&shape, &shadow))
        .transpose()?;
    Ok(Outcome::Separate { shadow, shape })
}

impl Layer {
    pub(super) fn empty() -> Self {
        Self {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            pixels: Vec::new(),
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    pub(super) fn from_premul(
        bytes: &[u8],
        x: i32,
        y: i32,
        width: usize,
        height: usize,
    ) -> Result<Self> {
        let expected = width
            .checked_mul(height)
            .and_then(|count| count.checked_mul(4))
            .ok_or_else(|| "canvas filter extent reached in chuzz".to_owned())?;
        if bytes.len() != expected {
            return Err("canvas filter extent reached in chuzz".to_owned());
        }
        let mut pixels = Vec::with_capacity(expected);
        for chunk in bytes.chunks_exact(4) {
            pixels.push(f32::from(chunk[0]));
            pixels.push(f32::from(chunk[1]));
            pixels.push(f32::from(chunk[2]));
            pixels.push(f32::from(chunk[3]));
        }
        Ok(Self {
            x,
            y,
            width,
            height,
            pixels,
        })
    }

    pub(super) fn premul_bytes(&self) -> Vec<u8> {
        self.pixels.iter().copied().map(byte).collect()
    }

    pub(super) fn scale_alpha(&mut self, alpha: f32) {
        for value in &mut self.pixels {
            *value *= alpha;
        }
    }

    /// Expand to transparent canvas coverage for unbounded composite operators.
    pub(super) fn padded_to(&self, x: i32, y: i32, width: usize, height: usize) -> Result<Self> {
        check_extent(width, height)?;
        if width == 0 || height == 0 {
            return Ok(Self::empty());
        }
        let mut pixels = vec![0.0; width * height * 4];
        for row in 0..height {
            for column in 0..width {
                let device_x = x.saturating_add(column as i32);
                let device_y = y.saturating_add(row as i32);
                let sample = self.sample(device_x, device_y);
                let index = (row * width + column) * 4;
                pixels[index..index + 4].copy_from_slice(&sample);
            }
        }
        Ok(Self {
            x,
            y,
            width,
            height,
            pixels,
        })
    }

    fn sample(&self, x: i32, y: i32) -> [f32; 4] {
        let local_x = i64::from(x) - i64::from(self.x);
        let local_y = i64::from(y) - i64::from(self.y);
        if local_x < 0
            || local_y < 0
            || local_x >= self.width as i64
            || local_y >= self.height as i64
        {
            return [0.0; 4];
        }
        let index = ((local_y as usize) * self.width + (local_x as usize)) * 4;
        [
            self.pixels[index],
            self.pixels[index + 1],
            self.pixels[index + 2],
            self.pixels[index + 3],
        ]
    }
}

fn validate(op: &FilterOp) -> Result<()> {
    match op {
        FilterOp::Blur { px } => finite_unit(*px, 1.0e6),
        FilterOp::DropShadow { x, y, blur, color } => {
            parse_colour(color)?;
            if x.is_finite() && x.abs() <= 1.0e6 && y.is_finite() && y.abs() <= 1.0e6 {
                finite_unit(*blur, 1.0e6)?;
                Ok(())
            } else {
                Err("invalid canvas filter".to_owned())
            }
        }
        FilterOp::Brightness { amount }
        | FilterOp::Contrast { amount }
        | FilterOp::Saturate { amount } => finite_unit(*amount, 1.0e6),
        FilterOp::Grayscale { amount }
        | FilterOp::Invert { amount }
        | FilterOp::Opacity { amount }
        | FilterOp::Sepia { amount } => finite_unit(*amount, 1.0),
        FilterOp::HueRotate { deg } => {
            if deg.is_finite() && deg.abs() <= 1.0e8 {
                Ok(())
            } else {
                Err("invalid canvas filter".to_owned())
            }
        }
    }
}

fn finite_unit(value: f64, max: f64) -> Result<()> {
    if value.is_finite() && (0.0..=max).contains(&value) {
        Ok(())
    } else {
        Err("invalid canvas filter".to_owned())
    }
}

fn spread(x: f64, y: f64, sigma: f64) -> f64 {
    x.abs().min(1.0e6) + y.abs().min(1.0e6) + radius_for(sigma) as f64 + 2.0
}

fn radius_for(sigma: f64) -> usize {
    if !(sigma > 0.0) || !sigma.is_finite() {
        return 0;
    }
    let radius = (sigma * 3.0).ceil();
    if !radius.is_finite() || radius > MAX_RADIUS as f64 {
        return MAX_RADIUS;
    }
    let radius = radius as usize;
    if radius == 0 { 1 } else { radius }
}

fn kernel(sigma: f64) -> Vec<f32> {
    let radius = radius_for(sigma);
    if radius == 0 {
        return vec![1.0];
    }
    let mut weights = vec![0.0f64; radius * 2 + 1];
    let mut sum = 0.0;
    for (index, weight) in weights.iter_mut().enumerate() {
        let distance = index as f64 - radius as f64;
        let value = (-0.5 * (distance / sigma) * (distance / sigma)).exp();
        *weight = value;
        sum += value;
    }
    if !(sum > 0.0) || !sum.is_finite() {
        return vec![1.0];
    }
    weights
        .into_iter()
        .map(|weight| (weight / sum) as f32)
        .collect()
}

fn byte(value: f32) -> u8 {
    if !value.is_finite() {
        return 0;
    }
    value.clamp(0.0, 255.0).round() as u8
}

fn blur_plane(source: &[f32], width: usize, height: usize, weights: &[f32]) -> Vec<f32> {
    let radius = weights.len() / 2;
    let mut horizontal = vec![0.0f32; width * height];
    for y in 0..height {
        let row = y * width;
        for x in 0..width {
            let mut sum = 0.0f32;
            for (offset, weight) in weights.iter().enumerate() {
                let position = x as isize + offset as isize - radius as isize;
                if position >= 0 && (position as usize) < width {
                    sum += source[row + position as usize] * *weight;
                }
            }
            horizontal[row + x] = sum;
        }
    }
    let mut vertical = vec![0.0f32; width * height];
    for y in 0..height {
        for x in 0..width {
            let mut sum = 0.0f32;
            for (offset, weight) in weights.iter().enumerate() {
                let position = y as isize + offset as isize - radius as isize;
                if position >= 0 && (position as usize) < height {
                    sum += horizontal[position as usize * width + x] * *weight;
                }
            }
            vertical[y * width + x] = sum;
        }
    }
    vertical
}

fn shifted_origin(origin: i32, radius: usize) -> Result<i32> {
    origin
        .checked_sub(i32::try_from(radius).unwrap_or(i32::MAX))
        .ok_or_else(|| "canvas filter extent reached in chuzz".to_owned())
}

fn make_shadow(source: &Layer, shadow: &Shadow) -> Result<Layer> {
    if source.is_empty() {
        return Ok(Layer::empty());
    }
    let radius = radius_for(shadow.sigma);
    let blurred_width = source.width + radius * 2;
    let blurred_height = source.height + radius * 2;
    check_extent(blurred_width, blurred_height)?;
    let mut alpha = vec![0.0f32; blurred_width * blurred_height];
    for y in 0..source.height {
        for x in 0..source.width {
            alpha[(y + radius) * blurred_width + (x + radius)] =
                source.pixels[(y * source.width + x) * 4 + 3];
        }
    }
    if radius > 0 {
        alpha = blur_plane(&alpha, blurred_width, blurred_height, &kernel(shadow.sigma));
    }
    let blurred_x = shifted_origin(source.x, radius)?;
    let blurred_y = shifted_origin(source.y, radius)?;
    let x0 = bounded((f64::from(blurred_x) + shadow.offset_x).floor())?;
    let y0 = bounded((f64::from(blurred_y) + shadow.offset_y).floor())?;
    let x1 = bounded((f64::from(blurred_x) + blurred_width as f64 + shadow.offset_x).ceil())?;
    let y1 = bounded((f64::from(blurred_y) + blurred_height as f64 + shadow.offset_y).ceil())?;
    if x1 <= x0 || y1 <= y0 {
        return Ok(Layer::empty());
    }
    let width = (i64::from(x1) - i64::from(x0)) as usize;
    let height = (i64::from(y1) - i64::from(y0)) as usize;
    check_extent(width, height)?;
    let premul = shadow.color.premultiply().components;
    let scale = [
        premul[0].clamp(0.0, 1.0),
        premul[1].clamp(0.0, 1.0),
        premul[2].clamp(0.0, 1.0),
        premul[3].clamp(0.0, 1.0),
    ];
    let mut pixels = vec![0.0f32; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let device_x = x0 + x as i32;
            let device_y = y0 + y as i32;
            let sample_x = f64::from(device_x) - shadow.offset_x - f64::from(blurred_x);
            let sample_y = f64::from(device_y) - shadow.offset_y - f64::from(blurred_y);
            let coverage = bilinear(&alpha, blurred_width, blurred_height, sample_x, sample_y)
                .clamp(0.0, 255.0);
            if coverage <= 0.0 {
                continue;
            }
            let index = (y * width + x) * 4;
            pixels[index] = scale[0] * coverage;
            pixels[index + 1] = scale[1] * coverage;
            pixels[index + 2] = scale[2] * coverage;
            pixels[index + 3] = scale[3] * coverage;
        }
    }
    Ok(Layer {
        x: x0,
        y: y0,
        width,
        height,
        pixels,
    })
}

fn bilinear(plane: &[f32], width: usize, height: usize, x: f64, y: f64) -> f32 {
    if !x.is_finite() || !y.is_finite() {
        return 0.0;
    }
    let x0 = x.floor();
    let y0 = y.floor();
    let fx = (x - x0) as f32;
    let fy = (y - y0) as f32;
    let ix = x0 as i32;
    let iy = y0 as i32;
    let a00 = plane_at(plane, width, height, ix, iy);
    let a10 = plane_at(plane, width, height, ix.saturating_add(1), iy);
    let a01 = plane_at(plane, width, height, ix, iy.saturating_add(1));
    let a11 = plane_at(
        plane,
        width,
        height,
        ix.saturating_add(1),
        iy.saturating_add(1),
    );
    a00 * (1.0 - fx) * (1.0 - fy) + a10 * fx * (1.0 - fy) + a01 * (1.0 - fx) * fy + a11 * fx * fy
}

fn plane_at(plane: &[f32], width: usize, height: usize, x: i32, y: i32) -> f32 {
    if x < 0 || y < 0 || x as usize >= width || y as usize >= height {
        0.0
    } else {
        plane[y as usize * width + x as usize]
    }
}

fn source_over(under: &Layer, over: &Layer) -> Result<Layer> {
    if under.is_empty() {
        return Ok(over.clone());
    }
    if over.is_empty() {
        return Ok(under.clone());
    }
    let x0 = i64::from(under.x).min(i64::from(over.x));
    let y0 = i64::from(under.y).min(i64::from(over.y));
    let x1 = (i64::from(under.x) + under.width as i64).max(i64::from(over.x) + over.width as i64);
    let y1 = (i64::from(under.y) + under.height as i64).max(i64::from(over.y) + over.height as i64);
    let width = (x1 - x0) as usize;
    let height = (y1 - y0) as usize;
    check_extent(width, height)?;
    let origin_x = bounded(x0 as f64)?;
    let origin_y = bounded(y0 as f64)?;
    let mut pixels = vec![0.0f32; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let device_x = origin_x + x as i32;
            let device_y = origin_y + y as i32;
            let src = over.sample(device_x, device_y);
            let dst = under.sample(device_x, device_y);
            let inverse = 1.0 - (src[3] / 255.0).clamp(0.0, 1.0);
            let index = (y * width + x) * 4;
            for channel in 0..4 {
                pixels[index + channel] = src[channel] + dst[channel] * inverse;
            }
        }
    }
    Ok(Layer {
        x: origin_x,
        y: origin_y,
        width,
        height,
        pixels,
    })
}

fn apply_op(layer: Layer, op: &FilterOp) -> Result<Layer> {
    match op {
        // CSS lengths are the Gaussian standard deviation.
        FilterOp::Blur { px } => blur_layer(layer, *px),
        FilterOp::DropShadow { x, y, blur, color } => {
            let shadow = Shadow {
                offset_x: *x,
                offset_y: *y,
                sigma: *blur,
                color: parse_colour(color)?,
            };
            let under = make_shadow(&layer, &shadow)?;
            source_over(&under, &layer)
        }
        FilterOp::Brightness { amount } => Ok(map_colors(layer, brightness(*amount))),
        FilterOp::Contrast { amount } => Ok(map_colors(layer, contrast(*amount))),
        FilterOp::Grayscale { amount } => Ok(map_colors(layer, grayscale(*amount))),
        FilterOp::Invert { amount } => Ok(map_colors(layer, invert(*amount))),
        FilterOp::Opacity { amount } => Ok(map_colors(layer, opacity(*amount))),
        FilterOp::Saturate { amount } => Ok(map_colors(layer, saturate(*amount))),
        FilterOp::Sepia { amount } => Ok(map_colors(layer, sepia(*amount))),
        FilterOp::HueRotate { deg } => Ok(map_colors(layer, hue_rotate(*deg))),
    }
}

fn blur_layer(layer: Layer, sigma: f64) -> Result<Layer> {
    let radius = radius_for(sigma);
    if radius == 0 || layer.is_empty() {
        return Ok(layer);
    }
    let width = layer.width + radius * 2;
    let height = layer.height + radius * 2;
    check_extent(width, height)?;
    let weights = kernel(sigma);
    let mut planes = vec![vec![0.0f32; width * height]; 4];
    for y in 0..layer.height {
        for x in 0..layer.width {
            let from = (y * layer.width + x) * 4;
            let to = (y + radius) * width + (x + radius);
            for channel in 0..4 {
                planes[channel][to] = layer.pixels[from + channel];
            }
        }
    }
    for plane in &mut planes {
        *plane = blur_plane(plane, width, height, &weights);
    }
    let mut pixels = vec![0.0f32; width * height * 4];
    for index in 0..width * height {
        for channel in 0..4 {
            pixels[index * 4 + channel] = planes[channel][index];
        }
    }
    Ok(Layer {
        x: shifted_origin(layer.x, radius)?,
        y: shifted_origin(layer.y, radius)?,
        width,
        height,
        pixels,
    })
}

fn map_colors(mut layer: Layer, matrix: [f32; 20]) -> Layer {
    for pixel in layer.pixels.chunks_exact_mut(4) {
        let alpha = pixel[3];
        if alpha <= 0.0 {
            pixel[0] = 0.0;
            pixel[1] = 0.0;
            pixel[2] = 0.0;
            pixel[3] = 0.0;
            continue;
        }
        let inverse = 1.0 / alpha;
        let red = (pixel[0] * inverse).clamp(0.0, 1.0);
        let green = (pixel[1] * inverse).clamp(0.0, 1.0);
        let blue = (pixel[2] * inverse).clamp(0.0, 1.0);
        let straight_alpha = (alpha / 255.0).clamp(0.0, 1.0);
        let next_red = (matrix[0] * red
            + matrix[1] * green
            + matrix[2] * blue
            + matrix[3] * straight_alpha
            + matrix[4])
            .clamp(0.0, 1.0);
        let next_green = (matrix[5] * red
            + matrix[6] * green
            + matrix[7] * blue
            + matrix[8] * straight_alpha
            + matrix[9])
            .clamp(0.0, 1.0);
        let next_blue = (matrix[10] * red
            + matrix[11] * green
            + matrix[12] * blue
            + matrix[13] * straight_alpha
            + matrix[14])
            .clamp(0.0, 1.0);
        let next_alpha = (matrix[15] * red
            + matrix[16] * green
            + matrix[17] * blue
            + matrix[18] * straight_alpha
            + matrix[19])
            .clamp(0.0, 1.0);
        pixel[0] = next_red * next_alpha * 255.0;
        pixel[1] = next_green * next_alpha * 255.0;
        pixel[2] = next_blue * next_alpha * 255.0;
        pixel[3] = next_alpha * 255.0;
    }
    layer
}

fn brightness(amount: f64) -> [f32; 20] {
    let amount = amount as f32;
    [
        amount, 0.0, 0.0, 0.0, 0.0, 0.0, amount, 0.0, 0.0, 0.0, 0.0, 0.0, amount, 0.0, 0.0, 0.0,
        0.0, 0.0, 1.0, 0.0,
    ]
}

fn contrast(amount: f64) -> [f32; 20] {
    let amount = amount as f32;
    let intercept = 0.5 * (1.0 - amount);
    [
        amount, 0.0, 0.0, 0.0, intercept, 0.0, amount, 0.0, 0.0, intercept, 0.0, 0.0, amount, 0.0,
        intercept, 0.0, 0.0, 0.0, 1.0, 0.0,
    ]
}

fn invert(amount: f64) -> [f32; 20] {
    let amount = amount as f32;
    let slope = 1.0 - 2.0 * amount;
    [
        slope, 0.0, 0.0, 0.0, amount, 0.0, slope, 0.0, 0.0, amount, 0.0, 0.0, slope, 0.0, amount,
        0.0, 0.0, 0.0, 1.0, 0.0,
    ]
}

fn opacity(amount: f64) -> [f32; 20] {
    let amount = amount as f32;
    [
        1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0,
        amount, 0.0,
    ]
}

fn grayscale(amount: f64) -> [f32; 20] {
    let amount = amount as f32;
    let keep = 1.0 - amount;
    let red = 0.2126 * amount;
    let green = 0.7152 * amount;
    let blue = 0.0722 * amount;
    [
        red + keep,
        green,
        blue,
        0.0,
        0.0,
        red,
        green + keep,
        blue,
        0.0,
        0.0,
        red,
        green,
        blue + keep,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
        0.0,
    ]
}

fn sepia(amount: f64) -> [f32; 20] {
    let amount = amount as f32;
    let mix = |sepia: f32, identity: f32| sepia * amount + identity * (1.0 - amount);
    [
        mix(0.393, 1.0),
        mix(0.769, 0.0),
        mix(0.189, 0.0),
        0.0,
        0.0,
        mix(0.349, 0.0),
        mix(0.686, 1.0),
        mix(0.168, 0.0),
        0.0,
        0.0,
        mix(0.272, 0.0),
        mix(0.534, 0.0),
        mix(0.131, 1.0),
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
        0.0,
    ]
}

fn saturate(amount: f64) -> [f32; 20] {
    let amount = amount as f32;
    [
        0.213 + 0.787 * amount,
        0.715 - 0.715 * amount,
        0.072 - 0.072 * amount,
        0.0,
        0.0,
        0.213 - 0.213 * amount,
        0.715 + 0.285 * amount,
        0.072 - 0.072 * amount,
        0.0,
        0.0,
        0.213 - 0.213 * amount,
        0.715 - 0.715 * amount,
        0.072 + 0.928 * amount,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
        0.0,
    ]
}

fn hue_rotate(deg: f64) -> [f32; 20] {
    let radians = (deg as f32).to_radians();
    let cos = radians.cos();
    let sin = radians.sin();
    [
        0.213 + cos * 0.787 - sin * 0.213,
        0.715 - cos * 0.715 - sin * 0.715,
        0.072 - cos * 0.072 + sin * 0.928,
        0.0,
        0.0,
        0.213 - cos * 0.213 + sin * 0.143,
        0.715 + cos * 0.285 + sin * 0.140,
        0.072 - cos * 0.072 - sin * 0.283,
        0.0,
        0.0,
        0.213 - cos * 0.213 - sin * 0.787,
        0.715 - cos * 0.715 + sin * 0.715,
        0.072 + cos * 0.928 + sin * 0.072,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
        0.0,
    ]
}
