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
    let chacha = matches!(reason,
        blitz_net::TlsPolicyError::CipherSuite
        | blitz_net::TlsPolicyError::PeerRejectedAlgorithms { .. });
    #[cfg(feature = "sandbox2-witness")]
    let chacha = chacha && std::env::var("SANDBOX2_TLS_DEFECT").ok().as_deref() != Some("tls-name");
    let details = if chacha {
        format!(r#"<h2 id="tls-chacha-required">tls-chacha-required</h2>{details}"#)
    } else {
        details
    };
    load_error_html(&reason.to_string(), &details)
}

/// Resolve browser-owned pages before handing a URL to the network.
pub(crate) fn html_for(url: &blitz_traits::net::Url) -> Option<String> {
    if url.as_str() == "chuzz:sandbox-probe" {
        return crate::sandbox::probe_html();
    }
    (url.as_str() == crate::tls_status::TLS_REFUSED_WHY_URL).then(tls_refused_explainer_html)
}

fn tls_refused_explainer_html() -> String {
    format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Why TLS was refused</title></head>
<body style="{INTERNAL_PAGE_STYLE};padding:2rem;font:16px system-ui,sans-serif;max-width:48rem">
<h1>Why TLS was refused</h1>
<p>chuzz offers ChaCha20-Poly1305 only in TLS 1.3 and ECDHE ChaCha20-Poly1305 in TLS 1.2. ECDSA, Ed25519 and RSA public signatures are verified. RSA key exchange, encryption and private-key operations are unavailable.</p>
<p>A TLS policy refusal ends the load. chuzz does not retry with weaker algorithms.</p>
<p>Certificate signatures, validity dates, host names and trust chains must verify. RSA-signed certificates are accepted for verification only.</p>
<p>Some servers instead reject the permitted TLS offer and send alert 40 handshake_failure to chuzz. Others send alert 71 insufficient_security. These server alerts do not establish whether the server requires RSA, AES, or another unsupported algorithm. The error page reports the observed reason and alert direction.</p>
<p>The site operator must enable ChaCha20-Poly1305, use ECDHE for TLS 1.2, and serve a valid certificate chain. A server that cannot negotiate this offer produces tls-chacha-required. There is no override.</p>
<p>Ask the site operator to provide that TLS configuration. chuzz cannot load the site until its TLS configuration satisfies this policy.</p>
</body></html>"#
    )
}
