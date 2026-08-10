//! Decompressing response bodies.
//!
//! `blitz-net` builds its `reqwest` client without any compression features,
//! so a server that answers `content-encoding: br` or `gzip` hands back raw
//! compressed bytes. Feeding those to the HTML parser or to the script engine
//! produces either an empty page or a syntax error on the first byte, which is
//! indistinguishable from a broken site.
//!
//! The encoding header is not visible through the net provider's API, so the
//! payload is sniffed instead: both formats have recognisable openings, and
//! anything that already decodes as UTF-8 text is left alone.
//!
//! This is a workaround, and the real fix is upstream. `blitz-net` declares
//! `reqwest` with `default-features = false` and the features `charset`,
//! `native-tls` and `form` (see `packages/blitz-net/Cargo.toml`). reqwest only
//! decompresses transparently when its `brotli`/`gzip`/`deflate`/`zstd`
//! features are enabled, so adding those there would make every response
//! arrive decoded and this module unnecessary. Sniffing is a heuristic: a
//! served file that happens to be valid brotli and not valid UTF-8 would be
//! decompressed when it should not be.

use std::io::Read;

/// Decode a response body to text, decompressing it first if it needs it.
pub fn decode_body(bytes: &[u8]) -> String {
    if let Some(text) = decompress(bytes) {
        return text;
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// Returns the decompressed text, or `None` when the bytes are not compressed
/// or cannot be decoded.
fn decompress(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() || looks_like_text(bytes) {
        return None;
    }
    if is_gzip(bytes) {
        return gunzip(bytes);
    }
    // Brotli has no magic number, so it is the fallback rather than a guess:
    // only attempted once the payload is known not to be text.
    unbrotli(bytes)
}

/// A gzip member always opens with 0x1f 0x8b and the deflate method.
fn is_gzip(bytes: &[u8]) -> bool {
    bytes.len() > 2 && bytes[0] == 0x1f && bytes[1] == 0x8b
}

/// Text that already decodes cleanly is never compressed data worth probing.
///
/// Only the opening bytes are checked, so a large body costs nothing here, and
/// a truncated multi-byte character at the boundary is not mistaken for binary.
fn looks_like_text(bytes: &[u8]) -> bool {
    let window = &bytes[..bytes.len().min(512)];
    match std::str::from_utf8(window) {
        Ok(text) => !text.contains('\u{0}'),
        // An error at the very end is a split character, not binary.
        Err(error) => error.valid_up_to() >= window.len().saturating_sub(4),
    }
}

fn gunzip(bytes: &[u8]) -> Option<String> {
    let mut decoded = String::new();
    flate2::read::GzDecoder::new(bytes)
        .read_to_string(&mut decoded)
        .ok()
        .map(|_| decoded)
}

fn unbrotli(bytes: &[u8]) -> Option<String> {
    let mut decoded = String::new();
    brotli::Decompressor::new(bytes, 4096)
        .read_to_string(&mut decoded)
        .ok()
        .filter(|_| !decoded.is_empty())
        .map(|_| decoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn plain_text_passes_through_untouched() {
        assert_eq!(
            decode_body(b"<!doctype html><p>hi</p>"),
            "<!doctype html><p>hi</p>"
        );
    }

    #[test]
    fn an_empty_body_decodes_to_nothing() {
        assert_eq!(decode_body(b""), "");
    }

    #[test]
    fn a_gzip_body_is_decompressed() {
        let source = "<!doctype html><title>compressed</title>";
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(source.as_bytes()).unwrap();
        let compressed = encoder.finish().unwrap();

        assert_ne!(compressed.as_slice(), source.as_bytes());
        assert_eq!(decode_body(&compressed), source);
    }

    #[test]
    fn a_brotli_body_is_decompressed() {
        let source = "export const value = 1; // a script, brotli encoded";
        let mut compressed = Vec::new();
        {
            let mut writer = brotli::CompressorWriter::new(&mut compressed, 4096, 9, 22);
            writer.write_all(source.as_bytes()).unwrap();
        }

        assert_ne!(compressed.as_slice(), source.as_bytes());
        assert_eq!(decode_body(&compressed), source);
    }

    #[test]
    fn utf8_text_is_not_mistaken_for_compressed_data() {
        let source = "<p>caf\u{e9} \u{2014} na\u{ef}ve \u{1f600}</p>";
        assert_eq!(decode_body(source.as_bytes()), source);
    }

    #[test]
    fn undecodable_bytes_fall_back_to_lossy_text_rather_than_panicking() {
        // Not gzip, not valid brotli, not UTF-8: still has to return something.
        let garbage = [0xff_u8, 0xfe, 0x00, 0x01, 0x02, 0x03];
        let _ = decode_body(&garbage);
    }
}
