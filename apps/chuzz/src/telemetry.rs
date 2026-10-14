//! Small, explicit block list for Amazon client telemetry endpoints.
//!
//! `unagi.amazon.*` is Amazon's regional Unagi telemetry family. The FLS
//! endpoints below are the North America, Europe, and Far East collectors.
//! These hosts are blocked only for page subresources and script network
//! requests. Top-level navigations deliberately do not consult this policy.

use blitz_traits::net::Url;

const FLS_HOSTS: &[&str] = &[
    "fls-na.amazon.com",
    "fls-eu.amazon.com",
    "fls-fe.amazon.com",
];

pub(crate) fn blocked_host(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host.starts_with("unagi.amazon.") || FLS_HOSTS.contains(&host.as_str())
}

pub(crate) fn blocked_url(url: &Url) -> bool {
    url.host_str().is_some_and(blocked_host)
}

pub(crate) fn diagnostic(url: &Url) -> String {
    format!(
        "blocked Amazon telemetry request to {}",
        url.host_str().unwrap_or_default()
    )
}
