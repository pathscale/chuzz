//! A capability-protected loopback source for AVFoundation.
//!
//! Only a single selected asset is exposed. Its upstream requests use Chuzz's
//! provider, including identity, redirects and cookies. Range responses are
//! bounded, validated and forwarded without copying their bodies. Servers
//! without ranges retain the bounded complete response as media data.

use std::future::Future;
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::Poll;
use std::time::Duration;

use blitz_traits::net::http::HeaderValue;
use blitz_traits::net::{Bytes, Request, Url};
use blitz_traits::platform::FetchResponse;
use nagoya::net::{TcpListener, TcpStream};
use nagoya::reactor::{Addr, Reactor};

use super::MediaError;
use crate::document_loader::NetProvider;

const CHUNK_BYTES: u64 = 512 * 1024;
const MAX_MOVIE_BYTES: usize = 128 * 1024 * 1024;
const DEADLINE: Duration = Duration::from_secs(20);

#[derive(Default)]
struct State {
    src: String,
    validator: Option<String>,
    complete: Option<Bytes>,
    error: Option<MediaError>,
}

pub(super) struct Source {
    pub url: String,
    stopped: Arc<AtomicBool>,
    state: Arc<Mutex<State>>,
}

impl Drop for Source {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
    }
}

impl Source {
    pub fn new(net: Arc<NetProvider>, upstream: Url) -> Result<Self, MediaError> {
        static REACTOR: OnceLock<Result<Reactor, String>> = OnceLock::new();
        let reactor = REACTOR.get_or_init(|| Reactor::start().map_err(|error| error.to_string()));
        let reactor = reactor
            .as_ref()
            .map_err(|error| MediaError::network(format!("media reactor: {error}")))?;
        let listener = TcpListener::bind(Addr::localhost(0), &reactor.handle())
            .map_err(|error| MediaError::network(format!("media listener: {error}")))?;
        let port = listener
            .local_addr()
            .map_err(|error| MediaError::network(format!("media listener address: {error}")))?
            .port();

        // This is a secret capability, not a persisted identifier.
        let mut secret = [0_u8; 32];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut file| file.read_exact(&mut secret))
            .map_err(|error| MediaError::network(format!("media capability: {error}")))?;
        let token: String = secret.iter().map(|byte| format!("{byte:02x}")).collect();
        let path = format!("/{token}/media.mp4");
        let source = Self {
            url: format!("http://127.0.0.1:{port}{path}"),
            stopped: Arc::new(AtomicBool::new(false)),
            state: Arc::new(Mutex::new(State {
                src: upstream.to_string(),
                ..State::default()
            })),
        };
        let stopped = Arc::clone(&source.stopped);
        let state = Arc::clone(&source.state);
        let active = Arc::new(AtomicUsize::new(0));

        let _ = nagoya::spawn(async move {
            // Validate MP4 before AVFoundation sees any bytes. In particular,
            // an HLS playlist must never cause native upstream URL requests.
            let probe = until_stopped(
                &stopped,
                fetch(&net, upstream.clone(), Some("bytes=0-31"), None),
            )
            .await;
            let Some(probe) = probe else {
                return;
            };
            let result = probe.and_then(|response| {
                if !response.status.is_success() {
                    return Err(MediaError::network(format!(
                        "media probe returned HTTP {}",
                        response.status
                    )));
                }
                if response.body.get(4..8) != Some(b"ftyp".as_slice()) {
                    return Err(MediaError::unsupported("the media source is not an MP4"));
                }
                let mut state = state
                    .lock()
                    .map_err(|_| MediaError::network("media source state was poisoned"))?;
                state.src = response.url.to_string();
                state.validator = validator(&response);
                if response.status.as_u16() == 200 {
                    state.complete = Some(response.body);
                }
                Ok(())
            });
            if let Err(error) = result {
                record_error(&state, error);
            }

            loop {
                let Some(accepted) = until_stopped(&stopped, listener.accept()).await else {
                    return;
                };
                let Ok((stream, _)) = accepted else {
                    return;
                };
                let permit = Permit(Arc::clone(&active));
                let count = active.fetch_add(1, Ordering::AcqRel);
                let net = Arc::clone(&net);
                let stopped = Arc::clone(&stopped);
                let state = Arc::clone(&state);
                let upstream = upstream.clone();
                let path = path.clone();
                let _ = nagoya::spawn(async move {
                    let _permit = permit;
                    let task = async {
                        if count >= 4 {
                            let mut stream = stream;
                            return empty(&mut stream, 503).await;
                        }
                        answer(stream, &net, upstream, &path, &state).await
                    };
                    if let Some(result) =
                        until_stopped(&stopped, nagoya::timeout(DEADLINE, task)).await
                    {
                        match result {
                            Ok(Ok(())) => {}
                            Ok(Err(error)) => record_error(&state, error),
                            Err(_) => {
                                record_error(&state, MediaError::network("media request timed out"));
                            }
                        }
                    }
                });
            }
        });
        Ok(source)
    }

    pub fn status(&self) -> Result<String, MediaError> {
        let state = self
            .state
            .lock()
            .map_err(|_| MediaError::network("media source state was poisoned"))?;
        if let Some(error) = state.error.as_ref() {
            return Err(error.clone());
        }
        Ok(state.src.clone())
    }
}

struct Permit(Arc<AtomicUsize>);

impl Drop for Permit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn record_error(state: &Mutex<State>, error: MediaError) {
    if let Ok(mut state) = state.lock()
        && state.error.is_none()
    {
        state.error = Some(error);
    }
}

async fn until_stopped<F: Future>(stopped: &AtomicBool, future: F) -> Option<F::Output> {
    let mut future = std::pin::pin!(future);
    let mut cancelled = std::pin::pin!(async {
        while !stopped.load(Ordering::Acquire) {
            nagoya::sleep(Duration::from_millis(20)).await;
        }
    });
    std::future::poll_fn(|context| {
        if cancelled.as_mut().poll(context).is_ready() {
            return Poll::Ready(None);
        }
        future.as_mut().poll(context).map(Some)
    })
    .await
}

fn validator(response: &FetchResponse) -> Option<String> {
    response
        .headers
        .get("etag")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.starts_with("W/"))
        .or_else(|| {
            response
                .headers
                .get("last-modified")
                .and_then(|value| value.to_str().ok())
        })
        .map(str::to_owned)
}

async fn fetch(
    net: &NetProvider,
    url: Url,
    range: Option<&str>,
    previous: Option<&str>,
) -> Result<FetchResponse, MediaError> {
    let mut request = Request::get(url);
    request
        .headers
        .insert("accept-encoding", HeaderValue::from_static("identity"));
    if let Some(range) = range {
        request.headers.insert(
            "range",
            HeaderValue::from_str(range)
                .map_err(|_| MediaError::network("invalid media range"))?,
        );
    }
    if let Some(previous) = previous {
        request.headers.insert(
            "if-range",
            HeaderValue::from_str(previous)
                .map_err(|_| MediaError::network("invalid media validator"))?,
        );
    }
    let response = nagoya::timeout(DEADLINE, net.fetch_response_async(request))
        .await
        .map_err(|_| MediaError::network("media fetch timed out"))?
        .map_err(|error| MediaError::network(format!("media fetch failed: {error:?}")))?;
    if response.body.len() > MAX_MOVIE_BYTES {
        return Err(MediaError::unsupported("the media response exceeds 128 MiB"));
    }
    if let Some(encoding) = response.headers.get("content-encoding")
        && encoding.as_bytes() != b"identity"
    {
        return Err(MediaError::network("encoded media cannot be addressed by byte range"));
    }
    if let (Some(previous), Some(current)) = (previous, validator(&response))
        && previous != current
    {
        return Err(MediaError::network("the media resource changed during playback"));
    }
    if response.status.as_u16() == 206 {
        let requested = range.ok_or_else(|| MediaError::network("unsolicited partial media"))?;
        let value = response
            .headers
            .get("content-range")
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| MediaError::network("partial media has no Content-Range"))?;
        let valid = (|| {
            let (interval, total) = value.strip_prefix("bytes ")?.split_once('/')?;
            let (start, end) = interval.split_once('-')?;
            let start = start.parse::<u64>().ok()?;
            let end = end.parse::<u64>().ok()?;
            let total = total.parse::<u64>().ok()?;
            let (requested_start, requested_end) = interval_for(requested, total)?;
            Some(
                start == requested_start
                    && end >= start
                    && end <= requested_end
                    && end < total
                    && end - start + 1 == response.body.len() as u64,
            )
        })()
        .unwrap_or(false);
        if !valid {
            return Err(MediaError::network("invalid partial media response"));
        }
    }
    Ok(response)
}

fn bounded_range(value: &str) -> Option<String> {
    let value = value.strip_prefix("bytes=")?;
    let (start, end) = value.split_once('-')?;
    if start.is_empty() {
        let length = end.parse::<u64>().ok()?.min(CHUNK_BYTES);
        return (length > 0).then(|| format!("bytes=-{length}"));
    }
    let start = start.parse::<u64>().ok()?;
    let limit = start.checked_add(CHUNK_BYTES - 1)?;
    let end = if end.is_empty() {
        limit
    } else {
        end.parse::<u64>().ok()?.min(limit)
    };
    (end >= start).then(|| format!("bytes={start}-{end}"))
}

fn interval_for(value: &str, total: u64) -> Option<(u64, u64)> {
    if total == 0 {
        return None;
    }
    let (start, end) = value.strip_prefix("bytes=")?.split_once('-')?;
    if start.is_empty() {
        let length = end.parse::<u64>().ok()?;
        return (length > 0).then(|| (total.saturating_sub(length), total - 1));
    }
    let start = start.parse::<u64>().ok()?;
    let end = end.parse::<u64>().ok()?.min(total - 1);
    (start <= end).then_some((start, end))
}

async fn answer(
    mut stream: TcpStream,
    net: &NetProvider,
    upstream: Url,
    path: &str,
    state: &Mutex<State>,
) -> Result<(), MediaError> {
    let mut bytes = Vec::new();
    let mut byte = [0_u8; 1];
    while !bytes.ends_with(b"\r\n\r\n") {
        if bytes.len() >= 16 * 1024 {
            return empty(&mut stream, 400).await;
        }
        if stream
            .read(&mut byte)
            .await
            .map_err(|error| MediaError::network(error.to_string()))?
            == 0
        {
            return Ok(());
        }
        bytes.push(byte[0]);
    }
    let Ok(head) = std::str::from_utf8(&bytes) else {
        return empty(&mut stream, 400).await;
    };
    let mut lines = head.lines();
    let mut request = lines.next().unwrap_or_default().split_whitespace();
    let method = request.next().unwrap_or_default();
    if request.next() != Some(path) {
        return empty(&mut stream, 404).await;
    }
    if !matches!(method, "GET" | "HEAD") {
        return empty(&mut stream, 405).await;
    }
    let mut range = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("range")
        {
            if range.is_some() {
                return empty(&mut stream, 400).await;
            }
            let Some(value) = bounded_range(value.trim()) else {
                return empty(&mut stream, 400).await;
            };
            range = Some(value);
        }
    }

    let (complete, previous, failed) = {
        let state = state
            .lock()
            .map_err(|_| MediaError::network("media source state was poisoned"))?;
        (
            state.complete.clone(),
            state.validator.clone(),
            state.error.is_some(),
        )
    };
    if failed {
        return empty(&mut stream, 502).await;
    }
    if let Some(body) = complete {
        return local(&mut stream, &body, range.as_deref(), method == "HEAD").await;
    }

    // A HEAD response needs the resource length, not a complete upstream GET.
    // The two-byte probe supplies it through Content-Range.
    let requested = if method == "HEAD" {
        Some("bytes=0-1")
    } else {
        range.as_deref()
    };
    let response = fetch(net, upstream, requested, previous.as_deref()).await?;
    if !response.status.is_success() && response.status.as_u16() != 416 {
        return Err(MediaError::network(format!(
            "media request returned HTTP {}",
            response.status
        )));
    }
    {
        let mut state = state
            .lock()
            .map_err(|_| MediaError::network("media source state was poisoned"))?;
        state.src = response.url.to_string();
    }

    if method == "HEAD" {
        let length = if response.status.as_u16() == 206 {
            response
                .headers
                .get("content-range")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.rsplit_once('/'))
                .and_then(|(_, total)| total.parse::<u64>().ok())
                .ok_or_else(|| MediaError::network("media size is unavailable"))?
        } else {
            response.body.len() as u64
        };
        return write(
            &mut stream,
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\nContent-Length: {length}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n"
            ),
            &[],
        )
        .await;
    }

    let status = response.status.as_u16();
    let reason = response.status.canonical_reason().unwrap_or("Media");
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: video/mp4\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n",
        response.body.len()
    );
    for name in ["content-range", "accept-ranges", "etag", "last-modified"] {
        if let Some(value) = response.headers.get(name)
            && let Ok(value) = value.to_str()
            && !value.contains(['\r', '\n'])
        {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
    }
    head.push_str("\r\n");
    write(&mut stream, head, &response.body).await
}

async fn local(
    stream: &mut TcpStream,
    body: &[u8],
    range: Option<&str>,
    head_only: bool,
) -> Result<(), MediaError> {
    let total = body.len() as u64;
    let (status, interval, bytes) = if head_only {
        (200, String::new(), body)
    } else if let Some(range) = range {
        let Some((start, end)) = interval_for(range, total) else {
            return write(
                stream,
                format!(
                    "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{total}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                ),
                &[],
            )
            .await;
        };
        (
            206,
            format!("Content-Range: bytes {start}-{end}/{total}\r\n"),
            &body[start as usize..=end as usize],
        )
    } else {
        (200, String::new(), body)
    };
    let reason = if status == 206 { "Partial Content" } else { "OK" };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: video/mp4\r\nContent-Length: {}\r\n{interval}Accept-Ranges: bytes\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        bytes.len()
    );
    write(stream, head, if head_only { &[] } else { bytes }).await
}

async fn empty(stream: &mut TcpStream, status: u16) -> Result<(), MediaError> {
    write(
        stream,
        format!(
            "HTTP/1.1 {status} Media\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        ),
        &[],
    )
    .await
}

async fn write(stream: &mut TcpStream, head: String, body: &[u8]) -> Result<(), MediaError> {
    stream
        .write_all(head.as_bytes())
        .await
        .map_err(|error| MediaError::network(error.to_string()))?;
    stream
        .write_all(body)
        .await
        .map_err(|error| MediaError::network(error.to_string()))
}
