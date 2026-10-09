//! Direct, bounded range reads through Chuzz's network provider.
//!
//! No socket or platform player sees the upstream URL. Identity, cookies,
//! redirects and fetch policy belong to the existing provider.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;

use blitz_traits::net::http::HeaderValue;
use blitz_traits::net::{Bytes, Request, Url};
use blitz_traits::platform::FetchResponse;

use super::MediaError;
use crate::document_loader::NetProvider;

const CHUNK_BYTES: usize = 512 * 1024;
const MAX_COMPLETE_BYTES: usize = 128 * 1024 * 1024;
pub(super) const MAX_READ_BYTES: usize = 16 * 1024 * 1024;
const DEADLINE: Duration = Duration::from_secs(20);

pub(super) struct Source {
    net: Arc<NetProvider>,
    url: Mutex<Url>,
    validator: Option<String>,
    complete: Mutex<Option<Bytes>>,
    stopped: Arc<AtomicBool>,
    pub length: u64,
}

impl Source {
    pub async fn open(
        net: Arc<NetProvider>,
        url: Url,
        stopped: Arc<AtomicBool>,
    ) -> Result<Arc<Self>, MediaError> {
        let (response, length) = fetch(&net, url, 0, 31, None, &stopped).await?;
        if length < 8 {
            return Err(MediaError::unsupported("the media source is not an MP4"));
        }
        let validator = validator(&response);
        let complete = (response.status.as_u16() == 200).then_some(response.body);
        Ok(Arc::new(Self {
            net,
            url: Mutex::new(response.url),
            validator,
            complete: Mutex::new(complete),
            stopped,
            length,
        }))
    }

    pub fn status(&self) -> Result<String, MediaError> {
        self.url
            .lock()
            .map(|url| url.to_string())
            .map_err(|_| MediaError::network("media source state was poisoned"))
    }

    pub async fn read(&self, offset: u64, length: usize) -> Result<Bytes, MediaError> {
        let end = offset
            .checked_add(length as u64)
            .filter(|end| *end <= self.length)
            .ok_or_else(|| MediaError::decode("a media sample exceeds the resource"))?;
        if length > MAX_READ_BYTES {
            return Err(MediaError::unsupported("a media read exceeds 16 MiB"));
        }
        if length == 0 {
            return Ok(Bytes::new());
        }
        let complete = self
            .complete
            .lock()
            .map_err(|_| MediaError::network("media source state was poisoned"))?
            .clone();
        if let Some(body) = complete {
            return Ok(body.slice(offset as usize..end as usize));
        }

        let mut position = offset;
        let mut joined = Vec::new();
        while position < end {
            let request_end = (position + CHUNK_BYTES as u64).min(end) - 1;
            let url = self
                .url
                .lock()
                .map_err(|_| MediaError::network("media source state was poisoned"))?
                .clone();
            let (response, total) = fetch(
                &self.net,
                url,
                position,
                request_end,
                self.validator.as_deref(),
                &self.stopped,
            )
            .await?;
            if total != self.length {
                return Err(MediaError::network(
                    "the media resource changed during playback",
                ));
            }
            *self
                .url
                .lock()
                .map_err(|_| MediaError::network("media source state was poisoned"))? =
                response.url;

            if response.status.as_u16() == 200 {
                let body = response.body;
                let selected = body.slice(offset as usize..end as usize);
                *self
                    .complete
                    .lock()
                    .map_err(|_| MediaError::network("media source state was poisoned"))? =
                    Some(body);
                return Ok(selected);
            }
            if position == offset && response.body.len() == length {
                return Ok(response.body);
            }
            if joined.is_empty() {
                joined.reserve(length);
            }
            position += response.body.len() as u64;
            joined.extend_from_slice(&response.body);
        }
        Ok(Bytes::from(joined))
    }
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
    start: u64,
    end: u64,
    previous: Option<&str>,
    stopped: &AtomicBool,
) -> Result<(FetchResponse, u64), MediaError> {
    let mut request = Request::get(url);
    request
        .headers
        .insert("accept-encoding", HeaderValue::from_static("identity"));
    request.headers.insert(
        "range",
        HeaderValue::from_str(&format!("bytes={start}-{end}"))
            .map_err(|_| MediaError::network("invalid media range"))?,
    );
    if let Some(previous) = previous {
        request.headers.insert(
            "if-range",
            HeaderValue::from_str(previous)
                .map_err(|_| MediaError::network("invalid media validator"))?,
        );
    }

    let response = until_stopped(
        stopped,
        nagoya::timeout(DEADLINE, net.fetch_response_async(request)),
    )
    .await
    .ok_or_else(|| MediaError::network("media loading was cancelled"))?
    .map_err(|_| MediaError::network("media fetch timed out"))?
    .map_err(|error| MediaError::network(format!("media fetch failed: {error:?}")))?;

    if let Some(encoding) = response.headers.get("content-encoding")
        && encoding.as_bytes() != b"identity"
    {
        return Err(MediaError::network(
            "encoded media cannot be addressed by byte range",
        ));
    }
    if let Some(previous) = previous
        && validator(&response).as_deref() != Some(previous)
    {
        return Err(MediaError::network(
            "the media resource changed during playback",
        ));
    }

    let total = match response.status.as_u16() {
        200 => {
            if response.body.len() > MAX_COMPLETE_BYTES {
                return Err(MediaError::unsupported(
                    "the complete media response exceeds 128 MiB",
                ));
            }
            response.body.len() as u64
        }
        206 => {
            let value = response
                .headers
                .get("content-range")
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| MediaError::network("partial media has no Content-Range"))?;
            let parsed = (|| {
                let (interval, total) = value.strip_prefix("bytes ")?.split_once('/')?;
                let (first, last) = interval.split_once('-')?;
                Some((
                    first.parse::<u64>().ok()?,
                    last.parse::<u64>().ok()?,
                    total.parse::<u64>().ok()?,
                ))
            })()
            .ok_or_else(|| MediaError::network("invalid partial media response"))?;
            let (first, last, total) = parsed;
            if total == 0
                || first != start
                || first > last
                || last != end.min(total - 1)
                || last >= total
                || last - first + 1 != response.body.len() as u64
                || response.body.len() > CHUNK_BYTES
            {
                return Err(MediaError::network("invalid partial media response"));
            }
            total
        }
        _ => {
            return Err(MediaError::network(format!(
                "media request returned HTTP {}",
                response.status
            )));
        }
    };
    Ok((response, total))
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
