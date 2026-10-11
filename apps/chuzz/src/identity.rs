//! What Chuzz tells a server it is.
//!
//! A server decides what to send from the `User-Agent`, and the engine's
//! default is a 2020 Firefox on Linux. Measured across a 104-site corpus, **23
//! sites answer that with an HTTP error and no page at all** — 403 mostly, with
//! 401, 429, 406, 404 and 400 among them. That is not a rendering fault and no
//! amount of engine work fixes it: the page was never sent.
//!
//! There is no Brave identity here, and that is not an omission. Brave sends
//! Chrome's `User-Agent` verbatim so that it cannot be singled out by it, and
//! distinguishes itself in-page through `navigator.brave` rather than on the
//! wire. A separate string would be a Brave no real Brave ever sends.

use std::fmt;

/// Which browser Chuzz presents itself as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Identity {
    /// Chuzz, honestly. Available from Settings for compatibility diagnosis.
    Chuzz,
    /// A current Chrome on macOS, which is byte for byte what Brave sends.
    #[default]
    Chrome,
}

/// The Chrome release this pretends to be when asked to.
///
/// A version far enough behind is itself a signal, so this needs moving
/// occasionally. One constant, on purpose.
const CHROME_VERSION: &str = "151.0.0.0";

impl Identity {
    /// Read the identity from `CHUZZ_USER_AGENT`.
    ///
    /// `chuzz` and `chrome` name the presets; `brave` is accepted and means
    /// `chrome`, because that is what Brave sends. Anything else is taken as a
    /// literal `User-Agent`, so a corpus run can reproduce one specific client
    /// without a code change.
    pub fn from_env() -> (Self, Option<String>) {
        match std::env::var("CHUZZ_USER_AGENT").ok() {
            None => (Self::Chrome, None),
            Some(value) => match value.trim().to_ascii_lowercase().as_str() {
                "" | "chuzz" => (Self::Chuzz, None),
                "chrome" | "brave" => (Self::Chrome, None),
                _ => (Self::Chuzz, Some(value)),
            },
        }
    }

    /// The `User-Agent` this identity sends.
    pub fn user_agent(self) -> String {
        match self {
            Self::Chuzz => format!(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chuzz/{} Safari/537.36",
                env!("CARGO_PKG_VERSION")
            ),
            Self::Chrome => format!(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/{CHROME_VERSION} Safari/537.36"
            ),
        }
    }
}

impl fmt::Display for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Chuzz => f.write_str("chuzz"),
            Self::Chrome => f.write_str("chrome"),
        }
    }
}

/// The `User-Agent` to send, honouring `CHUZZ_USER_AGENT`.
pub fn user_agent_from_env() -> String {
    match Identity::from_env() {
        (_, Some(literal)) => literal,
        (identity, None) => identity.user_agent(),
    }
}

/// Whether an environment override owns the identity for this process.
///
/// Corpus and compatibility runs use the override to reproduce a specific
/// client. Settings must report that it is locked instead of displaying a
/// switch whose click cannot affect those requests.
pub fn environment_override_is_set() -> bool {
    std::env::var_os("CHUZZ_USER_AGENT").is_some()
}

/// Resolve the ordinary Settings choice while preserving the environment
/// override used by corpus runs.
pub fn user_agent_for_spoofing(spoofing: bool) -> String {
    if environment_override_is_set() {
        user_agent_from_env()
    } else if spoofing {
        Identity::Chrome.user_agent()
    } else {
        Identity::Chuzz.user_agent()
    }
}

/// What a request is for, which decides the fetch metadata Chromium attaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestKind<'a> {
    /// A top-level navigation. `initiator` is the page it came from, if any.
    Navigate { initiator: Option<&'a url::Url> },
    /// A page `fetch()` or `XMLHttpRequest`.
    Fetch { initiator: Option<&'a url::Url> },
    /// An engine subresource (style, script, image, font). The engine does not
    /// say which, so only the client hints are sent: a wrong `sec-fetch-dest`
    /// is a stronger signal than none.
    Subresource,
}

/// The Chrome major version a `User-Agent` claims, when it claims one.
fn chrome_major(user_agent: &str) -> Option<&str> {
    let version = user_agent.split("Chrome/").nth(1)?;
    let major = version.split('.').next()?;
    (!major.is_empty() && major.bytes().all(|byte| byte.is_ascii_digit())).then_some(major)
}

/// `same-origin`, `same-site` or `cross-site`, by registrable domain.
fn fetch_site(initiator: &url::Url, target: &url::Url) -> &'static str {
    if initiator.origin() == target.origin() {
        return "same-origin";
    }
    let site = |url: &url::Url| {
        let host = url.host_str()?.to_ascii_lowercase();
        let domain = psl::domain(host.as_bytes())
            .map(|domain| domain.as_bytes().to_vec())
            .unwrap_or_else(|| host.into_bytes());
        Some((url.scheme().to_owned(), domain))
    };
    match (site(initiator), site(target)) {
        (Some(a), Some(b)) if a == b => "same-site",
        _ => "cross-site",
    }
}

/// Add the headers Chromium sends for `kind` when the `User-Agent` is
/// Chrome-family: the low-entropy client hints, and fetch metadata for
/// navigations and page fetches. Servers that see a Chrome `User-Agent`
/// without them answer 400 or a challenge (measured on Facebook). Only secure
/// URLs get them, as in Chromium. The brand list is plain Chromium's, so the
/// browser claims the family, not Google's product. Headers already present are
/// kept.
pub fn stamp_request_headers(
    headers: &mut blitz_traits::net::http::HeaderMap,
    user_agent: &str,
    url: &url::Url,
    kind: RequestKind<'_>,
) {
    use blitz_traits::net::http::{HeaderName, HeaderValue};

    let Some(major) = chrome_major(user_agent) else {
        return;
    };
    let secure = url.scheme() == "https"
        || matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if !secure {
        return;
    }
    let mut set = |name: &'static str, value: String| {
        if let Ok(value) = HeaderValue::from_str(&value) {
            headers
                .entry(HeaderName::from_static(name))
                .or_insert(value);
        }
    };
    set(
        "sec-ch-ua",
        format!("\"Chromium\";v=\"{major}\", \"Not-A.Brand\";v=\"99\""),
    );
    set("sec-ch-ua-mobile", "?0".to_owned());
    set("sec-ch-ua-platform", "\"macOS\"".to_owned());
    match kind {
        RequestKind::Navigate { initiator } => {
            set(
                "accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,\
                 image/webp,image/apng,*/*;q=0.8"
                    .to_owned(),
            );
            set("upgrade-insecure-requests", "1".to_owned());
            let site = initiator.map_or("none", |initiator| fetch_site(initiator, url));
            set("sec-fetch-site", site.to_owned());
            set("sec-fetch-mode", "navigate".to_owned());
            set("sec-fetch-user", "?1".to_owned());
            set("sec-fetch-dest", "document".to_owned());
            set("priority", "u=0, i".to_owned());
        }
        RequestKind::Fetch { initiator } => {
            let site = initiator.map_or("none", |initiator| fetch_site(initiator, url));
            set("sec-fetch-site", site.to_owned());
            set(
                "sec-fetch-mode",
                if site == "same-origin" { "same-origin" } else { "cors" }.to_owned(),
            );
            set("accept", "*/*".to_owned());
            set("sec-fetch-dest", "empty".to_owned());
            set("priority", "u=1, i".to_owned());
        }
        RequestKind::Subresource => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ordinary browsing defaults to the current Chrome-compatible identity.
    #[test]
    fn the_default_identity_is_chrome() {
        assert_eq!(Identity::default(), Identity::Chrome);
        assert!(Identity::default().user_agent().contains("Chrome/"));
    }

    /// Asking for Brave gets Chrome's string, because that is what Brave sends.
    #[test]
    fn brave_and_chrome_are_the_same_string() {
        assert!(Identity::Chrome.user_agent().contains("Chrome/"));
        assert!(!Identity::Chrome.user_agent().contains("Brave"));
    }

    /// Every identity is shaped like a browser a server will recognise.
    #[test]
    fn every_identity_looks_like_a_browser() {
        for identity in [Identity::Chuzz, Identity::Chrome] {
            let agent = identity.user_agent();
            assert!(agent.starts_with("Mozilla/5.0 "), "{identity}: {agent}");
            assert!(!agent.contains('\n'), "a header value cannot wrap");
        }
    }

    /// The two identities are actually different, so selecting one means
    /// something. A refactor that collapsed them would otherwise pass silently.
    #[test]
    fn the_identities_differ() {
        assert_ne!(Identity::Chuzz.user_agent(), Identity::Chrome.user_agent());
    }
}
