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
//! There are two ways in, and they differ only in where the document comes
//! from. [`capture`] fetches a URL and parses HTML. [`capture_wasm`] hands an
//! empty document to a WebAssembly guest and lets it build the tree through
//! `blitz-wasm`, with no URL, no fetch and no parser anywhere in the path.
//! Everything from layout onwards is the same code for both.

use std::path::{Path, PathBuf};

use anyrender::render_to_buffer;
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitz_dom::{BaseDocument, DocumentConfig};
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

    write_png(&buffer, device_width, device_height, output)
}

/// Render a document built by a WebAssembly guest and write a PNG to `png_out`.
///
/// The sibling of [`capture`], not a variant of it. That function renders a
/// *fetched* page: it resolves a URL, goes through the net provider, and parses
/// the bytes that come back. A wasm-built document has none of that. There is
/// no URL, nothing is fetched, no HTML is parsed, and the tree exists only
/// because the guest called `create_element` and `append_child` across the ABI.
/// So the two share the painter and the dump and nothing before them.
///
/// The document handed to the guest is empty apart from `<html><body>`, and the
/// body is seeded as the guest's mount handle ([`blitz_wasm::MOUNT`]). Without
/// that seed a guest can build a tree and has nowhere to put it.
pub fn capture_wasm(
    module_path: &Path,
    width: u32,
    height: u32,
    png_out: &Path,
    tree_out: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    let scale = capture_scale();
    let device_width = (width as f32 * scale).round() as u32;
    let device_height = (height as f32 * scale).round() as u32;

    // The viewport goes in at construction because it is what the style device
    // is built from, so a document created without one resolves media queries
    // and viewport units against a default that is not the size being captured.
    //
    // The seeding and the guest run are `wasm_page`'s, shared with `--wasm` in
    // the window, so the two cannot render the same module differently.
    let (document, mount) = crate::wasm_page::empty_document(DocumentConfig {
        viewport: Some(Viewport::new(
            device_width,
            device_height,
            scale,
            capture_color_scheme(),
        )),
        ..Default::default()
    });
    let mut document = crate::wasm_page::run_guest(module_path, document, mount)?;

    // Resolve once, then paint.
    //
    // Deliberately not the settling loop `capture` runs. That loop waits on
    // `pending_image_count` and `has_pending_critical_resources`, which count
    // subresources discovered by parsing HTML. Nothing here parses HTML and the
    // guest cannot request a subresource, so both are zero from the first pass
    // and every iteration would be a sleep with nothing to wait for.
    let buffer = paint(&mut document, scale, device_width, device_height, tree_out);

    write_png(&buffer, device_width, device_height, png_out)
}

/// Lay `document` out at `scale`, dump the tree if asked, and rasterise.
///
/// Shared by both entry points on purpose: a wasm capture that painted through
/// its own copy of this could drift from what `--capture` produces, and then a
/// difference between the two would mean nothing.
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

/// Proof that the wasm path renders, rather than that it runs.
///
/// A test that only checked the PNG exists would pass on a blank image, and
/// blank is the exact failure this path is prone to: strip `system-fonts` and
/// parley finds no face, every line shapes to zero height, the guest still
/// reports OK, the tree still has all the right nodes, and the picture is
/// empty. So the pixels are decoded and the boxes are read back.
#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("chuzz-capture-wasm-tests");
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir.join(name)
    }

    /// One box from the tree dump, whose format is
    /// `[id] tag#id.class  x,y w*h  display position`.
    #[derive(Debug)]
    struct Box_ {
        name: String,
        y: f32,
        width: f32,
        height: f32,
    }

    fn boxes(dump: &str) -> Vec<Box_> {
        dump.lines()
            .filter(|line| !line.starts_with('#'))
            .filter_map(|line| {
                let mut tokens = line.split_whitespace();
                let _node_id = tokens.next()?;
                let name = tokens.next()?;
                let (_x, y) = tokens.next()?.split_once(',')?;
                let (width, height) = tokens.next()?.split_once('*')?;
                Some(Box_ {
                    name: name.to_owned(),
                    y: y.parse().ok()?,
                    width: width.parse().ok()?,
                    height: height.parse().ok()?,
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
        let background = &pixels[0..4];
        let mut painted = Painted {
            differing: 0,
            total: pixels.len() / 4,
            lowest: 0,
        };
        for (index, pixel) in pixels.chunks_exact(4).enumerate() {
            if pixel != background {
                painted.differing += 1;
                painted.lowest = painted.lowest.max(index as u32 / info.width);
            }
        }
        painted
    }

    struct Painted {
        differing: usize,
        total: usize,
        /// The bottom-most row of pixels that is not the background.
        lowest: u32,
    }

    #[test]
    fn a_wasm_built_page_paints_something() {
        let png = scratch("paints.png");
        let _ = std::fs::remove_file(&png);
        super::capture_wasm(&crate::wasm_page::fixture_module(), 1440, 960, &png, None)
            .expect("the wasm capture should succeed");

        let painted = non_background_pixels(&png);
        // Not "at least one": a single stray pixel would satisfy that and is
        // not a rendered page.
        //
        // The measured figure for this page is 1231 of 1382400, all of it
        // antialiased black text on a fully transparent canvas. It is a small
        // number because the demo guest sets no background and draws no
        // borders, so glyph coverage is the *only* thing that paints. That is
        // what makes this assertion load-bearing rather than decorative: with
        // no font the count is not merely lower, it is exactly zero. 250
        // leaves room for a machine whose default serif is lighter than this
        // one's without leaving room for a blank page.
        assert!(
            painted.differing > 250,
            "the capture is blank: {} of {} pixels differ from the background. \
             This is what a build without `system-fonts` produces, because parley finds no \
             face, every line shapes to zero height, and the guest still reports OK.",
            painted.differing,
            painted.total
        );

        // Bounded by the layout, not merely present. The tree puts the whole
        // page in the top 187 rows, so ink below that would mean the pixels
        // and the boxes disagree, and then neither could be trusted to explain
        // the other.
        assert!(
            painted.lowest < 300,
            "something painted at row {}, far below the {}-tall page the tree describes",
            painted.lowest,
            187
        );
    }

    #[test]
    fn the_tree_dump_shows_the_panel_and_its_rows() {
        let png = scratch("tree.png");
        let tree = scratch("tree.txt");
        let _ = std::fs::remove_file(&tree);
        super::capture_wasm(
            &crate::wasm_page::fixture_module(),
            1440,
            960,
            &png,
            Some(&tree),
        )
        .expect("the wasm capture should succeed");

        let dump = std::fs::read_to_string(&tree).expect("the tree dump should have been written");
        let boxes = boxes(&dump);
        assert!(
            dump.contains("\"Blitz\""),
            "the heading's text node is missing from the dump:\n{dump}"
        );

        let panels: Vec<&Box_> = boxes.iter().filter(|b| b.name.contains(".panel")).collect();
        assert_eq!(panels.len(), 1, "expected one .panel, got {panels:?}");
        let panel = panels[0];
        assert_eq!(panel.name, "div#root.panel");
        assert!(
            panel.width > 0.0 && panel.width <= 1440.0,
            "the panel's width is not plausible: {panel:?}"
        );
        // A heading and three paragraphs at default UA sizes cannot be shorter
        // than this, and cannot overflow the viewport either. A zero height is
        // the blank-render failure; a height in the thousands would mean the
        // dump is not measuring what was painted.
        assert!(
            panel.height > 50.0 && panel.height <= 960.0,
            "the panel's height is not plausible: {panel:?}"
        );

        let rows: Vec<&Box_> = boxes.iter().filter(|b| b.name.contains(".row")).collect();
        assert_eq!(rows.len(), 3, "expected three .row elements, got {rows:?}");
        for row in &rows {
            assert_eq!(row.name, "p.row");
            assert!(
                row.width > 0.0 && row.width <= panel.width,
                "a row is wider than its panel, or has no width: {row:?}"
            );
            // One line of default-size text. Zero means nothing shaped.
            assert!(
                row.height > 4.0 && row.height < 100.0,
                "a row's height is not one line of text: {row:?}"
            );
            assert!(
                row.y >= panel.y && row.y + row.height <= panel.y + panel.height,
                "a row falls outside its panel: {row:?} against {panel:?}"
            );
        }
        // Stacked, not piled on one line, which is what proves they are block
        // children of the panel rather than three nodes that merely exist.
        assert!(
            rows.windows(2).all(|pair| pair[1].y > pair[0].y),
            "the rows do not stack in order: {rows:?}"
        );
    }
}
