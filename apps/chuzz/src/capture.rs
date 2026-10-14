//! Headless rendering, for seeing what the browser paints without a window.
//!
//! The windowed browser paints through Vello on wgpu. This paints the same
//! document with the CPU rasteriser into a buffer, which makes the result
//! inspectable: a page can be rendered in CI, diffed against a reference, or
//! checked by someone who cannot look at the screen.
//!
//! It deliberately shares the loader with the browser, so what it captures is
//! what the browser would show, not a second rendering path that could drift.
//!
//! [`capture`] fetches a URL and renders its parsed document.

use std::path::{Path, PathBuf};

use anyrender::render_to_buffer;
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitz_dom::BaseDocument;
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

/// CSS pixel size to lay the page out at. Defaults to 1440 by 960.
///
/// A capture is only comparable against a reference browser when both laid the
/// page out at the same width: every responsive breakpoint, every percentage
/// width and every centred box moves with it. These were exported by
/// `scripts/render-check.sh` and read by nothing, so a run at another size
/// silently produced the default and the resulting diff was all viewport and
/// no signal.
pub fn capture_viewport() -> (u32, u32) {
    fn dimension(name: &str, fallback: u32) -> u32 {
        std::env::var(name)
            .ok()
            .and_then(|value| value.parse().ok())
            .filter(|pixels: &u32| *pixels > 0)
            .unwrap_or(fallback)
    }

    (
        dimension("CHUZZ_CAPTURE_WIDTH", 1440),
        dimension("CHUZZ_CAPTURE_HEIGHT", 960),
    )
}

/// Which colour scheme to render at. Defaults to dark.
///
/// A site that respects `prefers-color-scheme` is a different page in each, so
/// a capture fixed at one of them cannot be compared against a reference
/// browser sitting in the other: the diff is all theme and no signal.
///
/// Dark rather than light, because the window this exists to explain follows
/// the OS appearance and the machines here are dark. A capture that disagreed
/// with the window on the first pixel would be answering a question nobody
/// asked. Set `CHUZZ_CAPTURE_SCHEME=light` for the other one.
///
/// This reports the hint; it does not impose a theme. A page that picks its
/// palette from a stored preference rather than from the media query renders
/// the same either way, which is worth knowing before reading a diff as a
/// rendering fault. support.cafe is exactly that case.
fn capture_color_scheme() -> ColorScheme {
    match std::env::var("CHUZZ_CAPTURE_SCHEME").ok().as_deref() {
        Some("light") | Some("Light") => ColorScheme::Light,
        _ => ColorScheme::Dark,
    }
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
    let cookies = std::sync::Arc::new(
        crate::cookie_store::BrowserCookieStore::open(
            crate::cookie_store::profile_directory().join("cookies"),
        )
        .await?,
    );
    let net_provider = std::sync::Arc::new(
        crate::document_loader::NetProvider::with_user_agent_and_cookies(
            None,
            &crate::identity::user_agent_from_env(),
            cookies,
        ),
    );
    // No prelude: a capture loads one page and has nothing to carry into it.
    let mut document = crate::document_loader::load_for_capture(
        request,
        std::sync::Arc::clone(&net_provider),
        "",
        None,
    )
    .await?;

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
        nagoya::sleep(std::time::Duration::from_millis(50)).await;
    }
    // A final settle: the last response may still be sitting on the channel,
    // and it is only applied by the `resolve` inside the render below.
    nagoya::sleep(std::time::Duration::from_millis(200)).await;

    let scale = capture_scale();
    // Physical pixels, the way a window sizes its surface.
    let device_width = (width as f32 * scale).round() as u32;
    let device_height = (height as f32 * scale).round() as u32;
    let tree = tree_dump_path();
    let buffer = document.with_document(|document| {
        paint(
            document,
            scale,
            device_width,
            device_height,
            tree.as_deref(),
        )
    });

    let result = write_png(&buffer, device_width, device_height, output);
    net_provider.cookie_store().flush().await?;
    result
}

/// Lay `document` out at `scale`, dump the tree if asked, and rasterise.
fn paint(
    document: &mut BaseDocument,
    scale: f32,
    device_width: u32,
    device_height: u32,
    tree_out: Option<&Path>,
) -> Vec<u8> {
    document.set_viewport(Viewport::new(
        device_width,
        device_height,
        scale,
        capture_color_scheme(),
    ));
    document.resolve(0.0);
    // Written from the same settled document the pixels come from, so a box in
    // the dump is the box that was painted.
    if let Some(path) = tree_out
        && let Err(error) = crate::dump::write_tree(document, path)
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
}

fn write_png(
    buffer: &[u8],
    width: u32,
    height: u32,
    output: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let file = std::fs::File::create(output)?;
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(buffer)?;
    writer.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("chuzz-capture-source");
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir.join(name)
    }

    /// One box from the tree dump, whose format is
    /// `[id] tag#id.class  x,y w*h  display position`.
    #[derive(Debug)]
    struct Box_ {
        name: String,
    }

    fn boxes(dump: &str) -> Vec<Box_> {
        dump.lines()
            .filter(|line| !line.starts_with('#'))
            .filter_map(|line| {
                let mut tokens = line.split_whitespace();
                let _node_id = tokens.next()?;
                let name = tokens.next()?;
                Some(Box_ {
                    name: name.to_owned(),
                })
            })
            .collect()
    }

    /// Decode the PNG and count how many pixels differ from the background.
    ///
    /// The background is defined as the top-left pixel rather than assumed to
    /// be white or transparent: a bare document has no `background-color`, so
    /// what fills the canvas is the renderer's business and not something this
    /// test should be asserting about.
    fn non_background_pixels(png_path: &Path) -> Painted {
        let file = std::io::BufReader::new(
            std::fs::File::open(png_path).expect("the capture should have written a PNG"),
        );
        let mut reader = png::Decoder::new(file)
            .read_info()
            .expect("the PNG header should decode");
        let mut buffer = vec![0; reader.output_buffer_size().expect("a bounded frame")];
        let info = reader
            .next_frame(&mut buffer)
            .expect("the PNG data should decode");
        assert_eq!(info.color_type, png::ColorType::Rgba);
        assert_eq!(info.bit_depth, png::BitDepth::Eight);

        let pixels = &buffer[..info.buffer_size()];
        let (rgba, _) = pixels.as_chunks::<4>();
        let background = rgba.first().copied().unwrap_or_default();
        let mut painted = Painted {
            differing: 0,
            total: pixels.len() / 4,
        };
        for pixel in rgba {
            if *pixel != background {
                painted.differing += 1;
            }
        }
        painted
    }

    struct Painted {
        differing: usize,
        total: usize,
    }

    /// `view-source:` renders through the capture path, escaped rather than
    /// parsed.
    ///
    /// Hermetic on purpose. The loader's `view-source:` branch fetches the
    /// inner URL, so driving it end to end would put a network round trip in
    /// the suite and make this test fail for reasons that have nothing to do
    /// with rendering. What the branch does *after* the fetch is wrap the bytes
    /// with `browser::source_html` and parse that, and it is the wrapping that
    /// can regress: escaping that stopped escaping would render the source as a
    /// page, which is the one thing view source must never do.
    ///
    /// So this paints exactly what the loader would hand to the renderer, and
    /// reads the pixels back from the rendered source document.
    #[test]
    fn a_source_document_paints_its_markup_rather_than_rendering_it() {
        use blitz_dom::{BaseDocument, DocumentConfig};

        // A page whose rendered form is unmistakably different from its source:
        // an <h1> would paint large and bold, and the tags would vanish.
        const PAGE: &str = "<h1>Example Domain</h1><p>a &amp; b</p>";
        let html = crate::internal_pages::source_html(PAGE);

        // The escaping is the contract the picture depends on, so assert it
        // before painting: a failure here explains a failure below.
        assert!(
            html.contains("&lt;h1&gt;") && !html.contains("<h1>"),
            "the markup must be escaped, not embedded: {html}"
        );
        assert!(
            html.contains("&amp;amp;"),
            "an entity in the source must itself be escaped, or the picture \
             shows `&` where the server sent `&amp;`: {html}"
        );

        let document = blitz_html::HtmlDocument::from_html(
            &html,
            DocumentConfig {
                html_parser_provider: Some(std::sync::Arc::new(blitz_html::HtmlProvider)),
                ..Default::default()
            },
        );
        let mut document: BaseDocument = document.into_inner();

        let png = scratch("view-source.png");
        let _ = std::fs::remove_file(&png);
        let tree = scratch("view-source-tree.txt");
        let _ = std::fs::remove_file(&tree);
        let buffer = super::paint(&mut document, 1.0, 1440, 960, Some(&tree));
        super::write_png(&buffer, 1440, 960, &png).expect("the png should be written");

        // A blank image would satisfy any
        // weaker assertion, and blank is this path's characteristic failure.
        let painted = non_background_pixels(&png);
        assert!(
            painted.differing > 250,
            "the source document painted almost nothing ({} of {}), which is \
             what a missing font looks like",
            painted.differing,
            painted.total
        );

        // And the tree proves it is *source*, not a rendered page: the text
        // sits in a <pre>, and no <h1> was ever created from the escaped input.
        let dump = std::fs::read_to_string(&tree).expect("the tree dump should be written");
        assert!(
            boxes(&dump).iter().any(|box_| box_.name == "pre"),
            "the source should be laid out in a <pre>: {dump}"
        );
        assert!(
            !boxes(&dump).iter().any(|box_| box_.name == "h1"),
            "an <h1> in the tree means the markup was rendered rather than \
             shown: {dump}"
        );
    }
}
