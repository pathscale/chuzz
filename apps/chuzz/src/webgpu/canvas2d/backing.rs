use std::sync::{Arc, Mutex};

use anyrender::PaintScene;
use anyrender_vello_cpu::{CpuPixmap, ImageCache, VelloCpuScenePainter};
use blitz_dom::kurbo::{Affine, Rect};
use peniko::{Blob, Extend, Fill, ImageAlphaType, ImageData, ImageFormat};

use super::{image_brush, lock};

const TILE_SIZE: u32 = 64;

enum Pixels {
    Empty,
    Straight(Arc<Vec<u8>>),
    Premultiplied(CpuPixmap),
}

struct Tile {
    pixels: Pixels,
    painter: Option<VelloCpuScenePainter>,
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

impl Tile {
    fn straight(&mut self, length: usize) -> &mut Arc<Vec<u8>> {
        if !matches!(&self.pixels, Pixels::Straight(_)) {
            let mut bytes = vec![0; length];
            if let Pixels::Premultiplied(source) = &self.pixels {
                unpremultiply(source.as_ref(), &mut bytes);
            }
            self.pixels = Pixels::Straight(Arc::new(bytes));
        }
        let Pixels::Straight(bytes) = &mut self.pixels else {
            unreachable!("tile was converted to straight RGBA");
        };
        bytes
    }
}

/// Native tiles are authoritative. Straight RGBA is produced only for reads.
///
/// A full snapshot is immutable and cached until a write. It never pins the
/// mutable backing of another tile, so a small read after a small draw cannot
/// cause a full-canvas copy even when an earlier snapshot is still retained.
pub(super) struct Backing {
    width: u32,
    height: u32,
    columns: u32,
    tiles: Vec<Tile>,
    dirty: Vec<usize>,
    snapshot: Option<Arc<Vec<u8>>>,
}

impl Backing {
    pub(super) fn new(width: u32, height: u32) -> Self {
        let columns = width.div_ceil(TILE_SIZE);
        let rows = height.div_ceil(TILE_SIZE);
        Self {
            width,
            height,
            columns,
            tiles: (0..columns * rows)
                .map(|_| Tile {
                    pixels: Pixels::Empty,
                    painter: None,
                })
                .collect(),
            dirty: Vec::new(),
            snapshot: None,
        }
    }

    pub(super) fn byte_len(&self) -> usize {
        self.width as usize * self.height as usize * 4
    }

    /// Record the command only in tiles its conservative device bounds touch.
    ///
    /// Integer translations preserve the pixel grid. The caller retains the
    /// original canvas clip, brush transform and composition layer per tile.
    pub(super) fn paint(
        &mut self,
        mut area: Rect,
        cache: &Arc<Mutex<ImageCache>>,
        mut draw: impl FnMut(Affine, &mut VelloCpuScenePainter),
    ) -> bool {
        let canvas = Rect::new(0.0, 0.0, self.width as f64, self.height as f64);
        if ![area.x0, area.y0, area.x1, area.y1]
            .iter()
            .all(|value| value.is_finite())
        {
            area = canvas;
        }
        area = area.intersect(canvas);
        if area.x1 <= area.x0 || area.y1 <= area.y0 {
            return false;
        }
        let x0 = area.x0.floor() as u32 / TILE_SIZE;
        let y0 = area.y0.floor() as u32 / TILE_SIZE;
        let x1 = (area.x1.ceil() as u32).div_ceil(TILE_SIZE);
        let y1 = (area.y1.ceil() as u32).div_ceil(TILE_SIZE);
        self.snapshot = None;
        for row in y0..y1 {
            for column in x0..x1 {
                let x = column * TILE_SIZE;
                let y = row * TILE_SIZE;
                let width = TILE_SIZE.min(self.width - x);
                let height = TILE_SIZE.min(self.height - y);
                let index = (row * self.columns + column) as usize;
                let tile = &mut self.tiles[index];
                if tile.painter.is_none() {
                    let mut painter = VelloCpuScenePainter::new(width as u16, height as u16)
                        .with_image_cache(Arc::clone(cache));
                    match &tile.pixels {
                        Pixels::Empty => {}
                        Pixels::Premultiplied(pixels) => painter.seed_pixmap(pixels),
                        Pixels::Straight(pixels) => {
                            let image = ImageData {
                                data: Blob::new(
                                    Arc::clone(pixels) as Arc<dyn AsRef<[u8]> + Send + Sync>
                                ),
                                format: ImageFormat::Rgba8,
                                alpha_type: ImageAlphaType::Alpha,
                                width,
                                height,
                            };
                            let id = image.data.id();
                            painter.fill(
                                Fill::NonZero,
                                Affine::IDENTITY,
                                image_brush(image, false, Extend::Pad, Extend::Pad).as_ref(),
                                None,
                                &Rect::new(0.0, 0.0, width as f64, height as f64),
                            );
                            // The recorded paint owns its converted image. A
                            // one-use seed must not occupy the shared cache.
                            lock(cache).remove(id);
                        }
                    }
                    tile.painter = Some(painter);
                    self.dirty.push(index);
                }
                draw(
                    Affine::translate((-(x as f64), -(y as f64))),
                    tile.painter.as_mut().expect("dirty tile has a painter"),
                );
            }
        }
        true
    }

    pub(super) fn flush(&mut self) {
        for index in self.dirty.drain(..) {
            let tile = &mut self.tiles[index];
            let painter = tile.painter.take().expect("dirty tile has a painter");
            tile.pixels = Pixels::Premultiplied(painter.finish_shared());
        }
    }

    /// Read an in-bounds row into straight RGBA, converting only that row.
    pub(super) fn read_row(&self, mut x: u32, y: u32, mut output: &mut [u8]) {
        while !output.is_empty() {
            let column = x / TILE_SIZE;
            let row = y / TILE_SIZE;
            let tile_width = TILE_SIZE.min(self.width - column * TILE_SIZE);
            let local_x = x % TILE_SIZE;
            let local_y = y % TILE_SIZE;
            let count = (output.len() / 4).min((tile_width - local_x) as usize) * 4;
            let from = (local_y as usize * tile_width as usize + local_x as usize) * 4;
            let (target, rest) = output.split_at_mut(count);
            match &self.tiles[(row * self.columns + column) as usize].pixels {
                Pixels::Empty => target.fill(0),
                Pixels::Straight(source) => {
                    target.copy_from_slice(&source[from..from + count]);
                }
                Pixels::Premultiplied(source) => {
                    unpremultiply(&source.as_ref()[from..from + count], target);
                }
            }
            x += (count / 4) as u32;
            output = rest;
        }
    }

    /// Replace an in-bounds row with exact putImageData bytes.
    pub(super) fn write_row(&mut self, mut x: u32, y: u32, mut input: &[u8]) {
        self.snapshot = None;
        while !input.is_empty() {
            let column = x / TILE_SIZE;
            let row = y / TILE_SIZE;
            let tile_width = TILE_SIZE.min(self.width - column * TILE_SIZE);
            let tile_height = TILE_SIZE.min(self.height - row * TILE_SIZE);
            let local_x = x % TILE_SIZE;
            let local_y = y % TILE_SIZE;
            let count = (input.len() / 4).min((tile_width - local_x) as usize) * 4;
            let to = (local_y as usize * tile_width as usize + local_x as usize) * 4;
            let tile = &mut self.tiles[(row * self.columns + column) as usize];
            let bytes = tile.straight(tile_width as usize * tile_height as usize * 4);
            Arc::make_mut(bytes)[to..to + count].copy_from_slice(&input[..count]);
            x += (count / 4) as u32;
            input = &input[count..];
        }
    }

    pub(super) fn snapshot(&mut self) -> Arc<Vec<u8>> {
        self.flush();
        if let Some(bytes) = &self.snapshot {
            return Arc::clone(bytes);
        }
        let mut bytes = vec![0; self.byte_len()];
        if self.width != 0 {
            for (y, row) in bytes.chunks_exact_mut(self.width as usize * 4).enumerate() {
                self.read_row(0, y as u32, row);
            }
        }
        let bytes = Arc::new(bytes);
        self.snapshot = Some(Arc::clone(&bytes));
        bytes
    }
}
