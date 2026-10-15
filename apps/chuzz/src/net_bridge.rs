//! Page networking, cookie access and completion wakeups shared by both hosts.

#[path = "net_bridge_script.rs"]
mod script;

use std::sync::{Arc, Mutex};
use std::task::Waker;
use std::time::Duration;

use blitz_traits::net::Url;
use blitz_traits::platform::{FetchError, FetchRequest, FetchResponse};
use blitz_traits::shell::ShellProvider;

use crate::decode::decode_body;
use crate::document_loader::NetProvider;

#[derive(Default)]
struct WakeState {
    waker: Option<Waker>,
    shell: Option<Arc<dyn ShellProvider>>,
    retired: bool,
}

/// Publish results before waking the document. Never hold a queue lock while
/// calling a host waker or the window's queued shell provider.
#[derive(Clone, Default)]
pub(crate) struct PageWake(Arc<Mutex<WakeState>>);

impl PageWake {
    pub(crate) fn new(shell: Arc<dyn ShellProvider>) -> Self {
        Self(Arc::new(Mutex::new(WakeState {
            waker: None,
            shell: Some(shell),
            retired: false,
        })))
    }

    fn retire(&self) {
        let mut state = self.0.lock().unwrap_or_else(|poison| poison.into_inner());
        state.retired = true;
        state.waker.take();
        state.shell.take();
    }

    pub(crate) fn bind(&self, waker: Option<&Waker>) {
        if let Some(waker) = waker {
            self.0
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .waker = Some(waker.clone());
        }
    }

    pub(crate) fn wake(&self) {
        let (waker, shell) = {
            let state = self.0.lock().unwrap_or_else(|poison| poison.into_inner());
            if state.retired {
                return;
            }
            (state.waker.clone(), state.shell.clone())
        };
        if let Some(waker) = waker {
            waker.wake();
        } else if let Some(shell) = shell {
            shell.request_redraw();
        }
    }
}

pub(super) struct Delivery {
    pub(super) id: u64,
    pub(super) reply: Result<NetReply, NetworkError>,
}

#[derive(Clone)]
pub(super) struct Mailbox {
    entries: Arc<Mutex<Vec<Delivery>>>,
    wake: PageWake,
}

impl Mailbox {
    pub(super) fn post(&self, delivery: Delivery) {
        self.entries
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(delivery);
        self.wake.wake();
    }

    pub(super) fn drain(&self) -> Vec<Delivery> {
        std::mem::take(
            &mut *self
                .entries
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()),
        )
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct NetRequest {
    #[serde(default)]
    pub(crate) id: u64,
    pub(crate) url: String,
    #[serde(default = "get_method")]
    pub(crate) method: String,
    #[serde(default)]
    pub(crate) headers: Vec<(String, String)>,
    #[serde(default)]
    pub(crate) body: Option<Vec<u8>>,
    #[serde(default = "default_credentials")]
    pub(crate) credentials: String,
}

fn get_method() -> String {
    "GET".to_owned()
}

fn default_credentials() -> String {
    "same-origin".to_owned()
}

pub(crate) struct NetReply {
    pub(crate) status: u16,
    pub(crate) status_text: String,
    pub(crate) url: Url,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: String,
    pub(crate) bytes: Vec<u8>,
}

#[derive(Debug)]
pub(crate) struct NetworkError(String);

impl NetworkError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl std::fmt::Display for NetworkError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for NetworkError {}

async fn fetch_response_owned(
    provider: &NetProvider,
    request: FetchRequest,
    lifetime: &Arc<blitz_dom::release::Teardown>,
) -> Result<FetchResponse, FetchError> {
    let _lease = lifetime.lease();
    fetch_response(provider, request).await
}

pub(crate) async fn fetch_response(
    provider: &NetProvider,
    request: FetchRequest,
) -> Result<FetchResponse, FetchError> {
    provider.fetch_platform_async(request).await
}

#[derive(Clone)]
pub(crate) struct NetworkHost {
    pub(super) net: Arc<NetProvider>,
    pub(super) base: Option<Url>,
    deadline: Duration,
    signal: blitz_traits::net::AbortSignal,
    lifetime: Arc<blitz_dom::release::Teardown>,
}

impl NetworkHost {
    pub(super) fn new(
        net: Arc<NetProvider>,
        base: Option<Url>,
        deadline: Duration,
        signal: blitz_traits::net::AbortSignal,
        lifetime: Arc<blitz_dom::release::Teardown>,
    ) -> Self {
        Self {
            net,
            base,
            deadline,
            signal,
            lifetime,
        }
    }

    pub(crate) async fn perform(&self, request: NetRequest) -> Result<NetReply, NetworkError> {
        let _lease = self.lifetime.lease();
        match nagoya::timeout(self.deadline, self.perform_inner(request)).await {
            Ok(Ok(reply)) => Ok(reply),
            Ok(Err(error)) => Err(NetworkError::new(error)),
            Err(_) => Err(NetworkError::new("timed out")),
        }
    }

    pub(crate) async fn perform_unbounded(
        &self,
        request: NetRequest,
    ) -> Result<NetReply, NetworkError> {
        let _lease = self.lifetime.lease();
        self.perform_inner(request).await.map_err(NetworkError::new)
    }

    pub(crate) fn cookie_get(&self) -> String {
        self.base
            .as_ref()
            .map(|url| self.net.cookie_store().script_cookies(url))
            .unwrap_or_default()
    }

    pub(crate) fn cookie_set(&self, assignment: &str) -> Result<String, NetworkError> {
        if let Some(url) = &self.base {
            self.net
                .cookie_store()
                .set_script_cookie(url, assignment)
                .map_err(|error| NetworkError::new(error.to_string()))?;
        }
        Ok(self.cookie_get())
    }
}

/// The `strict-origin-when-cross-origin` Referer for a request from `document`:
/// the full URL (no fragment or credentials) to the same origin, the origin
/// across origins, nothing on an HTTPS to HTTP downgrade or from a non-HTTP page.
fn default_referrer(document: &Url, target: &Url) -> Option<String> {
    if !matches!(document.scheme(), "http" | "https")
        || document.scheme() == "https" && target.scheme() == "http"
    {
        return None;
    }
    if document.origin() == target.origin() {
        let mut full = document.clone();
        full.set_fragment(None);
        let _ = full.set_username("");
        let _ = full.set_password(None);
        Some(full.to_string())
    } else {
        Some(format!("{}/", document.origin().ascii_serialization()))
    }
}

fn forbidden_header(name: &str) -> bool {
    matches!(
        name,
        "accept-charset"
            | "accept-encoding"
            | "access-control-request-headers"
            | "access-control-request-method"
            | "connection"
            | "content-length"
            | "cookie"
            | "cookie2"
            | "date"
            | "dnt"
            | "expect"
            | "host"
            | "keep-alive"
            | "origin"
            | "referer"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "via"
    ) || name.starts_with("proxy-")
        || name.starts_with("sec-")
}

fn cors_allowed(response: &FetchResponse, origin: &str, credentials: &str) -> bool {
    let allowed = response
        .headers
        .get("access-control-allow-origin")
        .and_then(|value| value.to_str().ok());
    (allowed == Some(origin) || (allowed == Some("*") && credentials != "include"))
        && (credentials != "include"
            || response
                .headers
                .get("access-control-allow-credentials")
                .and_then(|value| value.to_str().ok())
                == Some("true"))
}

fn header_tokens(response: &FetchResponse, name: &str) -> Vec<String> {
    response
        .headers
        .get_all(name)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(|value| value.trim().to_ascii_lowercase())
        .collect()
}

impl NetworkHost {
    async fn perform_inner(&self, request: NetRequest) -> Result<NetReply, String> {
        use blitz_traits::platform::http::{HeaderName, HeaderValue, Method};

        let mut url = Url::options()
            .base_url(self.base.as_ref())
            .parse(&request.url)
            .map_err(|error| error.to_string())?;
        if !matches!(url.scheme(), "http" | "https" | "data") {
            return Err("unsupported request scheme".to_owned());
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err("request URL cannot contain credentials".to_owned());
        }
        url.set_fragment(None);
        if crate::telemetry::blocked_url(&url) {
            return Err(crate::telemetry::diagnostic(&url));
        }

        let method =
            Method::from_bytes(request.method.as_bytes()).map_err(|error| error.to_string())?;
        if matches!(method.as_str(), "CONNECT" | "TRACE" | "TRACK") {
            return Err("forbidden request method".to_owned());
        }
        let credentials = match request.credentials.as_str() {
            "same-origin" => blitz_traits::net::Credentials::SameOrigin,
            "include" => blitz_traits::net::Credentials::Include,
            "omit" => blitz_traits::net::Credentials::Omit,
            _ => return Err("invalid credentials mode".to_owned()),
        };
        let mut wire = FetchRequest::get(url.clone())
            .method(method.clone())
            .signal(self.signal.clone());
        wire.credentials = credentials;
        wire.initiator = self.net.initiator();
        let mut unsafe_headers = Vec::new();
        for (name, value) in request.headers {
            let name =
                HeaderName::from_bytes(name.as_bytes()).map_err(|error| error.to_string())?;
            if forbidden_header(name.as_str()) {
                continue;
            }
            let value = HeaderValue::from_str(&value).map_err(|error| error.to_string())?;
            let safelisted = match name.as_str() {
                "accept" | "accept-language" | "content-language" => {
                    value.as_bytes().len() <= 128
                        && !value
                            .as_bytes()
                            .iter()
                            .any(|byte| *byte < 0x20 && *byte != b'\t' || *byte == 0x7f)
                }
                "content-type" => value.to_str().is_ok_and(|value| {
                    matches!(
                        value.split(';').next().unwrap_or_default().trim(),
                        "application/x-www-form-urlencoded" | "multipart/form-data" | "text/plain"
                    )
                }),
                _ => false,
            };
            if !safelisted {
                unsafe_headers.push(name.as_str().to_owned());
            }
            wire.headers.append(name, value);
        }
        crate::languages::insert_accept_language(&mut wire.headers);
        // The page cannot set Referer; the browser sends one under the default
        // strict-origin-when-cross-origin policy, as for engine subresources.
        if let Some(referrer) = self
            .base
            .as_ref()
            .and_then(|base| default_referrer(base, &url))
            && let Ok(value) = HeaderValue::from_str(&referrer)
        {
            wire.headers
                .insert(HeaderName::from_static("referer"), value);
        }
        crate::identity::stamp_request_headers(
            &mut wire.headers,
            self.net.user_agent(),
            &url,
            crate::identity::RequestKind::Fetch {
                initiator: self.base.as_ref(),
            },
        );
        if !matches!(method, Method::GET | Method::HEAD) {
            wire.body = request.body.map(Into::into);
        }

        let origin = self
            .base
            .as_ref()
            .filter(|base| matches!(base.scheme(), "http" | "https"))
            .map(|base| base.origin().ascii_serialization());
        let cross_origin = matches!(url.scheme(), "http" | "https")
            && self
                .base
                .as_ref()
                .filter(|base| matches!(base.scheme(), "http" | "https"))
                .is_none_or(|base| base.origin() != url.origin());
        if cross_origin {
            let origin = origin.as_deref().unwrap_or("null");
            wire.headers.insert(
                HeaderName::from_static("origin"),
                HeaderValue::from_str(origin).map_err(|error| error.to_string())?,
            );
            if !matches!(method, Method::GET | Method::HEAD | Method::POST)
                || !unsafe_headers.is_empty()
            {
                unsafe_headers.sort();
                unsafe_headers.dedup();
                let mut preflight = FetchRequest::get(url.clone())
                    .method(Method::OPTIONS)
                    .signal(self.signal.clone());
                preflight.credentials = blitz_traits::net::Credentials::Omit;
                preflight.initiator = self.net.initiator();
                preflight.headers.insert(
                    HeaderName::from_static("origin"),
                    HeaderValue::from_str(origin).map_err(|error| error.to_string())?,
                );
                preflight.headers.insert(
                    HeaderName::from_static("access-control-request-method"),
                    HeaderValue::from_str(method.as_str()).map_err(|error| error.to_string())?,
                );
                if !unsafe_headers.is_empty() {
                    preflight.headers.insert(
                        HeaderName::from_static("access-control-request-headers"),
                        HeaderValue::from_str(&unsafe_headers.join(", "))
                            .map_err(|error| error.to_string())?,
                    );
                }
                crate::languages::insert_accept_language(&mut preflight.headers);
                let response = fetch_response_owned(&self.net, preflight, &self.lifetime)
                    .await
                    .map_err(|error| error.to_string())?;
                let methods = header_tokens(&response, "access-control-allow-methods");
                let headers = header_tokens(&response, "access-control-allow-headers");
                if !response.status.is_success()
                    || !cors_allowed(&response, origin, &request.credentials)
                    || (!matches!(method, Method::GET | Method::HEAD | Method::POST)
                        && !methods.contains(&method.as_str().to_ascii_lowercase())
                        && !(methods.contains(&"*".to_owned()) && request.credentials != "include"))
                    || unsafe_headers.iter().any(|name| {
                        !headers.contains(name)
                            && !(headers.contains(&"*".to_owned())
                                && request.credentials != "include"
                                && name != "authorization")
                    })
                {
                    return Err("CORS preflight rejected".to_owned());
                }
            }
        }

        let response = fetch_response_owned(&self.net, wire, &self.lifetime)
            .await
            .map_err(|error| error.to_string())?;
        if !matches!(response.url.scheme(), "http" | "https" | "data") {
            return Err("unsupported response scheme".to_owned());
        }
        let response_cross_origin = matches!(response.url.scheme(), "http" | "https")
            && self
                .base
                .as_ref()
                .filter(|base| matches!(base.scheme(), "http" | "https"))
                .is_none_or(|base| base.origin() != response.url.origin());
        if response_cross_origin
            && !cors_allowed(
                &response,
                origin.as_deref().unwrap_or("null"),
                &request.credentials,
            )
        {
            return Err("CORS response rejected".to_owned());
        }
        let exposed = header_tokens(&response, "access-control-expose-headers");
        let headers = response
            .headers
            .iter()
            .filter(|(name, _)| {
                !matches!(name.as_str(), "set-cookie" | "set-cookie2")
                    && (!response_cross_origin
                        || matches!(
                            name.as_str(),
                            "cache-control"
                                | "content-language"
                                | "content-length"
                                | "content-type"
                                | "expires"
                                | "last-modified"
                                | "pragma"
                        )
                        || exposed.iter().any(|value| value == name.as_str())
                        || (request.credentials != "include"
                            && exposed.iter().any(|value| value == "*")))
            })
            .filter_map(|(name, value)| value.to_str().ok().map(|value| (name.as_str(), value)))
            .collect::<Vec<_>>();
        let bytes = response.body.to_vec();
        Ok(NetReply {
            status: response.status.as_u16(),
            status_text: response.status.canonical_reason().unwrap_or("").to_owned(),
            url: response.url,
            headers: headers
                .into_iter()
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect(),
            body: decode_body(&bytes),
            bytes,
        })
    }
}

pub(crate) async fn worker_fetch(
    net: Arc<NetProvider>,
    base: Option<Url>,
    signal: blitz_traits::net::AbortSignal,
    lifetime: Arc<blitz_dom::release::Teardown>,
    request: NetRequest,
) -> Result<(serde_json::Value, Vec<u8>), String> {
    script::worker_fetch(net, base, signal, lifetime, request).await
}

pub fn install(
    document: &mut blitz_script::ScriptDocument,
    net: Arc<NetProvider>,
    deadline: Duration,
    base: Option<Url>,
) {
    script::install(document, net, deadline, base);
}
