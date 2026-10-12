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

/// Resolve browser-owned pages before handing a URL to the network.
pub(crate) fn html_for(url: &blitz_traits::net::Url) -> Option<String> {
    (url.as_str() == crate::tls_status::RSA_FALLBACK_WHY_URL).then(rsa_fallback_html)
}

fn rsa_fallback_html() -> String {
    format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Why RSA fallback?</title></head>
<body style="{INTERNAL_PAGE_STYLE};padding:2rem;font:16px system-ui,sans-serif;max-width:48rem">
<h1>Why RSA fallback?</h1>
<p>This site's certificate chain uses RSA, in the server certificate or a certificate authority (CA) key.</p>
<p>On every new connection, chuzz tries ECC-only first. When it receives an RSA chain, it refuses it and sends alert 43 (unsupported_certificate), visible in the site's logs. Some servers instead reject the ECC-only hello with alert 40 (handshake_failure).</p>
<p>Then chuzz opens a separate connection allowing RSA only for authentication: RSA-PSS verifies the server handshake signature, and PKCS#1 v1.5 verifies certificate signatures. RSA is never used for key exchange or decryption.</p>
<p>Key exchange stays ECDHE (X25519 or P-256), and encryption stays ChaCha20-Poly1305 or AES-GCM. Forward secrecy is preserved.</p>
<p>RSA chains depend on RSA CA keys and larger, older algorithms. The concern is authentication strength and chuzz's policy preference for ECC, not a loss of forward secrecy.</p>
<p>Nothing is required of you. Prefer sites with ECDSA certificates, and tell the site you would like an ECC certificate chain.</p>
</body></html>"#
    )
}
