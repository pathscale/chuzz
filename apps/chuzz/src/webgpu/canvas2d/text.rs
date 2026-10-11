use std::sync::Mutex;

use blitz_dom::kurbo::{Affine, BezPath, Point, Rect, Shape};
use parley::style::{FontStyle, FontWeight, StyleProperty};
use parley::{FontContext, Layout, LayoutContext, PositionedLayoutItem};
use serde_json::{Value, json};
use skrifa::instance::{NormalizedCoord, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider};

use super::{DrawState, PATH_LIMIT, Result, lock, missing};

struct Font {
    size: f32,
    weight: f32,
    italic: bool,
    family: String,
}

fn length(value: &str, base: f64, root: f64) -> Option<f64> {
    let position = value.find(|c: char| !c.is_ascii_digit() && c != '.')?;
    let amount: f64 = value[..position].parse().ok()?;
    let factor = match &value[position..] {
        "px" => 1.0,
        "pt" => 96.0 / 72.0,
        "pc" => 16.0,
        "in" => 96.0,
        "cm" => 96.0 / 2.54,
        "mm" => 96.0 / 25.4,
        "q" => 96.0 / 101.6,
        "em" => base,
        "rem" => root,
        "%" => base / 100.0,
        _ => return None,
    };
    let size = amount * factor;
    (size.is_finite() && size > 0.0 && size <= 4096.0).then_some(size)
}

fn font(value: &str, base: f64, root: f64) -> Result<Font> {
    if value.len() > 4096 || !base.is_finite() || !root.is_finite() {
        return Err("invalid canvas font".to_owned());
    }
    let mut weight = 400.0;
    let mut italic = false;
    let mut consumed = 0;
    for word in value.split_whitespace() {
        let offset = value[consumed..].find(word).ok_or("invalid canvas font")? + consumed;
        consumed = offset + word.len();
        let size_word = word.split('/').next().unwrap_or(word);
        if let Some(size) = length(size_word, base, root) {
            let mut family = value[consumed..].trim();
            if family.starts_with('/') {
                family = family[1..].trim_start();
                let end = family
                    .find(char::is_whitespace)
                    .ok_or("canvas font has no family")?;
                family = family[end..].trim_start();
            }
            if family.is_empty() {
                return Err("canvas font has no family".to_owned());
            }
            return Ok(Font {
                size: size as f32,
                weight,
                italic,
                family: family.to_owned(),
            });
        }
        match word {
            "normal" => {}
            "italic" | "oblique" => italic = true,
            "bold" | "bolder" => weight = 700.0,
            "lighter" => weight = 300.0,
            "small-caps" => return Err(missing("font.small-caps")),
            "condensed" | "expanded" | "semi-condensed" | "semi-expanded" | "ultra-condensed"
            | "ultra-expanded" | "extra-condensed" | "extra-expanded" => {
                return Err(missing("font.stretch"));
            }
            _ => {
                weight = word
                    .parse::<f32>()
                    .ok()
                    .filter(|value| value.is_finite() && (1.0..=1000.0).contains(value))
                    .ok_or("invalid canvas font")?;
            }
        }
    }
    Err("canvas font has no size".to_owned())
}

pub(super) fn parse_font(value: &str, base: f64, root: f64) -> Result<String> {
    let font = font(value, base, root)?;
    Ok(format!(
        "{}{} {}px {}",
        if font.italic { "italic " } else { "" },
        font.weight,
        font.size,
        font.family,
    ))
}

struct Pen<'a> {
    path: &'a mut BezPath,
    matrix: Affine,
}

impl Pen<'_> {
    fn point(&self, x: f32, y: f32) -> Point {
        self.matrix * Point::new(x as f64, y as f64)
    }
}

impl OutlinePen for Pen<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.path.move_to(self.point(x, y));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.path.line_to(self.point(x, y));
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.path.quad_to(self.point(cx, cy), self.point(x, y));
    }
    fn curve_to(&mut self, ax: f32, ay: f32, bx: f32, by: f32, x: f32, y: f32) {
        self.path
            .curve_to(self.point(ax, ay), self.point(bx, by), self.point(x, y));
    }
    fn close(&mut self) {
        self.path.close_path();
    }
}

pub(super) struct Text {
    pub path: BezPath,
    pub width: f64,
    ascent: f64,
    descent: f64,
}

impl Text {
    pub fn offset(&self, state: &DrawState) -> Result<(f64, f64)> {
        let rtl = state.direction == "rtl";
        let x = match state.align.as_str() {
            "left" => 0.0,
            "right" => -self.width,
            "center" => -self.width * 0.5,
            "start" => {
                if rtl {
                    -self.width
                } else {
                    0.0
                }
            }
            "end" => {
                if rtl {
                    0.0
                } else {
                    -self.width
                }
            }
            _ => return Err("invalid canvas textAlign".to_owned()),
        };
        let y = match state.baseline.as_str() {
            "alphabetic" => 0.0,
            "top" => self.ascent,
            "hanging" => self.ascent * 0.8,
            "middle" => (self.ascent - self.descent) * 0.5,
            "bottom" | "ideographic" => -self.descent,
            _ => return Err("invalid canvas textBaseline".to_owned()),
        };
        Ok((x, y))
    }

    pub fn metrics(&self, state: &DrawState) -> Result<Value> {
        let (x, y) = self.offset(state)?;
        let bounds = if self.path.elements().is_empty() {
            Rect::ZERO
        } else {
            self.path.bounding_box()
        };
        let ink = !self.path.elements().is_empty();
        Ok(json!({
            "width": self.width,
            "actualBoundingBoxLeft": if ink { -bounds.x0 - x } else { 0.0 },
            "actualBoundingBoxRight": if ink { bounds.x1 + x } else { 0.0 },
            "actualBoundingBoxAscent": if ink { -bounds.y0 - y } else { 0.0 },
            "actualBoundingBoxDescent": if ink { bounds.y1 + y } else { 0.0 },
            "fontBoundingBoxAscent": self.ascent - y,
            "fontBoundingBoxDescent": self.descent + y,
            "emHeightAscent": self.ascent - y,
            "emHeightDescent": self.descent + y,
            "hangingBaseline": self.ascent * 0.8 - y,
            "alphabeticBaseline": -y,
            "ideographicBaseline": -self.descent - y
        }))
    }
}

pub(super) fn shape(value: &str, shorthand: &str, fonts: &Mutex<FontContext>) -> Result<Text> {
    if value.len() > 65_536 {
        return Err("canvas text limit reached in chuzz".to_owned());
    }
    let font = font(shorthand, 16.0, 16.0)?;
    // Canvas text replaces ASCII whitespace with spaces and never wraps.
    let value = value
        .chars()
        .map(|c| if c.is_ascii_whitespace() { ' ' } else { c })
        .collect::<String>();
    let mut fonts = lock(fonts);
    let mut context = LayoutContext::<[u8; 4]>::new();
    let mut builder = context.ranged_builder(&mut fonts, &value, 1.0, false);
    let family = crate::headless_family_list(&font.family);
    builder.push_default(StyleProperty::FontFamily(family.as_ref().into()));
    builder.push_default(StyleProperty::FontSize(font.size));
    builder.push_default(StyleProperty::FontWeight(FontWeight::new(font.weight)));
    builder.push_default(StyleProperty::FontStyle(if font.italic {
        FontStyle::Italic
    } else {
        FontStyle::Normal
    }));
    let mut layout: Layout<[u8; 4]> = builder.build(&value);
    layout.break_all_lines(None);
    drop(fonts);
    let mut result = Text {
        path: BezPath::new(),
        width: layout.full_width() as f64,
        ascent: 0.0,
        descent: 0.0,
    };
    let baseline = layout
        .lines()
        .next()
        .map_or(0.0, |line| line.metrics().baseline);
    for line in layout.lines() {
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(glyphs) = item else {
                continue;
            };
            let run = glyphs.run();
            result.ascent = result.ascent.max(run.metrics().ascent as f64);
            result.descent = result.descent.max(run.metrics().descent as f64);
            let face = FontRef::from_index(run.font().data.data(), run.font().index)
                .map_err(|error| error.to_string())?;
            let outlines = face.outline_glyphs();
            let coords = run
                .normalized_coords()
                .iter()
                .map(|value| NormalizedCoord::from_bits(*value))
                .collect::<Vec<_>>();
            let skew = run.synthesis().skew().map_or(Affine::IDENTITY, |angle| {
                Affine::skew(angle.to_radians().tan() as f64, 0.0)
            });
            for glyph in glyphs.positioned_glyphs() {
                let id = GlyphId::new(glyph.id);
                let Some(outline) = outlines.get(id) else {
                    if face.charmap().map(' ') == Some(id) {
                        continue;
                    }
                    return Err(missing("fillText.colorGlyph"));
                };
                let mut pen = Pen {
                    path: &mut result.path,
                    matrix: Affine::translate((glyph.x as f64, (glyph.y - baseline) as f64))
                        * Affine::scale_non_uniform(1.0, -1.0)
                        * skew,
                };
                outline
                    .draw(
                        DrawSettings::unhinted(Size::new(run.font_size()), coords.as_slice()),
                        &mut pen,
                    )
                    .map_err(|error| error.to_string())?;
                if result.path.elements().len() > PATH_LIMIT {
                    return Err("canvas text outline limit reached in chuzz".to_owned());
                }
            }
        }
    }
    Ok(result)
}
