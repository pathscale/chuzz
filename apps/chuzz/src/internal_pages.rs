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

#[cfg(feature = "gui")]
pub(crate) fn error_html(error: &str) -> String {
    load_error_html(error, "")
}

fn load_error_html(error: &str, details: &str) -> String {
    let escaped = error
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>Page not available</title></head><body style="{INTERNAL_PAGE_STYLE};padding:2rem;font:14px system-ui,sans-serif"><h1>Page not available</h1><p>{escaped}</p>{details}</body></html>"#
    )
}

/// A policy refusal is the error document for this navigation, with its address retained.
pub(crate) fn tls_refused_html(
    url: &blitz_traits::net::Url,
    reason: blitz_net::TlsPolicyError,
) -> String {
    let url = crate::tls_status::navigation_url(url);
    let host = url
        .host_str()
        .unwrap_or("unknown")
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let alert = crate::tls_status::alert_description(reason);
    let explainer = crate::tls_status::TLS_REFUSED_WHY_URL;
    let details = format!(
        r#"<p>Requested host: <strong>{host}</strong></p><p>{alert}</p><p>The requested page was not loaded. chuzz does not retry with weaker algorithms.</p><p><a href="{explainer}">Why chuzz refused this TLS connection</a></p>"#
    );
    load_error_html(&reason.to_string(), &details)
}

/// Resolve browser-owned pages before handing a URL to the network.
pub(crate) fn html_for(url: &blitz_traits::net::Url) -> Option<String> {
    (url.as_str() == crate::tls_status::TLS_REFUSED_WHY_URL).then(tls_refused_explainer_html)
}

fn tls_refused_explainer_html() -> String {
    format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Why TLS was refused</title></head>
<body style="{INTERNAL_PAGE_STYLE};padding:2rem;font:16px system-ui,sans-serif;max-width:48rem">
<h1>Why TLS was refused</h1>
<p>chuzz requires ECDSA or Ed25519 authentication throughout the certificate chain and in TLS handshake signatures, ECDHE key exchange, and ChaCha20-Poly1305 encryption only. RSA and AES are forbidden.</p>
<p>A TLS policy refusal ends the load. chuzz does not retry with weaker algorithms.</p>
<p>If chuzz refuses an RSA certificate chain or another unsupported certificate key, it sends alert 43 unsupported_certificate. That is the alert the server can see in its logs.</p>
<p>Some servers instead reject the permitted TLS offer and send alert 40 handshake_failure to chuzz. Others send alert 71 insufficient_security. These server alerts do not establish whether the server requires RSA, AES, or another unsupported algorithm. The error page reports the observed reason and alert direction.</p>
<p>The site operator can fix an RSA certificate-chain refusal by serving an ECDSA certificate chain, including compatible intermediate and trust-anchor keys and signatures. The server must also support ECDHE and a ChaCha20-Poly1305 cipher suite. An ECDSA leaf alone does not fix a chain that still requires RSA.</p>
<p>Ask the site operator to provide that TLS configuration. chuzz cannot load the site until its TLS configuration satisfies this policy.</p>
</body></html>"#
    )
}
