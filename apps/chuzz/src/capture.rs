//! Headless rendering, for seeing what the browser paints without a window.
//!
//! The windowed browser paints through Vello on wgpu. This paints the same
//! document with the CPU rasteriser into a buffer, which makes the result
//! inspectable: a page can be rendered in CI, diffed against a reference, or
//! checked by someone who cannot look at the screen.
//!
//! It deliberately shares the loader with the browser, so what it captures is
//! what the browser would show, not a second rendering path that could drift.

use std::path::{Path, PathBuf};

use anyrender::render_to_buffer;
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitz_paint::paint_scene;
use blitz_traits::shell::{ColorScheme, Viewport};

/// Where to write the laid-out tree alongside the PNG, when asked.
///
/// An environment variable rather than a flag: the capture path parses its own
/// arguments by hand, and a second positional would be taken for the URL.
fn tree_dump_path() -> Option<PathBuf> {
    std::env::var_os("CHUZZ_CAPTURE_TREE").map(PathBuf::from)
}

/// Device pixel ratio to render at. Defaults to 1.
///
/// The window renders the page as a sub-document on a retina display, so its
/// scale is 2. A capture fixed at 1 therefore cannot reproduce anything that
/// only goes wrong when the scale is not 1, which is a whole class of paint
/// fault. The output is `width * scale` by `height * scale` pixels, laid out
/// at `width` by `height` CSS pixels, the same as the window does.
fn capture_scale() -> f32 {
    std::env::var("CHUZZ_CAPTURE_SCALE")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|scale: &f32| *scale > 0.0)
        .unwrap_or(1.0)
}

/// Render `url` at the given size and write a PNG to `output`.
pub async fn capture(
    url: &str,
    width: u32,
    height: u32,
    output: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let request = crate::nav::request_from_input(url).ok_or("no URL to capture")?;

    // The same loader the browser uses, so a capture cannot silently diverge
    // from what a tab would render.
    let net_provider = std::sync::Arc::new(blitz_net::Provider::new(None));
    let mut document = crate::document_loader::load_for_capture(request, net_provider).await?;

    // Images are fetched asynchronously and applied through the document's
    // message channel, which only drains inside `resolve`. Resolving once
    // captures the page before any image has arrived, so every <img> paints
    // nothing.
    //
    // `has_pending_critical_resources` is not enough on its own: only
    // stylesheets in <head> are critical, so it reports "settled" while every
    // image is still in flight. Wait on the images too.
    for _ in 0..80 {
        let pending = document.with_document(|document| {
            document.resolve(0.0);
            document.has_pending_critical_resources() || document.pending_image_count() > 0
        });
        if !pending {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    // A final settle: the last response may still be sitting on the channel,
    // and it is only applied by the `resolve` inside the render below.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let scale = capture_scale();
    // Physical pixels, the way a window sizes its surface.
    let device_width = (width as f32 * scale).round() as u32;
    let device_height = (height as f32 * scale).round() as u32;
    let buffer = document.with_document(|document| {
        document.set_viewport(Viewport::new(
            device_width,
            device_height,
            scale,
            ColorScheme::Light,
        ));
        document.resolve(0.0);
        // Written from the same settled document the pixels come from, so a box
        // in the dump is the box that was painted.
        if let Some(path) = tree_dump_path()
            && let Err(error) = crate::dump::write_tree(document, &path)
        {
            eprintln!("chuzz: could not write the tree dump: {error}");
        }
        render_to_buffer::<VelloCpuImageRenderer, _>(
            |scene| {
                paint_scene(
                    scene,
                    document,
                    scale as f64,
                    device_width,
                    device_height,
                    0,
                    0,
                )
            },
            device_width,
            device_height,
        )
    });

    let file = std::fs::File::create(output)?;
    let mut encoder = png::Encoder::new(file, device_width, device_height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&buffer)?;
    writer.finish()?;
    Ok(())
}
