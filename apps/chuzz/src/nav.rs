//! Address-bar policy: deciding what a typed string means.
//!
//! The browser, not the engine, owns this. A string is a URL if it parses as
//! one, a bare hostname if it looks like a domain, and a search query
//! otherwise.

use blitz_traits::navigation::NavigationOptions;
use blitz_traits::net::{Method, Request, Url};

/// Page opened by the home button.
pub const HOME_URL: &str = "https://24x.ai/";

/// Page opened by a new tab: blank, so opening one costs nothing and shows
/// nothing until you ask for something.
pub const NEW_TAB_URL: &str = "about:blank";

/// Search endpoint used when the address bar holds something that is not a URL.
const SEARCH_URL: &str = "https://duckduckgo.com/";

/// Turn whatever the user typed into a request.
///
/// Returns `None` only when the input is empty or whitespace, which the
/// toolbar treats as "do nothing" rather than as a failed navigation.
pub fn request_from_input(input: &str) -> Option<Request> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }

    if let Ok(url) = Url::parse(input)
        && url.scheme() != "localhost"
    {
        return Some(Request::get(url));
    }

    if looks_like_hostname(input)
        && let Ok(url) = Url::parse(&format!("https://{input}"))
    {
        return Some(Request::get(url));
    }

    Some(search_request(input))
}

/// A dotted, space-free token is treated as a host rather than a query.
///
/// `localhost` and `localhost:3000` are special-cased because developers type
/// them constantly and they carry no dot.
fn looks_like_hostname(input: &str) -> bool {
    if input.contains(char::is_whitespace) {
        return false;
    }
    if input == "localhost" || input.starts_with("localhost:") || input.starts_with("localhost/") {
        return true;
    }
    let host = input
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(input)
        .split(':')
        .next()
        .unwrap_or(input);
    // A trailing dot ("foo.") or a leading dot (".foo") is a typo, not a host.
    host.contains('.') && !host.starts_with('.') && !host.ends_with('.')
}

fn search_request(query: &str) -> Request {
    // `Url::parse` of a constant literal cannot fail.
    #[allow(clippy::expect_used)]
    let mut url = Url::parse(SEARCH_URL).expect("search URL is a valid constant");
    url.query_pairs_mut().append_pair("q", query);

    NavigationOptions::new(
        url,
        Some(String::from("application/x-www-form-urlencoded")),
        0,
    )
    .set_method(Method::GET)
    .into_request()
}

/// Text shown in the tab strip and the window title.
pub fn display_title(title: &str, url: &Url) -> String {
    if title.trim().is_empty() {
        url.as_str().to_string()
    } else {
        title.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(input: &str) -> String {
        request_from_input(input).unwrap().url.to_string()
    }

    #[test]
    fn blank_input_is_not_a_navigation() {
        assert!(request_from_input("").is_none());
        assert!(request_from_input("   \t ").is_none());
    }

    #[test]
    fn an_explicit_scheme_is_preserved() {
        assert_eq!(target("http://example.com/a"), "http://example.com/a");
        assert_eq!(target("https://example.com/a"), "https://example.com/a");
    }

    #[test]
    fn a_bare_hostname_gets_https() {
        assert_eq!(target("example.com"), "https://example.com/");
        assert_eq!(
            target("example.com/path?q=1"),
            "https://example.com/path?q=1"
        );
    }

    #[test]
    fn localhost_is_treated_as_a_host_despite_having_no_dot() {
        assert_eq!(target("localhost:3000"), "https://localhost:3000/");
        assert_eq!(target("localhost"), "https://localhost/");
    }

    #[test]
    fn prose_becomes_a_search() {
        let searched = target("how tall is the eiffel tower");
        assert!(
            searched.starts_with("https://duckduckgo.com/?q="),
            "{searched}"
        );
        assert!(searched.contains("eiffel"), "{searched}");
    }

    #[test]
    fn a_dotted_phrase_with_spaces_is_a_search_not_a_host() {
        let searched = target("what is rust.lang about");
        assert!(
            searched.starts_with("https://duckduckgo.com/?q="),
            "{searched}"
        );
    }

    #[test]
    fn a_trailing_dot_is_a_typo_and_searches() {
        let searched = target("example.");
        assert!(
            searched.starts_with("https://duckduckgo.com/?q="),
            "{searched}"
        );
    }

    #[test]
    fn search_queries_are_percent_encoded() {
        let searched = target("rust & c++");
        assert!(!searched.contains(' '), "{searched}");
        assert!(searched.contains("%26"), "{searched}");
    }

    #[test]
    fn an_untitled_page_falls_back_to_its_url() {
        let url = Url::parse("https://example.com/a").unwrap();
        assert_eq!(display_title("", &url), "https://example.com/a");
        assert_eq!(display_title("   ", &url), "https://example.com/a");
        assert_eq!(display_title("Example", &url), "Example");
    }
}
