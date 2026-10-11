//! PDF navigation documents, shared by the window and headless loader.
//!
//! The toolkit owns parsing and rasterization. Browser policy still owns the
//! navigation URL; this module only supplies replacement HTML for its bytes.

use std::fmt::Write as _;
use std::sync::Arc;

use pdf_toolkit::core::pages::{Page, PageWalk};
use pdf_toolkit::core::{Document, Limits, OpenMode, PdfSource, Resolver};
use pdf_toolkit::render::RenderOptions;

const MAX_PAGES: usize = 50;
const PAGE_WIDTH: f32 = 1200.0;
const MAX_PAGE_HEIGHT: f32 = 4096.0;

/// Recognize a PDF by its response type, URL path or file signature.
pub fn is_pdf(url: &str, content_type: Option<&str>, body: &[u8]) -> bool {
    let typed = content_type.is_some_and(|value| {
        value
            .split(';')
            .next()
            .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/pdf"))
    });
    let named = url::Url::parse(url).ok().is_some_and(|url| {
        url.path()
            .rsplit_once('.')
            .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("pdf"))
    });
    // A `.pdf` name only counts when the server did not say otherwise: a login
    // or 404 page served as text/html at a .pdf URL stays HTML.
    let untyped = content_type.is_none_or(|value| {
        value
            .split(';')
            .next()
            .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/octet-stream"))
    });
    typed || (named && untyped) || body.starts_with(b"%PDF-")
}

/// Build a self-contained viewer without changing the navigation URL.
pub fn viewer_html(url: &str, bytes: &[u8]) -> String {
    let title = url::Url::parse(url)
        .ok()
        .and_then(|url| {
            url.path_segments()
                .and_then(|segments| segments.rfind(|segment| !segment.is_empty()))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "PDF".to_owned());
    let mut html = format!(
        r#"<!doctype html>
<html><head><meta charset="utf-8"><title>{}</title>
<style>
html,body{{margin:0;background:#303030;color:#f5f5f5;font-family:sans-serif}}
body{{padding:24px;box-sizing:border-box}}
main{{max-width:1200px;margin:0 auto}}
.pdf-page{{margin:0 0 24px}}
.pdf-page img{{display:block;max-width:100%;height:auto;margin:0 auto;background:white}}
.pdf-text{{margin:12px 0 0;white-space:pre-wrap;overflow-wrap:anywhere;font:16px sans-serif}}
</style></head><body><main aria-label="PDF pages">
"#,
        escape_html(&title)
    );
    if let Err(error) = render_document(bytes, &mut html) {
        let _ = write!(
            html,
            "<p role=\"alert\">PDF could not be opened: {}</p>",
            escape_html(&error)
        );
    }
    html.push_str("</main></body></html>");
    html
}

fn render_document(bytes: &[u8], html: &mut String) -> Result<(), String> {
    let source = PdfSource::new(Arc::<[u8]>::from(bytes)).map_err(|error| error.to_string())?;
    let document = Document::open_with(source, Limits::default(), OpenMode::Repair)
        .map_err(|error| error.to_string())?;
    let mut resolver = document.resolver();
    // Release the walk's mutable resolver borrow before rendering each page.
    let pages = PageWalk::new(&mut resolver)
        .map_err(|error| error.to_string())?
        .take(MAX_PAGES)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    if pages.is_empty() {
        return Err("the document contains no pages".to_owned());
    }
    // PDF standard fonts often have no embedded program. Use the same bundled
    // sans face in GUI and headless builds, without consulting the OS.
    let fallback_font = headless_fonts::FACES
        .iter()
        .find(|face| face.family == headless_fonts::SANS)
        .map(|face| Arc::<[u8]>::from(face.data));
    for page in &pages {
        let number = page.index + 1;
        html.push_str("<section class=\"pdf-page\">");
        if let Err(error) = append_image(html, &mut resolver, page, fallback_font.clone()) {
            let _ = write!(
                html,
                "<p role=\"alert\">Page {number} could not be rendered: {}</p>",
                escape_html(&error)
            );
        }
        match pdf_toolkit::text::page_text(&mut resolver, page) {
            Ok(text) => {
                let text = pdf_toolkit::text::layout(&text).plain_text();
                let _ = write!(
                    html,
                    "<pre class=\"pdf-text\" aria-label=\"Text of page {number}\">{}</pre>",
                    escape_html(&text)
                );
            }
            Err(error) => {
                let _ = write!(
                    html,
                    "<p>Text of page {number} could not be extracted: {}</p>",
                    escape_html(&error.to_string())
                );
            }
        }
        html.push_str("</section>");
    }
    if pages.len() == MAX_PAGES {
        let _ = write!(html, "<p>Showing at most the first {MAX_PAGES} pages.</p>");
    }
    Ok(())
}

fn page_scale(page: &Page) -> Result<f32, String> {
    let [x0, y0, x1, y1] = page.crop_box;
    if !page.user_unit.is_finite() || page.user_unit <= 0.0 {
        return Err("the page has an invalid user unit".to_owned());
    }
    let mut width = ((x1 - x0) * page.user_unit) as f32;
    let mut height = ((y1 - y0) * page.user_unit) as f32;
    match page.rotate % 360 {
        0 | 180 => {}
        90 | 270 => std::mem::swap(&mut width, &mut height),
        _ => return Err("the page has an invalid rotation".to_owned()),
    }
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return Err("the page has invalid display dimensions".to_owned());
    }
    // Normal pages are about 1200 pixels wide. Bound very tall pages as well,
    // so their aspect ratio cannot turn one raster into a huge allocation.
    let scale = (PAGE_WIDTH / width).min(MAX_PAGE_HEIGHT / height);
    if !scale.is_finite() || scale <= 0.0 {
        return Err("the page cannot be rasterized at a positive scale".to_owned());
    }
    Ok(scale)
}

fn append_image(
    html: &mut String,
    resolver: &mut Resolver<'_>,
    page: &Page,
    fallback_font: Option<Arc<[u8]>>,
) -> Result<(), String> {
    let options = RenderOptions {
        scale: page_scale(page)?,
        fallback_font,
    };
    let raster = pdf_toolkit::raster::render_page(resolver, page, &options, [255, 255, 255, 255])
        .map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, raster.width, raster.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
        writer
            .write_image_data(&raster.rgba)
            .map_err(|error| error.to_string())?;
        writer.finish().map_err(|error| error.to_string())?;
    }
    let number = page.index + 1;
    let _ = write!(
        html,
        "<img id=\"pdf-page-{number}\" alt=\"Page {number}\" aria-label=\"Page {number}\" \
         width=\"{}\" height=\"{}\" src=\"data:image/png;base64,",
        raster.width, raster.height
    );
    append_base64(html, &bytes);
    html.push_str("\">");
    Ok(())
}

fn append_base64(output: &mut String, bytes: &[u8]) {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    for chunk in bytes.chunks(3) {
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        output.push(char::from(ALPHABET[usize::from(a >> 2)]));
        output.push(char::from(ALPHABET[usize::from(((a & 3) << 4) | (b >> 4))]));
        output.push(if chunk.len() > 1 {
            char::from(ALPHABET[usize::from(((b & 15) << 2) | (c >> 6))])
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            char::from(ALPHABET[usize::from(c & 63)])
        } else {
            '='
        });
    }
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
