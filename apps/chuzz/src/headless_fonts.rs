//! The faces a headless build renders with: the headless-fonts crate, never the machine's fonts.
//!
//! One collection per process, built once. Each document gets a clone, which shares the font
//! blobs; the prototype stays unshared so one page's `@font-face` never reaches the next document.

use std::sync::OnceLock;

use blitz_dom::font_fallback;
use parley::fontique::{Collection, CollectionOptions, SourceCache};

fn font_context() -> blitz_dom::FontContext {
    static CONTEXT: OnceLock<blitz_dom::FontContext> = OnceLock::new();
    CONTEXT.get_or_init(build).clone()
}

fn build() -> blitz_dom::FontContext {
    let mut ctx = blitz_dom::FontContext {
        collection: Collection::new(CollectionOptions {
            shared: false,
            system_fonts: false,
        }),
        source_cache: SourceCache::new_shared(),
    };
    // BaseDocument::new registers the list bullet face only when it builds the context itself.
    font_fallback::register_face(&mut ctx, blitz_dom::BULLET_FONT);
    let mut family = |name: &str| {
        let mut id = None;
        for face in headless_fonts::FACES
            .iter()
            .filter(|face| face.family == name)
        {
            id = id.or(font_fallback::register_family(
                &mut ctx,
                face.family,
                face.data,
            ));
        }
        id.unwrap_or_else(|| panic!("headless-fonts has no faces for {name}"))
    };
    let sans = family(headless_fonts::SANS);
    let serif = family(headless_fonts::SERIF);
    let mono = family(headless_fonts::MONO);
    let devanagari = family(headless_fonts::DEVANAGARI);
    font_fallback::bind(&mut ctx, sans, serif, mono, &[devanagari], &[*b"Deva"]);
    ctx
}

/// A CSS family list with each name followed by its bundled stand-in, for text laid out outside the
/// document (canvas), which never reaches blitz-dom's family fallback.
pub(crate) fn family_list(list: &str) -> String {
    let mut out = Vec::new();
    for name in list
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        out.push(name.to_owned());
        if let Some(fallback) = headless_fonts::resolve(name) {
            out.push(format!("\"{fallback}\""));
        }
    }
    out.join(", ")
}

/// Give a document the headless faces and the CSS name fallback, unless it already has a context.
pub(crate) fn install(config: &mut blitz_dom::DocumentConfig) {
    if config.font_ctx.is_none() {
        config.font_ctx = Some(font_context());
    }
    config.family_fallback = Some(headless_fonts::resolve);
}
