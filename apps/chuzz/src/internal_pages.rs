//! The documents the browser writes itself.
//!
//! View source and the error page are pages, not chrome: they are built as
//! HTML and loaded into a viewport like anything else. That makes them
//! reachable from the headless host as well as the window, which is why they
//! live here rather than in `browser`, whose Tauri command surface a headless
//! build does not compile.

/// Colours for the pages the browser writes itself.
///
/// Explicit, and light, like every other browser's error and source pages.
/// These documents declare no colours of their own, so they inherited the
/// engine's defaults: black text on a transparent background, over a viewport
/// the shell paints with the dark theme surface. The source of a page was
/// therefore rendered, laid out, and unreadable, which is indistinguishable
/// from not being rendered at all and was reported as exactly that.
///
/// Not a theme token. These are documents in a page viewport, not part of the
/// chrome, and nothing in a page can reach the shell's custom properties.
pub(crate) const INTERNAL_PAGE_STYLE: &str = "margin:0;background:#f6f6f7;color:#16181d";

/// A page's own source, as a document.
///
/// Escaped and put in a `<pre>`, which is the whole job: the point of view
/// source is that what you read is what arrived, so nothing here may reformat,
/// pretty-print or re-serialise it. A document that showed a parsed and
/// re-emitted tree would be answering a different question, and for a page
/// whose claim is "there is no script here" it would be the wrong answer.
pub(crate) fn source_html(text: &str) -> String {
    let escaped = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>Source</title></head>
<body style="{INTERNAL_PAGE_STYLE}"><pre style="margin:0;padding:1rem;font:13px ui-monospace,monospace;white-space:pre-wrap;word-break:break-word">{escaped}</pre></body></html>"#
    )
}
