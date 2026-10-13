use blitz_net::TlsPolicyError;
use blitz_traits::net::Url;

pub const TLS_REFUSED_WHY_URL: &str = "chuzz:tls-refused";

/// Borrow an ordinary navigation URL; unwrap source navigation only when needed.
pub(crate) fn navigation_url(url: &Url) -> std::borrow::Cow<'_, Url> {
    if url.scheme() == "view-source"
        && let Ok(inner) = Url::parse(url.path())
    {
        std::borrow::Cow::Owned(inner)
    } else {
        std::borrow::Cow::Borrowed(url)
    }
}

/// Report the alert's observed direction without inventing a server requirement.
pub fn alert_description(reason: TlsPolicyError) -> &'static str {
    match reason {
        TlsPolicyError::RsaCertificateChain | TlsPolicyError::CertificateKey => {
            "chuzz sent alert 43 unsupported_certificate to the server."
        }
        TlsPolicyError::PeerRejectedAlgorithms { alert: 40 } => {
            "The server sent alert 40 handshake_failure to chuzz."
        }
        TlsPolicyError::PeerRejectedAlgorithms { alert: 71 } => {
            "The server sent alert 71 insufficient_security to chuzz."
        }
        _ => "This refusal does not identify an alert number.",
    }
}

fn error_details(reason: Option<TlsPolicyError>) -> serde_json::Value {
    let Some(reason) = reason else {
        return serde_json::Value::Null;
    };
    let (kind, alert, direction): (&str, Option<u8>, Option<&str>) = match reason {
        TlsPolicyError::RsaCertificateChain => ("RsaCertificateChain", Some(43), Some("sent")),
        TlsPolicyError::CertificateKey => ("CertificateKey", Some(43), Some("sent")),
        TlsPolicyError::CertificateSignature => ("CertificateSignature", None, None),
        TlsPolicyError::HandshakeSignature => ("HandshakeSignature", None, None),
        TlsPolicyError::CipherSuite => ("CipherSuite", None, None),
        TlsPolicyError::PeerRejectedAlgorithms { alert } => {
            ("PeerRejectedAlgorithms", Some(alert), Some("received"))
        }
        _ => ("TlsPolicyError", None, None),
    };
    serde_json::json!({
        "kind": kind,
        "message": reason.to_string(),
        "alert": alert,
        "alert_direction": direction,
    })
}

/// The committed navigation reason is independent of later host connections.
pub fn inspection_state(url: &Url, policy_error: Option<TlsPolicyError>) -> serde_json::Value {
    let tls_url = navigation_url(url);
    serde_json::json!({
        "tls_policy_error": error_details(policy_error),
        "tls_explainer": policy_error.map(|_| TLS_REFUSED_WHY_URL),
        "connections": blitz_net::connection_stats()
            .into_iter()
            .map(|(host, stats)| {
                serde_json::json!({
                    "host": host,
                    "connections": stats.connections,
                    "refusals": stats.refusals,
                    "handshakes": stats.handshakes,
                    "last_tls_policy_error": error_details(stats.last_tls_policy_error),
                    "last_alpn": stats.last_alpn,
                })
            })
            .collect::<Vec<_>>(),
        "tls_host": if policy_error.is_some() || tls_url.scheme() == "https" {
            tls_url.host_str()
        } else {
            None
        },
        "url": url.as_str(),
    })
}
