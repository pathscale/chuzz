//! Checked limited-range I420 conversion, including 10-bit planar samples.

use super::MediaError;

pub(super) fn i420<T: Copy + Into<i32>>(
    width: u32,
    height: u32,
    depth: u8,
    planes: [&[T]; 3],
    rgba: &mut Vec<u8>,
) -> Result<(), MediaError> {
    if !matches!(depth, 8 | 10) {
        return Err(MediaError::unsupported("unsupported video sample depth"));
    }
    let width = width as usize;
    let height = height as usize;
    let pixels = width
        .checked_mul(height)
        .filter(|pixels| width >= 2 && height >= 2 && *pixels <= 3840 * 2160)
        .ok_or_else(|| MediaError::unsupported("unsupported video dimensions"))?;
    let chroma_width = width.div_ceil(2);
    let chroma_height = height.div_ceil(2);
    if planes[0].len() != pixels
        || planes[1].len() != chroma_width * chroma_height
        || planes[2].len() != chroma_width * chroma_height
    {
        return Err(MediaError::decode("invalid I420 plane lengths"));
    }
    rgba.resize(pixels * 4, 0);
    let shift = depth - 8;
    let rounding = if shift == 0 { 0 } else { 1 << (shift - 1) };
    let down = |sample: T| (sample.into() + rounding) >> shift;

    // BT.709 for HD, BT.601 for SD, until the codec exposes VUI colour data.
    let (red_v, green_u, green_v, blue_u) = if height >= 720 {
        (459, 55, 136, 541)
    } else {
        (409, 100, 208, 516)
    };
    for row in 0..height {
        for column in 0..width {
            let pixel = row * width + column;
            let chroma = (row / 2) * chroma_width + column / 2;
            let y = (down(planes[0][pixel]) - 16).max(0);
            let u = down(planes[1][chroma]) - 128;
            let v = down(planes[2][chroma]) - 128;
            let output = &mut rgba[pixel * 4..pixel * 4 + 4];
            output[0] = ((298 * y + red_v * v + 128) >> 8).clamp(0, 255) as u8;
            output[1] = ((298 * y - green_u * u - green_v * v + 128) >> 8).clamp(0, 255) as u8;
            output[2] = ((298 * y + blue_u * u + 128) >> 8).clamp(0, 255) as u8;
            output[3] = 255;
        }
    }
    Ok(())
}
