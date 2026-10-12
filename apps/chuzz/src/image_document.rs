//! Image navigation documents, shared by the window and headless loader.
//!
//! A top-level navigation to an image shows that image, the way every browser
//! does, instead of parsing its bytes as HTML text. The navigation URL stays
//! the document URL; this module only supplies the replacement HTML.

/// Recognize an image by its response type, or by its signature when the
/// server sent no type or a generic one.
///
/// SVG is left alone: it is a document of its own, not a raster to wrap.
pub fn is_image(content_type: Option<&str>, body: &[u8]) -> bool {
    let mime = content_type
        .and_then(|value| value.split(';').next())
        .map(|mime| mime.trim().to_ascii_lowercase());
    match mime.as_deref() {
        Some("image/svg+xml") => false,
        Some(mime) if mime.starts_with("image/") => true,
        None | Some("application/octet-stream") => has_image_signature(body),
        Some(_) => false,
    }
}

fn has_image_signature(body: &[u8]) -> bool {
    body.starts_with(&[0xFF, 0xD8, 0xFF])
        || body.starts_with(b"\x89PNG\r\n\x1a\n")
        || body.starts_with(b"GIF87a")
        || body.starts_with(b"GIF89a")
        || (body.len() >= 12 && body.starts_with(b"RIFF") && &body[8..12] == b"WEBP")
}

/// A document holding one `img` for `url`, centred on a dark page.
pub fn viewer_html(url: &str) -> String {
    let title = url::Url::parse(url)
        .ok()
        .and_then(|url| {
            url.path_segments()
                .and_then(|mut segments| segments.rfind(|segment| !segment.is_empty()))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "Image".to_owned());
    format!(
        r#"<!doctype html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, minimum-scale=0.1"><title>{}</title>
<style>
html,body{{margin:0;height:100%;background:#0e0e0e}}
body{{display:flex;align-items:center;justify-content:center}}
img{{display:block;max-width:100%;max-height:100%}}
</style></head><body><img src="{}" alt="{}"></body></html>
"#,
        escape(&title),
        escape(url),
        escape(&title)
    )
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}
