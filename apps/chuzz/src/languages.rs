//! The languages Chuzz asks for, on the wire and inside the page.
//!
//! One list is `Accept-Language` on every HTTP request and
//! `navigator.language` / `navigator.languages`, including workers. The
//! default is `en-US`, then `en`. `CHUZZ_LANGUAGES` replaces it with a
//! comma-separated list of BCP 47 tags; entries that are not tags are
//! dropped. The window and the headless host are the same process, so they
//! read the same value.

use blitz_traits::net::http::HeaderValue;
use std::sync::{Arc, OnceLock};

const DEFAULT_TAGS: [&str; 2] = ["en-US", "en"];

/// Irregular and regular grandfathered tags from RFC 5646, in canonical case.
const GRANDFATHERED: &[&str] = &[
    "art-lojban",
    "cel-gaulish",
    "en-GB-oed",
    "i-ami",
    "i-bnn",
    "i-default",
    "i-enochian",
    "i-hak",
    "i-klingon",
    "i-lux",
    "i-mingo",
    "i-navajo",
    "i-pwn",
    "i-tao",
    "i-tay",
    "i-tsu",
    "no-bok",
    "no-nyn",
    "sgn-BE-FR",
    "sgn-BE-NL",
    "sgn-CH-DE",
    "zh-guoyu",
    "zh-hakka",
    "zh-min",
    "zh-min-nan",
    "zh-xiang",
];

struct Preference {
    tags: Arc<[String]>,
    header: HeaderValue,
}

fn preference() -> &'static Preference {
    static PREFERENCE: OnceLock<Preference> = OnceLock::new();
    PREFERENCE.get_or_init(|| {
        let tags = languages_from_env();
        let header = HeaderValue::from_str(&format_accept_language(&tags))
            .expect("Accept-Language is an ASCII header value");
        Preference {
            tags: tags.into(),
            header,
        }
    })
}

/// The tags `navigator.languages` exposes, in preference order.
pub fn preferred_languages() -> &'static [String] {
    preference().tags.as_ref()
}

/// The `Accept-Language` value for those tags.
///
/// The first tag is q=1 and is written without a q parameter. Each later tag
/// drops by 0.1, and nothing goes below 0.1.
pub fn accept_language_value() -> &'static str {
    preference()
        .header
        .to_str()
        .expect("Accept-Language is ASCII")
}

/// Attach [`accept_language_value`] when the caller did not set one.
pub fn insert_accept_language(headers: &mut blitz_traits::net::http::HeaderMap) {
    use blitz_traits::net::http::header::ACCEPT_LANGUAGE;
    if headers.contains_key(ACCEPT_LANGUAGE) {
        return;
    }
    headers.insert(ACCEPT_LANGUAGE, preference().header.clone());
}

fn languages_from_env() -> Vec<String> {
    let Ok(raw) = std::env::var("CHUZZ_LANGUAGES") else {
        return default_tags();
    };
    let mut tags = Vec::new();
    for entry in raw.split(',') {
        let Some(tag) = canonical_bcp47(entry.trim()) else {
            continue;
        };
        if tags.iter().any(|existing| existing == &tag) {
            continue;
        }
        tags.push(tag);
        if tags.len() == 32 {
            break;
        }
    }
    if tags.is_empty() {
        default_tags()
    } else {
        tags
    }
}

fn default_tags() -> Vec<String> {
    DEFAULT_TAGS.iter().map(|tag| (*tag).to_owned()).collect()
}

fn format_accept_language(tags: &[String]) -> String {
    let mut header = String::new();
    for (index, tag) in tags.iter().enumerate() {
        if index > 0 {
            header.push(',');
        }
        header.push_str(tag);
        if index > 0 {
            // index 1 -> 0.9, index 9 and beyond -> 0.1.
            let tenths = 10usize.saturating_sub(index).clamp(1, 9);
            header.push_str(";q=0.");
            header.push(char::from(b'0' + tenths as u8));
        }
    }
    header
}

/// Canonical BCP 47, or `None` when `tag` is not one.
fn canonical_bcp47(tag: &str) -> Option<String> {
    if tag.is_empty() || tag.len() > 100 || !tag.is_ascii() {
        return None;
    }
    if let Some(known) = GRANDFATHERED
        .iter()
        .copied()
        .find(|candidate| candidate.eq_ignore_ascii_case(tag))
    {
        return Some(known.to_owned());
    }
    if !tag
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        || tag.starts_with('-')
        || tag.ends_with('-')
        || tag.contains("--")
    {
        return None;
    }
    let parts: Vec<&str> = tag.split('-').collect();
    if parts.iter().any(|part| part.len() > 8) {
        return None;
    }
    if parts[0].eq_ignore_ascii_case("x") {
        return private_tag(&parts);
    }
    let primary = parts[0];
    if !is_alpha(primary) || !(2..=8).contains(&primary.len()) {
        return None;
    }
    let mut out = Vec::new();
    out.push(primary.to_ascii_lowercase());
    let mut index = 1;
    if (2..=3).contains(&primary.len()) {
        let mut extlangs = 0;
        while index < parts.len() && extlangs < 3 && is_alpha_len(parts[index], 3) {
            out.push(parts[index].to_ascii_lowercase());
            index += 1;
            extlangs += 1;
        }
    }
    if index < parts.len() && is_alpha_len(parts[index], 4) {
        out.push(title_case(parts[index]));
        index += 1;
    }
    if index < parts.len() && is_region(parts[index]) {
        out.push(canonical_region(parts[index]));
        index += 1;
    }
    let mut seen_variants = Vec::new();
    while index < parts.len() && is_variant(parts[index]) {
        let variant = parts[index].to_ascii_lowercase();
        if seen_variants.iter().any(|seen| seen == &variant) {
            return None;
        }
        seen_variants.push(variant.clone());
        out.push(variant);
        index += 1;
    }
    let mut seen_singletons = Vec::new();
    while index < parts.len() && is_singleton(parts[index]) {
        let singleton = parts[index].to_ascii_lowercase();
        if seen_singletons.iter().any(|seen| seen == &singleton) {
            return None;
        }
        seen_singletons.push(singleton.clone());
        index += 1;
        let start = index;
        while index < parts.len() && is_extension_part(parts[index]) {
            index += 1;
        }
        if index == start {
            return None;
        }
        out.push(singleton);
        for part in &parts[start..index] {
            out.push((*part).to_ascii_lowercase());
        }
    }
    if index < parts.len() {
        private_tag(&parts[index..])?;
        for part in &parts[index..] {
            out.push(part.to_ascii_lowercase());
        }
    }
    Some(out.join("-"))
}

fn private_tag(parts: &[&str]) -> Option<String> {
    if parts.len() < 2 || !parts[0].eq_ignore_ascii_case("x") {
        return None;
    }
    if parts[1..].iter().any(|part| {
        part.is_empty() || part.len() > 8 || !part.bytes().all(|byte| byte.is_ascii_alphanumeric())
    }) {
        return None;
    }
    Some(
        parts
            .iter()
            .map(|part| part.to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join("-"),
    )
}

fn is_alpha(part: &str) -> bool {
    !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_alphabetic())
}

fn is_alpha_len(part: &str, len: usize) -> bool {
    part.len() == len && is_alpha(part)
}

fn is_region(part: &str) -> bool {
    is_alpha_len(part, 2) || (part.len() == 3 && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn canonical_region(part: &str) -> String {
    if part.len() == 2 {
        part.to_ascii_uppercase()
    } else {
        part.to_owned()
    }
}

fn is_variant(part: &str) -> bool {
    let bytes = part.as_bytes();
    ((5..=8).contains(&bytes.len()) && bytes.iter().all(|byte| byte.is_ascii_alphanumeric()))
        || (bytes.len() == 4
            && bytes[0].is_ascii_digit()
            && bytes[1..].iter().all(|byte| byte.is_ascii_alphanumeric()))
}

fn is_singleton(part: &str) -> bool {
    let mut bytes = part.bytes();
    match (bytes.next(), bytes.next()) {
        (Some(byte), None) => byte.is_ascii_alphanumeric() && byte != b'x' && byte != b'X',
        _ => false,
    }
}

fn is_extension_part(part: &str) -> bool {
    (2..=8).contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn title_case(part: &str) -> String {
    let mut chars = part.chars();
    match chars.next() {
        Some(first) => {
            let mut titled = String::new();
            titled.push(first.to_ascii_uppercase());
            titled.extend(chars.map(|character| character.to_ascii_lowercase()));
            titled
        }
        None => String::new(),
    }
}
