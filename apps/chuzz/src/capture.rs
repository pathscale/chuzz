//! Headless rendering, for seeing what the browser paints without a window.
//!
//! The windowed browser paints through Vello on wgpu. This paints the same
//! document with the CPU rasteriser into a buffer, which makes the result
//! inspectable: a page can be rendered in CI, diffed against a reference, or
//! checked by someone who cannot look at the screen.
//!
//! It deliberately shares the loader with the browser, so what it captures is
//! what the browser would show, not a second rendering path that could drift.

use std::path::Path;

use anyrender::render_to_buffer;
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitz_paint::paint_scene;
use blitz_traits::shell::{ColorScheme, Viewport};

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
    // nothing. Give the fetches a few passes to land.
    for _ in 0..40 {
        let pending = document.with_document(|document| {
            document.resolve(0.0);
            document.has_pending_critical_resources()
        });
        if !pending {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    // A final settle for anything that arrived on the last pass.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let buffer = document.with_document(|document| {
        document.set_viewport(Viewport::new(width, height, 1.0, ColorScheme::Light));
        document.resolve(0.0);
        render_to_buffer::<VelloCpuImageRenderer, _>(
            |scene| paint_scene(scene, document, 1.0, width, height, 0, 0),
            width,
            height,
        )
    });

    let file = std::fs::File::create(output)?;
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&buffer)?;
    writer.finish()?;
    Ok(())
}
