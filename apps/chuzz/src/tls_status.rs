use blitz_traits::net::Url;

pub const RSA_FALLBACK_WHY_URL: &str = "chuzz:rsa-fallback";
pub const RSA_FALLBACK_BANNER: &str = "This site uses RSA. chuzz tried ECC-only first, then opened a separate connection allowing RSA.";

pub fn rsa_fallback_for(url: &Url) -> bool {
    url.scheme() == "https"
        && url
            .host_str()
            .is_some_and(|host| blitz_net::rsa_fallback_state(host) == Some(true))
}

/// Inspection metadata, separate from the site's DOM.
pub fn inspection_state(url: &Url) -> serde_json::Value {
    serde_json::json!({
        "tls_rsa_fallback": rsa_fallback_for(url),
        "connections": blitz_net::connection_stats()
            .into_iter()
            .map(|(host, stats)| {
                serde_json::json!({
                    "host": host,
                    "connections": stats.connections,
                    "refusals": stats.refusals,
                    "fallbacks": stats.fallbacks,
                })
            })
            .collect::<Vec<_>>(),
        "tls_host": if url.scheme() == "https" { url.host_str() } else { None },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_tls_urls_have_no_fallback() {
        for address in ["http://example.com/", "about:blank", RSA_FALLBACK_WHY_URL] {
            let url = Url::parse(address).expect("valid test URL");
            assert!(!rsa_fallback_for(&url));
            assert_eq!(inspection_state(&url)["tls_host"], serde_json::Value::Null);
        }
    }
}
