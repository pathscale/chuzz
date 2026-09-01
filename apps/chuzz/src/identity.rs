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
    /// Chuzz, honestly. The default, because claiming to be another browser is
    /// a decision a person should make rather than inherit.
    #[default]
    Chuzz,
    /// A current Chrome on macOS, which is byte for byte what Brave sends.
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
            None => (Self::Chuzz, None),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The default says Chuzz rather than borrowing another browser's name.
    #[test]
    fn the_default_identity_is_chuzz() {
        assert_eq!(Identity::default(), Identity::Chuzz);
        assert!(Identity::Chuzz.user_agent().contains("Chuzz/"));
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
