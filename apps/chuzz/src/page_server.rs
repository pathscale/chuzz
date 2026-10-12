//! A loopback origin for a built page, so a directory can be served as a site.
//!
//! # Why a directory is not enough
//!
//! A built single-page application is not a file, it is a site. Its markup
//! references `/static/js/app.mjs`, absolutely, and its router asks the history
//! API for `/settings` and expects the same document back. Opened as
//! `file:///.../index.html` that first reference resolves to the filesystem
//! root, so the bundle is never fetched and the page renders as an empty mount
//! point with one line in the log. Measured on support.cafe's dist, which is
//! two absolute references and nothing else.
//!
//! An origin also decides things a page can observe. `localStorage` is keyed by
//! origin and `file://` has an opaque one, so storage a page writes at startup
//! and reads back is not obviously the same storage.
//!
//! So a directory gets an origin: `127.0.0.1` on a port the kernel picks, for
//! as long as the host runs. A file or a URL is taken as given, because a
//! caller naming one of those has already decided how the page is reached.
//!
//! # What it is not
//!
//! Not a dev server, and deliberately not a general one. It serves `GET` and
//! `HEAD` for files under one directory, answers an extension-less path with
//! `index.html` so client routing works, and refuses everything else. It binds
//! loopback, so it is reachable only from this machine, and it is gone when the
//! process is.

use std::future::poll_fn;
use std::io::{self, Read as _, Write as _};
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll, ready};

use bytes::Bytes;
use http::{Method, Response, StatusCode, header};
use nago_rustls::rustls::{
    ServerConfig, ServerConnection,
    pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
};
use nagoya::net::{TcpListener, TcpStream};
use nagoya::reactor::{Addr, Reactor};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// The most a request head may be before it is refused.
///
/// A client that never sends the blank line would otherwise be read from until
/// this process runs out of memory.
const MAX_HEAD_BYTES: usize = 16 * 1024;

const TLS_READ_CHUNK: usize = 16 * 1024;

/// Start serving `root` on loopback and return its origin.
///
/// The listener is bound before this returns, so the caller can hand the URL
/// straight to the loader without racing it.
pub async fn start(root: &Path) -> Result<String, String> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("could not resolve {}: {error}", root.display()))?;
    let tls_config = page_server_tls_config()?;
    let reactor = reactor()?;
    let handle = reactor.handle();
    let listener = TcpListener::bind(Addr::localhost(0), &handle)
        .map_err(|error| format!("could not bind a loopback port: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| format!("could not read the bound port: {error}"))?
        .port();

    let secure = tls_config.is_some();
    drop(nagoya::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let root = root.clone();
            let tls_config = tls_config.clone();
            drop(nagoya::spawn(async move {
                let result = match tls_config {
                    Some(config) => answer_tls(stream, &root, config).await,
                    None => answer(stream, &root).await,
                };
                if let Err(error) = result {
                    eprintln!("chuzz-headless: page server: {error}");
                }
            }));
        }
    }));

    let scheme = if secure { "https" } else { "http" };
    Ok(format!("{scheme}://127.0.0.1:{port}/"))
}

fn page_server_tls_config() -> Result<Option<Arc<ServerConfig>>, String> {
    let certificate_path = std::env::var_os("CHUZZ_PAGE_SERVER_TLS_CERT").map(PathBuf::from);
    let key_path = std::env::var_os("CHUZZ_PAGE_SERVER_TLS_KEY").map(PathBuf::from);

    let (Some(certificate_path), Some(key_path)) = (certificate_path, key_path) else {
        return match (
            std::env::var_os("CHUZZ_PAGE_SERVER_TLS_CERT"),
            std::env::var_os("CHUZZ_PAGE_SERVER_TLS_KEY"),
        ) {
            (None, None) => Ok(None),
            _ => Err(
                "CHUZZ_PAGE_SERVER_TLS_CERT and CHUZZ_PAGE_SERVER_TLS_KEY must be set together"
                    .to_owned(),
            ),
        };
    };

    let certificates = CertificateDer::pem_file_iter(&certificate_path)
        .map_err(|error| {
            format!(
                "could not read TLS certificate chain {}: {error}",
                certificate_path.display()
            )
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            format!(
                "could not parse TLS certificate chain {}: {error}",
                certificate_path.display()
            )
        })?;
    if certificates.is_empty() {
        return Err(format!(
            "TLS certificate chain {} contains no certificates",
            certificate_path.display()
        ));
    }

    let key = PrivateKeyDer::from_pem_file(&key_path).map_err(|error| {
        format!(
            "could not read TLS private key {}: {error}",
            key_path.display()
        )
    })?;
    let mut config = ServerConfig::builder_with_provider(Arc::new(ps_rustcrypto::provider()))
        .with_safe_default_protocol_versions()
        .map_err(|error| format!("could not configure TLS protocol versions: {error}"))?
        .with_no_client_auth()
        .with_single_cert(certificates, key)
        .map_err(|error| format!("could not configure the page server certificate: {error}"))?;
    config.alpn_protocols = vec![b"h2".to_vec()];
    Ok(Some(Arc::new(config)))
}

fn reactor() -> Result<&'static Reactor, String> {
    static REACTOR: OnceLock<Result<Reactor, String>> = OnceLock::new();

    match REACTOR.get_or_init(|| {
        Reactor::start()
            .map_err(|error| format!("could not start the page server reactor: {error}"))
    }) {
        Ok(reactor) => Ok(reactor),
        Err(error) => Err(error.clone()),
    }
}

async fn answer_tls(
    stream: TcpStream,
    root: &Path,
    config: Arc<ServerConfig>,
) -> Result<(), String> {
    let connection =
        ServerConnection::new(config).map_err(|error| format!("could not start TLS: {error}"))?;
    let mut stream = TlsIo::new(stream, Box::new(connection));
    stream
        .handshake()
        .await
        .map_err(|error| format!("TLS handshake failed: {error}"))?;
    if stream.tls.alpn_protocol() != Some(b"h2".as_slice()) {
        return Err("TLS connection did not negotiate HTTP/2".to_owned());
    }

    let mut connection = h2::server::handshake(stream)
        .await
        .map_err(|error| format!("HTTP/2 handshake failed: {error}"))?;
    while let Some(accepted) = connection.accept().await {
        let (request, response) =
            accepted.map_err(|error| format!("accepting an HTTP/2 request: {error}"))?;
        let root = root.to_path_buf();
        drop(nagoya::spawn(async move {
            if let Err(error) = answer_h2(request, response, &root).await {
                eprintln!("chuzz-headless: page server: {error}");
            }
        }));
    }
    Ok(())
}

async fn answer_h2(
    request: http::Request<h2::RecvStream>,
    mut response: h2::server::SendResponse<Bytes>,
    root: &Path,
) -> Result<(), String> {
    let (parts, _request_body) = request.into_parts();
    let target = parts
        .uri
        .path_and_query()
        .map(|target| target.as_str())
        .unwrap_or("/");
    let head_only = parts.method == Method::HEAD;
    let supported_method = head_only || parts.method == Method::GET;

    let (status, response_type, body) = if !supported_method {
        (405, "text/plain", b"method not allowed".to_vec())
    } else {
        match resolve(root, target) {
            Some(path) => match std::fs::read(&path) {
                Ok(body) => {
                    let response_type = content_type(&path);
                    (200, response_type, decoded(response_type, body))
                }
                Err(error) => {
                    let message = format!("could not read {}: {error}", path.display());
                    (500, "text/plain", message.into_bytes())
                }
            },
            None => (404, "text/plain", b"not found".to_vec()),
        }
    };

    let end_stream = head_only || body.is_empty();
    let response_head = Response::builder()
        .status(
            StatusCode::from_u16(status)
                .map_err(|error| format!("invalid HTTP response status: {error}"))?,
        )
        .header(header::CONTENT_TYPE, response_type)
        .header(header::CONTENT_LENGTH, body.len().to_string())
        .header(header::CACHE_CONTROL, "no-store")
        .body(())
        .map_err(|error| format!("building the HTTP/2 response: {error}"))?;
    let send_stream = response
        .send_response(response_head, end_stream)
        .map_err(|error| format!("sending the HTTP/2 response head: {error}"))?;
    if !end_stream {
        send_h2_body(send_stream, body).await?;
    }
    Ok(())
}

async fn send_h2_body(mut stream: h2::SendStream<Bytes>, body: Vec<u8>) -> Result<(), String> {
    let mut sent = 0;
    while sent < body.len() {
        let wanted = (body.len() - sent).min(TLS_READ_CHUNK);
        stream.reserve_capacity(wanted);
        let capacity = poll_fn(|cx| {
            if stream.capacity() > 0 {
                Poll::Ready(Some(Ok(stream.capacity())))
            } else {
                stream.poll_capacity(cx)
            }
        })
        .await
        .ok_or_else(|| "HTTP/2 response stream closed".to_owned())?
        .map_err(|error| format!("waiting for HTTP/2 response capacity: {error}"))?;
        let count = capacity.min(wanted);
        if count == 0 {
            continue;
        }
        sent += count;
        let data = Bytes::copy_from_slice(&body[sent - count..sent]);
        stream
            .send_data(data, sent == body.len())
            .map_err(|error| format!("sending the HTTP/2 response body: {error}"))?;
    }
    Ok(())
}

/// Rustls uses synchronous record processing, while h2 requires Tokio I/O
/// traits. This adapter lets h2 use nagoya readiness without a Tokio runtime.
struct TlsIo {
    tcp: TcpStream,
    tls: Box<ServerConnection>,
    out: Vec<u8>,
    out_sent: usize,
    close_sent: bool,
    cipher: Vec<u8>,
    cipher_taken: usize,
}

impl TlsIo {
    fn new(tcp: TcpStream, tls: Box<ServerConnection>) -> Self {
        Self {
            tcp,
            tls,
            out: Vec::new(),
            out_sent: 0,
            close_sent: false,
            cipher: Vec::new(),
            cipher_taken: 0,
        }
    }

    async fn handshake(&mut self) -> io::Result<()> {
        poll_fn(|cx| self.poll_handshake(cx)).await
    }

    fn poll_handshake(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        loop {
            if self.tls.wants_write() {
                ready!(self.poll_drain(cx))?;
            }
            if !self.tls.is_handshaking() {
                return Poll::Ready(Ok(()));
            }
            if self.cipher_taken == self.cipher.len() {
                self.cipher.resize(TLS_READ_CHUNK, 0);
                self.cipher_taken = 0;
                let read = match self.tcp.poll_read(cx, &mut self.cipher) {
                    Poll::Ready(Ok(read)) => read,
                    Poll::Ready(Err(error)) => {
                        self.cipher.clear();
                        return Poll::Ready(Err(io_error(error)));
                    }
                    Poll::Pending => {
                        self.cipher.clear();
                        return Poll::Pending;
                    }
                };
                self.cipher.truncate(read);
                if read == 0 {
                    return Poll::Ready(Err(io::ErrorKind::UnexpectedEof.into()));
                }
            }

            let mut input = &self.cipher[self.cipher_taken..];
            let read = self.tls.read_tls(&mut input)?;
            if read == 0 {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "rustls consumed no TLS input",
                )));
            }
            self.cipher_taken += read;
            if let Err(error) = self.tls.process_new_packets() {
                let _ = self.poll_drain(cx);
                return Poll::Ready(Err(io::Error::new(io::ErrorKind::InvalidData, error)));
            }
        }
    }

    fn poll_drain(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        loop {
            if self.out_sent < self.out.len() {
                match ready!(self.tcp.poll_write(cx, &self.out[self.out_sent..])) {
                    Ok(0) => return Poll::Ready(Err(io::ErrorKind::WriteZero.into())),
                    Ok(sent) => self.out_sent += sent,
                    Err(error) => return Poll::Ready(Err(io_error(error))),
                }
            } else if self.tls.wants_write() {
                self.out.clear();
                self.out_sent = 0;
                self.tls.write_tls(&mut self.out)?;
            } else {
                return Poll::Ready(Ok(()));
            }
        }
    }
}

fn io_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

impl AsyncRead for TlsIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            match this.tls.reader().read(buf.initialize_unfilled()) {
                Ok(read) => {
                    buf.advance(read);
                    return Poll::Ready(Ok(()));
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Poll::Ready(Err(error)),
            }

            if let Poll::Ready(Err(error)) = this.poll_drain(cx) {
                return Poll::Ready(Err(error));
            }
            if this.cipher_taken == this.cipher.len() {
                this.cipher.resize(TLS_READ_CHUNK, 0);
                this.cipher_taken = 0;
                let read = match this.tcp.poll_read(cx, &mut this.cipher) {
                    Poll::Ready(Ok(read)) => read,
                    Poll::Ready(Err(error)) => {
                        this.cipher.clear();
                        return Poll::Ready(Err(io_error(error)));
                    }
                    Poll::Pending => {
                        this.cipher.clear();
                        return Poll::Pending;
                    }
                };
                this.cipher.truncate(read);
                if read == 0 {
                    return Poll::Ready(Ok(()));
                }
            }

            let mut input = &this.cipher[this.cipher_taken..];
            let read = match this.tls.read_tls(&mut input) {
                Ok(read) => read,
                Err(error) => return Poll::Ready(Err(error)),
            };
            this.cipher_taken += read;
            if let Err(error) = this.tls.process_new_packets() {
                let _ = this.poll_drain(cx);
                return Poll::Ready(Err(io::Error::new(io::ErrorKind::InvalidData, error)));
            }
        }
    }
}

impl AsyncWrite for TlsIo {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        loop {
            let written = this.tls.writer().write(data)?;
            if written > 0 || data.is_empty() {
                if let Poll::Ready(Err(error)) = this.poll_drain(cx) {
                    return Poll::Ready(Err(error));
                }
                return Poll::Ready(Ok(written));
            }
            ready!(this.poll_drain(cx))?;
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        this.tls.writer().flush()?;
        this.poll_drain(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if !this.close_sent {
            this.tls.send_close_notify();
            this.close_sent = true;
        }
        this.poll_drain(cx)
    }
}

async fn answer(mut stream: TcpStream, root: &Path) -> Result<(), String> {
    let head = read_head(&mut stream).await?;
    let Some(line) = head.lines().next() else {
        return respond(&mut stream, 400, "text/plain", b"no request line", false).await;
    };
    let mut parts = line.split(' ');
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    if !matches!(method, "GET" | "HEAD") {
        return respond(&mut stream, 405, "text/plain", b"method not allowed", false).await;
    }

    match resolve(root, target) {
        Some(path) => match std::fs::read(&path) {
            Ok(body) => {
                let body = decoded(content_type(&path), body);
                respond(
                    &mut stream,
                    200,
                    content_type(&path),
                    &body,
                    method == "HEAD",
                )
                .await
            }
            Err(error) => {
                let message = format!("could not read {}: {error}", path.display());
                respond(&mut stream, 500, "text/plain", message.as_bytes(), false).await
            }
        },
        None => respond(&mut stream, 404, "text/plain", b"not found", false).await,
    }
}

async fn read_head(stream: &mut TcpStream) -> Result<String, String> {
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    while !head.ends_with(b"\r\n\r\n") && !head.ends_with(b"\n\n") {
        if head.len() >= MAX_HEAD_BYTES {
            return Err("request head is too long".to_owned());
        }
        match stream.read(&mut byte).await {
            Ok(0) => break,
            Ok(_) => head.push(byte[0]),
            Err(error) => return Err(format!("reading the request: {error}")),
        }
    }
    String::from_utf8(head).map_err(|_| "the request head is not UTF-8".to_owned())
}

/// Which file, if any, answers `target`.
///
/// An extension-less path is the client router's, and gets `index.html` so that
/// `/settings` is the application rather than a 404. A path with an extension
/// that is not there is a missing asset and is reported as one: answering it
/// with the document instead is how a broken bundle reference turns into a
/// page that parses HTML as JavaScript.
fn resolve(root: &Path, target: &str) -> Option<PathBuf> {
    let path = target.split(['?', '#']).next().unwrap_or(target);
    let relative = Path::new(path.trim_start_matches('/'));
    // Nothing that climbs, and nothing absolute: a request is a name under the
    // root, not a way to name the rest of the disk.
    if relative.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return None;
    }

    let index = root.join("index.html");
    if relative.as_os_str().is_empty() {
        return index.is_file().then_some(index);
    }

    let candidate = root.join(relative);
    if candidate.is_file() {
        // Canonicalised and checked, because a symlink inside the directory can
        // still point out of it.
        let resolved = candidate.canonicalize().ok()?;
        return resolved.starts_with(root).then_some(resolved);
    }
    if candidate.is_dir() {
        let nested = candidate.join("index.html");
        return nested.is_file().then_some(nested);
    }
    if relative.extension().is_none() {
        return index.is_file().then_some(index);
    }
    None
}

/// Enough of a type table for a built page.
///
/// A wrong type is not cosmetic: a module served as `text/plain` is refused by
/// the script fetcher, and a stylesheet served as anything but `text/css` is
/// parsed as nothing, which reads as a page with no styles rather than a page
/// with a mistyped response.
/// Serve a compressed asset as what it decompresses to.
///
/// A pathscale build ships its bundle brotli-encoded under `.mjs` and `.mcss`
/// and lets the CDN answer `content-encoding: br`. This server does not, so a
/// page that reads its own bundle through `fetch` and hands the bytes to a
/// blob URL gets brotli where it expects JavaScript, and mounts nothing. That
/// is how nofilter.io rendered 20 anonymous nodes here while rendering fine in
/// a browser, and while its own checks were quietly measuring the copy the
/// public CDN answered with rather than the build under test.
///
/// Declaring the encoding instead would leave the decoding to whoever asked,
/// and a page `fetch` that never negotiated `accept-encoding` does not decode
/// what it did not ask for. Decoding here is decided by this process.
///
/// Only text payloads are considered, and `decode_body` leaves anything that
/// is already text alone, so an image or a font passes through untouched.
fn decoded(content_type: &str, body: Vec<u8>) -> Vec<u8> {
    let is_text = content_type.starts_with("text/")
        || content_type.starts_with("application/json")
        || content_type.contains("javascript");
    if !is_text {
        return body;
    }
    match crate::decode::decode_body_if_compressed(&body) {
        Some(text) => text.into_bytes(),
        None => body,
    }
}

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
    {
        "html" | "htm" => "text/html; charset=utf-8",
        // A stylesheet is served as one whatever the build named the file.
        // `.mcss` is what support.cafe emits; four sites in the fleet emit
        // `.scss`, and served as anything else the engine ignores them and
        // the page renders with no styling at all -- which still passes a
        // check that only asks whether a control exists.
        "css" | "mcss" | "scss" | "sass" | "less" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "wasm" => "application/wasm",
        "txt" => "text/plain; charset=utf-8",
        "xml" => "application/xml",
        _ => "application/octet-stream",
    }
}

async fn respond(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
    head_only: bool,
) -> Result<(), String> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Internal Server Error",
    };
    // `Connection: close` rather than keep-alive: one exchange per connection
    // is all a page load needs from a server that exists for the length of one
    // process, and returning from `answer` drops the stream after the exchange.
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(head.as_bytes())
        .await
        .map_err(|error| format!("writing the response head: {error}"))?;
    if !head_only {
        stream
            .write_all(body)
            .await
            .map_err(|error| format!("writing the response body: {error}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{content_type, decoded, resolve};
    use std::io::Write;

    fn fixture() -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "chuzz-page-server-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock is after the epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("static/js")).expect("create fixture tree");
        std::fs::write(root.join("index.html"), "<html></html>").expect("write index");
        std::fs::write(root.join("static/js/app.mjs"), "export {}").expect("write bundle");
        root.canonicalize().expect("canonicalise fixture")
    }

    /// The reference that made this module necessary. Under `file://` it
    /// resolves to the filesystem root and the bundle is never fetched.
    #[test]
    fn an_absolute_asset_path_resolves_under_the_root() {
        let root = fixture();
        assert_eq!(
            resolve(&root, "/static/js/app.mjs?v=1.0.0"),
            Some(root.join("static/js/app.mjs"))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// A client router owns paths that were never built as files.
    #[test]
    fn a_route_falls_back_to_the_document() {
        let root = fixture();
        assert_eq!(resolve(&root, "/settings"), Some(root.join("index.html")));
        assert_eq!(resolve(&root, "/"), Some(root.join("index.html")));
        let _ = std::fs::remove_dir_all(root);
    }

    /// A missing asset is a missing asset. Falling back to the document for
    /// anything with an extension hands a bundle request the HTML, and the
    /// script engine reports a syntax error in a file that was never a script.
    #[test]
    fn a_missing_asset_is_not_answered_with_the_document() {
        let root = fixture();
        assert_eq!(resolve(&root, "/static/js/missing.mjs"), None);
        let _ = std::fs::remove_dir_all(root);
    }

    /// A module served as anything but JavaScript is refused by the fetcher,
    /// which reads as a bundle that will not parse rather than one mislabelled.
    #[test]
    fn a_module_is_typed_as_javascript() {
        assert_eq!(
            content_type(std::path::Path::new("app.mjs")),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(
            content_type(std::path::Path::new("app.mcss")),
            "text/css; charset=utf-8"
        );
    }

    /// A stylesheet named for its source language is still a stylesheet.
    ///
    /// Four sites in the fleet emit their built CSS as `app.scss`. Served as
    /// `application/octet-stream` the engine ignored the link entirely, so
    /// those pages rendered with no styling at all: every element at the
    /// body's default 8px margin, every responsive rule inert, the desktop
    /// and mobile halves of a header both visible at once.
    ///
    /// Nothing failed. A check that asks whether a control exists and paints
    /// gets the same answer from an unstyled page, which is what makes this
    /// worth a test rather than a fix.
    #[test]
    fn a_stylesheet_is_typed_as_css_whatever_the_build_named_it() {
        for name in ["app.css", "app.mcss", "app.scss", "app.sass", "app.less"] {
            assert_eq!(
                content_type(std::path::Path::new(name)),
                "text/css; charset=utf-8",
                "{name} is a stylesheet"
            );
        }
    }

    #[test]
    fn a_brotli_bundle_is_served_as_the_script_it_decompresses_to() {
        let source = "export const value = 1;";
        let mut compressed = Vec::new();
        {
            let mut writer = brotli::CompressorWriter::new(&mut compressed, 4096, 9, 22);
            writer.write_all(source.as_bytes()).expect("compress");
        }
        assert_ne!(compressed.as_slice(), source.as_bytes());

        let served = decoded("text/javascript; charset=utf-8", compressed);
        assert_eq!(String::from_utf8(served).expect("utf8"), source);
    }

    #[test]
    fn a_plain_script_is_served_byte_for_byte() {
        let source = b"export const value = 1;".to_vec();
        assert_eq!(
            decoded("text/javascript; charset=utf-8", source.clone()),
            source
        );
    }

    #[test]
    fn a_binary_asset_is_never_probed_for_compression() {
        // A PNG header: not text, and not something to hand a decompressor.
        let png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        assert_eq!(decoded("image/png", png.clone()), png);
    }
}
