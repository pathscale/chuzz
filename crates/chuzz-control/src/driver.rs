//! The deep-debugging plane: a screenshot of the window as it is painted.
//!
//! This is a different socket from [`crate::client`], and deliberately so.
//! That one is the browsing layer, reading the tree and clicking and typing, and it answers
//! questions about *what the document says*. This is the layer underneath, and
//! it answers the one question the other cannot: what the window actually put
//! on screen. A box with the right coordinates and a box that was painted are
//! not the same claim, and the difference is exactly where two bugs hid.
//!
//! The renderer exposes it as a loopback WebDriver server, published in a
//! `0600` descriptor carrying its address and a token. It exists only when the
//! browser is started with both:
//!
//! ```text
//! TAURI_BLITZ_DRIVER=127.0.0.1:0 \
//! TAURI_BLITZ_DRIVER_DESCRIPTOR=/tmp/chuzz-driver.json \
//! CHUZZ_CONTROL=1 target/release/chuzz-gui
//! ```
//!
//! Hand-rolled HTTP rather than a client crate. It is one GET and one POST to
//! a loopback port that answers `Connection: close`, and the alternative is
//! putting a TLS stack and an async runtime into a tool whose whole job is to
//! still work when the build is broken.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

/// A connection to a running browser's debug-control server.
pub struct Driver {
    address: String,
    session: String,
}

/// Where the driver descriptor is, if the environment names one.
pub fn descriptor_from_env() -> Option<PathBuf> {
    std::env::var_os("TAURI_BLITZ_DRIVER_DESCRIPTOR").map(PathBuf::from)
}

impl Driver {
    /// Read a descriptor, present its token, and hold the one session the
    /// server allows.
    pub fn connect(descriptor: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let descriptor: Value = serde_json::from_slice(&std::fs::read(descriptor)?)?;
        let address = descriptor["address"]
            .as_str()
            .ok_or("the driver descriptor has no address")?
            .to_owned();
        let token = descriptor["token"]
            .as_str()
            .ok_or("the driver descriptor has no token")?
            .to_owned();

        let response = request(
            &address,
            "POST",
            "/session",
            Some(json!({
                "capabilities": {"alwaysMatch": {"blitz:token": token}},
            })),
        )?;
        // The server allows one session at a time. A previous client that
        // exited without releasing its own is the usual reason this fails, so
        // say that rather than repeating the server's wording.
        let session = response["value"]["sessionId"]
            .as_str()
            .ok_or_else(|| {
                format!(
                    "could not open a driver session ({}). Another client may still hold \
                     the one the browser allows; restart the browser if nothing else is \
                     connected.",
                    response["value"]
                )
            })?
            .to_owned();
        Ok(Self { address, session })
    }

    /// The window, as PNG bytes.
    pub fn screenshot(&self) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let response = request(
            &self.address,
            "GET",
            &format!("/session/{}/screenshot", self.session),
            None,
        )?;
        let encoded = response["value"]
            .as_str()
            .ok_or_else(|| format!("no screenshot: {}", response["value"]))?;
        Ok(decode_base64(encoded)?)
    }

    /// Release the session so the next invocation can open one.
    ///
    /// The server allows exactly one at a time and keeps it after the socket
    /// closes, so a client that just exits leaves the browser refusing every
    /// later connection with "only one session is supported" until it is
    /// restarted. That turned a screenshot into a per-launch privilege.
    fn release(&self) {
        let _ = request(
            &self.address,
            "DELETE",
            &format!("/session/{}", self.session),
            None,
        );
    }

    /// The document's serialised source, as the renderer holds it.
    pub fn source(&self) -> Result<String, Box<dyn std::error::Error>> {
        let response = request(
            &self.address,
            "GET",
            &format!("/session/{}/source", self.session),
            None,
        )?;
        Ok(response["value"].as_str().unwrap_or_default().to_owned())
    }
}

fn request(
    address: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Result<Value, Box<dyn std::error::Error>> {
    let mut stream = TcpStream::connect(address)?;
    // A screenshot of a 1440x960 window is a megabyte or so of base64 and the
    // renderer has to paint before it answers, so this is generous rather than
    // snappy: a timeout here reads as "the browser is wedged", which is a much
    // worse thing to be told wrongly.
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;

    let encoded = body.map(|value| value.to_string()).unwrap_or_default();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{encoded}",
        encoded.len()
    )?;
    stream.flush()?;

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or("the driver answered without headers")?;
    Ok(serde_json::from_slice(&raw[split + 4..])?)
}

/// Standard base64, no padding assumptions beyond the usual `=`.
///
/// Written out rather than pulled in: this crate has three dependencies and a
/// decoder is twenty lines.
fn decode_base64(text: &str) -> Result<Vec<u8>, String> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut accumulator: u32 = 0;
    let mut bits = 0;
    for byte in text.bytes() {
        if byte == b'=' || byte.is_ascii_whitespace() {
            continue;
        }
        let value = ALPHABET
            .iter()
            .position(|candidate| *candidate == byte)
            .ok_or_else(|| format!("not base64: byte {byte:#04x}"))? as u32;
        accumulator = (accumulator << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((accumulator >> bits) as u8);
        }
    }
    Ok(out)
}

impl Drop for Driver {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_the_shapes_a_png_produces() {
        // The three residues, because the tail is where a hand-written decoder
        // goes wrong, and a truncated PNG decodes to a plausible image rather
        // than to an error.
        assert_eq!(decode_base64("").unwrap(), b"");
        assert_eq!(decode_base64("TQ==").unwrap(), b"M");
        assert_eq!(decode_base64("TWE=").unwrap(), b"Ma");
        assert_eq!(decode_base64("TWFu").unwrap(), b"Man");
        assert_eq!(
            decode_base64("iVBORw0KGgo=").unwrap(),
            [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a],
            "a PNG signature has to survive, because that is what this carries"
        );
        assert!(decode_base64("****").is_err());
    }
}
