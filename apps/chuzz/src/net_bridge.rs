//! Page networking, cookie access and completion wakeups shared by both hosts.

#[path = "net_bridge_host.rs"]
pub mod native_host;

use std::sync::{Arc, Mutex};
use std::task::Waker;
use std::time::Duration;

use blitz_traits::net::{Credentials, Url};
use blitz_traits::platform::http::{HeaderName, HeaderValue, Method};
use blitz_traits::platform::{FetchError, FetchRequest, FetchResponse};
use blitz_traits::shell::ShellProvider;

use crate::decode::decode_body;
use crate::document_loader::NetProvider;

const MAX_FETCH_REDIRECTS: usize = 20;

#[derive(Default)]
struct WakeState {
    waker: Option<Waker>,
    shell: Option<Arc<dyn ShellProvider>>,
    retired: bool,
}

/// Publish results before waking the document. Never hold a queue lock while
/// calling a host waker or the window's queued shell provider.
#[derive(Clone, Default)]
pub struct PageWake(Arc<Mutex<WakeState>>);

impl PageWake {
    pub fn new(shell: Arc<dyn ShellProvider>) -> Self {
        Self(Arc::new(Mutex::new(WakeState {
            waker: None,
            shell: Some(shell),
            retired: false,
        })))
    }

    pub fn retire(&self) {
        let mut state = self.0.lock().unwrap_or_else(|poison| poison.into_inner());
        state.retired = true;
        state.waker.take();
        state.shell.take();
    }

    pub fn bind(&self, waker: Option<&Waker>) {
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

#[derive(serde::Deserialize)]
/// Owned request accepted by the page and dedicated worker network bridges.
pub struct NetRequest {
    #[serde(default)]
    pub id: u64,
    pub url: String,
    #[serde(default = "get_method")]
    pub method: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub body: Option<Vec<u8>>,
    #[serde(default = "default_credentials")]
    pub credentials: String,
    /// A browser-initiated no-cors subresource load (a classic `<script src>`),
    /// named by its fetch destination. Page `fetch()` requests leave it unset
    /// and keep CORS.
    #[serde(skip)]
    pub no_cors_destination: Option<&'static str>,
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
    pub(crate) redirected: bool,
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

fn cors_allowed(response: &FetchResponse, origin: &str, credentials: Credentials) -> bool {
    let allowed = response
        .headers
        .get("access-control-allow-origin")
        .and_then(|value| value.to_str().ok());
    (allowed == Some(origin) || (allowed == Some("*") && credentials != Credentials::Include))
        && (credentials != Credentials::Include
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
    async fn preflight(
        &self,
        wire: &FetchRequest,
        origin: &str,
        credentials: Credentials,
        unsafe_headers: &[String],
    ) -> Result<(), String> {
        let simple_method = matches!(wire.method, Method::GET | Method::HEAD | Method::POST);
        if simple_method && unsafe_headers.is_empty() {
            return Ok(());
        }

        let mut preflight = FetchRequest::get(wire.url.clone())
            .method(Method::OPTIONS)
            .signal(self.signal.clone())
            .return_redirects();
        preflight.credentials = Credentials::Omit;
        preflight.initiator = self.net.initiator();
        preflight.headers.insert(
            "origin",
            HeaderValue::from_str(origin).map_err(|error| error.to_string())?,
        );
        preflight.headers.insert(
            "access-control-request-method",
            HeaderValue::from_str(wire.method.as_str()).map_err(|error| error.to_string())?,
        );
        if !unsafe_headers.is_empty() {
            preflight.headers.insert(
                "access-control-request-headers",
                HeaderValue::from_str(&unsafe_headers.join(", "))
                    .map_err(|error| error.to_string())?,
            );
        }
        let response = fetch_response_owned(&self.net, preflight, &self.lifetime)
            .await
            .map_err(|error| error.to_string())?;
        // Only this URL's successful OPTIONS response can authorize dispatch.
        if response.url != wire.url
            || !response.status.is_success()
            || !cors_allowed(&response, origin, credentials)
        {
            return Err("CORS preflight rejected".to_owned());
        }
        let methods = header_tokens(&response, "access-control-allow-methods");
        let headers = header_tokens(&response, "access-control-allow-headers");
        let wildcard = |tokens: &[String]| {
            credentials != Credentials::Include && tokens.iter().any(|token| token == "*")
        };
        let wildcard_headers = wildcard(&headers);
        if (!simple_method
            && !methods.contains(&wire.method.as_str().to_ascii_lowercase())
            && !wildcard(&methods))
            || unsafe_headers.iter().any(|name| {
                !headers.contains(name) && (name == "authorization" || !wildcard_headers)
            })
        {
            return Err("CORS preflight rejected".to_owned());
        }
        Ok(())
    }

    async fn perform_inner(&self, request: NetRequest) -> Result<NetReply, String> {
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
            "same-origin" => Credentials::SameOrigin,
            "include" => Credentials::Include,
            "omit" => Credentials::Omit,
            _ => return Err("invalid credentials mode".to_owned()),
        };
        let mut wire = FetchRequest::get(url.clone())
            .method(method.clone())
            .signal(self.signal.clone())
            .return_redirects();
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
        unsafe_headers.sort_unstable();
        unsafe_headers.dedup();
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
        if !matches!(method, Method::GET | Method::HEAD) {
            wire.body = request.body.map(Into::into);
        }

        let document_origin = self
            .base
            .as_ref()
            .filter(|base| matches!(base.scheme(), "http" | "https"))
            .map(Url::origin);
        let origin = document_origin
            .as_ref()
            .map_or_else(|| "null".to_owned(), |origin| origin.ascii_serialization());
        let mut cors = false;
        let mut tainted_origin = false;
        let mut redirect_site = None;
        let mut redirects = 0;
        let response = loop {
            let current_origin = wire.url.origin();
            cors |= request.no_cors_destination.is_none()
                && matches!(wire.url.scheme(), "http" | "https")
                && document_origin.as_ref() != Some(&current_origin);
            let request_origin = if tainted_origin {
                "null"
            } else {
                origin.as_str()
            };

            crate::identity::stamp_request_headers(
                &mut wire.headers,
                self.net.user_agent(),
                &wire.url,
                match request.no_cors_destination {
                    Some(destination) => crate::identity::RequestKind::Subresource {
                        destination,
                        initiator: self.base.as_ref(),
                    },
                    None => crate::identity::RequestKind::Fetch {
                        initiator: self.base.as_ref(),
                    },
                },
            );
            if let Some(site) = redirect_site
                && let Some(value) = wire.headers.get_mut("sec-fetch-site")
            {
                *value = HeaderValue::from_static(site);
            }
            if cors {
                if let Some(value) = wire.headers.get_mut("sec-fetch-mode") {
                    *value = HeaderValue::from_static("cors");
                }
                wire.headers.insert(
                    "origin",
                    HeaderValue::from_str(request_origin).map_err(|error| error.to_string())?,
                );
                // Redirect method rewrites can remove body headers, and a
                // cross-origin hop can remove authorization.
                unsafe_headers.retain(|name| wire.headers.contains_key(name.as_str()));
                self.preflight(&wire, request_origin, credentials, &unsafe_headers)
                    .await?;
                // A chain remains CORS-tainted even if it returns to the
                // page's origin.
                if credentials == Credentials::SameOrigin {
                    wire.credentials = Credentials::Omit;
                }
            }

            let mut response = fetch_response_owned(&self.net, wire, &self.lifetime)
                .await
                .map_err(|error| error.to_string())?;
            if !matches!(response.url.scheme(), "http" | "https" | "data") {
                return Err("unsupported response scheme".to_owned());
            }
            if cors && !cors_allowed(&response, request_origin, credentials) {
                return Err("CORS response rejected".to_owned());
            }
            let Some(mut next) = response.take_redirect_request() else {
                break response;
            };
            if redirects == MAX_FETCH_REDIRECTS {
                return Err("too many HTTP redirects".to_owned());
            }
            redirects += 1;
            if crate::telemetry::blocked_url(&next.url) {
                return Err(crate::telemetry::diagnostic(&next.url));
            }
            if next.url.origin() != current_origin {
                // An origin change away from a foreign origin taints the
                // serialized request origin for the rest of the chain.
                tainted_origin |= document_origin.as_ref() != Some(&current_origin);
            }
            if let Some(base) = &self.base {
                let previous = redirect_site
                    .unwrap_or_else(|| crate::identity::fetch_site(base, &response.url));
                let target = crate::identity::fetch_site(base, &next.url);
                redirect_site = Some(match (previous, target) {
                    ("cross-site", _) | (_, "cross-site") => "cross-site",
                    ("same-site", _) | (_, "same-site") => "same-site",
                    _ => "same-origin",
                });
            }
            // Regenerate browser-owned metadata for the next destination.
            for name in [
                "sec-ch-ua",
                "sec-ch-ua-mobile",
                "sec-ch-ua-platform",
                "sec-fetch-site",
                "sec-fetch-mode",
                "sec-fetch-dest",
            ] {
                next.headers.remove(name);
            }
            wire = next;
        };
        let exposed = header_tokens(&response, "access-control-expose-headers");
        let headers = response
            .headers
            .iter()
            .filter(|(name, _)| {
                !matches!(name.as_str(), "set-cookie" | "set-cookie2")
                    && (!cors
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
                        || (credentials != Credentials::Include
                            && exposed.iter().any(|value| value == "*")))
            })
            .filter_map(|(name, value)| value.to_str().ok().map(|value| (name.as_str(), value)))
            .collect::<Vec<_>>();
        let bytes = response.body.to_vec();
        Ok(NetReply {
            status: response.status.as_u16(),
            status_text: response.status.canonical_reason().unwrap_or("").to_owned(),
            url: response.url,
            redirected: redirects != 0,
            headers: headers
                .into_iter()
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect(),
            body: decode_body(&bytes),
            bytes,
        })
    }
}

/// Fetch from a worker using its creator page's network policy and lifetime.
pub async fn worker_fetch(
    net: Arc<NetProvider>,
    base: Option<Url>,
    signal: blitz_traits::net::AbortSignal,
    lifetime: Arc<blitz_dom::release::Teardown>,
    request: NetRequest,
) -> Result<(serde_json::Value, Vec<u8>), String> {
    let url = Url::options()
        .base_url(base.as_ref())
        .parse(&request.url)
        .map_err(|error| error.to_string())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("worker fetch requires an HTTP URL".to_owned());
    }

    // Bound the full worker chain by the old two transport timeouts (an
    // optional preflight and the fetch), now shared by all redirect hops.
    let host = NetworkHost::new(net, base, Duration::from_secs(240), signal, lifetime);
    let reply = host
        .perform(request)
        .await
        .map_err(|error| error.to_string())?;
    if !matches!(reply.url.scheme(), "http" | "https") {
        return Err("worker response requires an HTTP URL".to_owned());
    }
    Ok((
        serde_json::json!({
            "ok": true,
            "status": reply.status,
            "statusText": reply.status_text,
            "url": reply.url.as_str(),
            "headers": reply.headers,
        }),
        reply.bytes,
    ))
}

#[cfg(test)]
mod redirect_regression {
    use super::{NetRequest, NetworkHost};
    use crate::document_loader::NetProvider;
    use blitz_traits::net::{AbortSignal, Url};
    use std::io::{self, BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, mpsc};
    use std::thread;
    use std::time::Duration;

    const BODY: &[u8] = b"redirect test";
    const ALLOW_CORS: &str = "Access-Control-Allow-Origin: *\r\n";
    const ALLOW_PREFLIGHT: &str = concat!(
        "Access-Control-Allow-Origin: *\r\n",
        "Access-Control-Allow-Methods: PUT\r\n",
        "Access-Control-Allow-Headers: x-probe\r\n",
    );

    fn read_request(stream: &mut TcpStream) -> (String, String, Vec<u8>) {
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let mut request = line.split_ascii_whitespace();
        let method = request.next().unwrap().to_owned();
        let path = request.next().unwrap().to_owned();
        let mut length = 0;
        loop {
            line.clear();
            assert_ne!(reader.read_line(&mut line).unwrap(), 0);
            if line == "\r\n" {
                break;
            }
            if let Some((name, value)) = line.split_once(':')
                && name.eq_ignore_ascii_case("content-length")
            {
                length = value.trim().parse::<usize>().unwrap();
            }
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        (method, path, body)
    }

    #[test]
    fn unsafe_redirect_preflights_target_before_replay() {
        let source = TcpListener::bind("127.0.0.1:0").unwrap();
        let target = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = Url::parse(&format!("http://{}/", source.local_addr().unwrap())).unwrap();
        let target_addr = target.local_addr().unwrap();
        source.set_nonblocking(true).unwrap();
        target.set_nonblocking(true).unwrap();
        let (stop, stopped) = mpsc::channel::<()>();
        let fixture = thread::spawn(move || {
            let mut target_requests = Vec::new();
            while matches!(stopped.try_recv(), Err(mpsc::TryRecvError::Empty)) {
                for (listener, is_target) in [(&source, false), (&target, true)] {
                    let mut stream = match listener.accept() {
                        Ok((stream, _)) => stream,
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                        Err(error) => panic!("{error}"),
                    };
                    let (method, path, body) = read_request(&mut stream);
                    if is_target {
                        let (status, headers) = match (method.as_str(), path.as_str()) {
                            ("OPTIONS", "/allow" | "/intermediate") => {
                                ("204 No Content", ALLOW_PREFLIGHT)
                            }
                            ("OPTIONS", "/preflight-redirect") => {
                                ("307 Temporary Redirect", "Location: /allow\r\n")
                            }
                            ("PUT", "/allow") | ("GET", "/rewrite") => ("200 OK", ALLOW_CORS),
                            ("PUT", "/intermediate") => {
                                ("307 Temporary Redirect", "Location: /unapproved\r\n")
                            }
                            _ => ("403 Forbidden", ""),
                        };
                        target_requests.push((method, path, body));
                        write!(
                            stream,
                            "HTTP/1.1 {status}\r\n{headers}Content-Length: 0\r\nConnection: close\r\n\r\n"
                        )
                        .unwrap();
                    } else {
                        assert_eq!(method, "PUT");
                        assert_eq!(body.as_slice(), BODY);
                        let (status, location) = match path.as_str() {
                            "/same" => ("307 Temporary Redirect", "/deny".to_owned()),
                            "/rewrite" => ("303 See Other", format!("http://{target_addr}{path}")),
                            _ => ("307 Temporary Redirect", format!("http://{target_addr}{path}")),
                        };
                        write!(
                            stream,
                            "HTTP/1.1 {status}\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        )
                        .unwrap();
                    }
                }
                thread::sleep(Duration::from_millis(2));
            }
            target_requests
        });

        let net = NetProvider::new(None);
        let navigation = net.browser_initiator(base.clone());
        let host = NetworkHost::new(
            Arc::new(net.for_document(base.clone(), &navigation)),
            Some(base),
            Duration::from_secs(5),
            AbortSignal::default(),
            blitz_dom::release::Teardown::new(),
        );
        let send = |path: &str, header: (&str, &str)| {
            nagoya::block_on(host.perform(NetRequest {
                id: 0,
                url: path.to_owned(),
                method: "PUT".to_owned(),
                headers: vec![(header.0.to_owned(), header.1.to_owned())],
                body: Some(BODY.to_vec()),
                credentials: "omit".to_owned(),
                no_cors_destination: None,
            }))
        };
        let probe = ("x-probe", "present");
        let denied = send("/same", probe);
        let preflight_redirect = send("/preflight-redirect", probe);
        let allowed = send("/allow", probe);
        let rewrite = send("/rewrite", ("content-type", "application/json"));
        let intermediate = send("/intermediate", probe);
        drop(stop);
        let target_requests = fixture.join().unwrap();

        assert_eq!(
            target_requests,
            vec![
                ("OPTIONS".to_owned(), "/deny".to_owned(), Vec::new()),
                (
                    "OPTIONS".to_owned(),
                    "/preflight-redirect".to_owned(),
                    Vec::new(),
                ),
                ("OPTIONS".to_owned(), "/allow".to_owned(), Vec::new()),
                ("PUT".to_owned(), "/allow".to_owned(), BODY.to_vec()),
                ("GET".to_owned(), "/rewrite".to_owned(), Vec::new()),
                ("OPTIONS".to_owned(), "/intermediate".to_owned(), Vec::new()),
                ("PUT".to_owned(), "/intermediate".to_owned(), BODY.to_vec()),
            ],
            "the destination must authorize the redirected PUT before receiving it"
        );
        assert!(denied.is_err());
        assert!(preflight_redirect.is_err());
        assert!(intermediate.is_err());
        for reply in [allowed.unwrap(), rewrite.unwrap()] {
            assert_eq!(reply.status, 200);
            assert!(reply.redirected);
        }
    }
}
