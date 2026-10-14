//! Page networking, cookie access and completion wakeups shared by both hosts.


use std::sync::{Arc, Mutex};
use std::task::Waker;
use std::time::Duration;

use blitz_dom::Document as _;
use blitz_script::boa_engine::object::builtins::{AlignedVec, JsArrayBuffer, JsUint8Array};
use blitz_script::boa_engine::{
    Context, JsNativeError, JsResult, JsString, JsValue, NativeFunction,
};
use blitz_traits::net::Url;
use blitz_traits::platform::{
    FetchError, FetchRequest, FetchResponse,
};
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
            if state.retired { return; }
            (state.waker.clone(), state.shell.clone())
        };
        if let Some(waker) = waker {
            waker.wake();
        } else if let Some(shell) = shell {
            shell.request_redraw();
        }
    }
}

struct Delivery {
    id: u64,
    payload: serde_json::Value,
    bytes: Option<Vec<u8>>,
}

#[derive(Clone)]
struct Mailbox {
    entries: Arc<Mutex<Vec<Delivery>>>,
    wake: PageWake,
}

impl Mailbox {
    fn post(&self, delivery: Delivery) {
        self.entries
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(delivery);
        self.wake.wake();
    }

    fn drain(&self) -> Vec<Delivery> {
        std::mem::take(
            &mut *self
                .entries
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()),
        )
    }
}

#[derive(serde::Deserialize)]
struct NetRequest {
    #[serde(default)]
    id: u64,
    url: String,
    #[serde(default = "get_method")]
    method: String,
    #[serde(default)]
    headers: Vec<(String, String)>,
    #[serde(default)]
    body: Option<Vec<u8>>,
    #[serde(default = "default_credentials")]
    credentials: String,
}

fn get_method() -> String {
    "GET".to_owned()
}

fn default_credentials() -> String {
    "same-origin".to_owned()
}

async fn fetch_response_owned(provider: &NetProvider, request: FetchRequest, lifetime: &Arc<blitz_dom::release::Teardown>) -> Result<FetchResponse, FetchError> {
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
struct ScriptHost {
    net: Arc<NetProvider>,

    base: Option<Url>,
    deadline: Duration,
    signal: blitz_traits::net::AbortSignal,
    lifetime: Arc<blitz_dom::release::Teardown>,
}

fn failure(message: impl std::fmt::Display) -> serde_json::Value {
    serde_json::json!({"ok": false, "error": message.to_string()})
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

impl ScriptHost {
    async fn perform(&self, request: NetRequest) -> (serde_json::Value, Option<Vec<u8>>) {
        let _lease = self.lifetime.lease();
        match nagoya::timeout(self.deadline, self.perform_inner(request)).await {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => (failure(error), None),
            Err(_) => (failure("timed out"), None),
        }
    }

    async fn perform_inner(
        &self,
        request: NetRequest,
    ) -> Result<(serde_json::Value, Option<Vec<u8>>), String> {
        use blitz_traits::platform::http::{HeaderName, HeaderValue, Method};

        let mut url = Url::options()
            .base_url(self.base.as_ref())
            .parse(&request.url)
            .map_err(|error| error.to_string())?;
        if !matches!(url.scheme(), "http" | "https" | "file" | "data") {
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
        let mut wire = FetchRequest::get(url.clone()).method(method.clone()).signal(self.signal.clone());
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
            .filter(|base| base.origin().is_tuple())
            .map(|base| base.origin().ascii_serialization());
        let cross_origin = matches!(url.scheme(), "http" | "https")
            && self
                .base
                .as_ref()
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
                let mut preflight = FetchRequest::get(url.clone()).method(Method::OPTIONS).signal(self.signal.clone());
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
        let response_cross_origin = matches!(response.url.scheme(), "http" | "https")
            && self
                .base
                .as_ref()
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
        Ok((
            serde_json::json!({
                "ok": true,
                "status": response.status.as_u16(),
                "statusText": response.status.canonical_reason().unwrap_or(""),
                "url": response.url.as_str(),
                "headers": headers,
                "body": decode_body(&bytes),
            }),
            Some(bytes),
        ))
    }
}

fn script_host(context: &Context) -> JsResult<ScriptHost> {
    context.get_data::<ScriptHost>().cloned().ok_or_else(|| {
        JsNativeError::typ()
            .with_message("page network host is unavailable")
            .into()
    })
}

fn argument_string(args: &[JsValue], context: &mut Context) -> JsResult<String> {
    Ok(args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped())
}

/// The response as a script object. The body travels as a Uint8Array over one
/// buffer, never as source text the script parser would have to read.
fn js_response(
    payload: &serde_json::Value,
    bytes: Option<Vec<u8>>,
    context: &mut Context,
) -> JsResult<JsValue> {
    let object = JsValue::from_json(payload, context)?
        .as_object()
        .ok_or_else(|| JsNativeError::typ().with_message("network response is not an object"))?;
    if let Some(bytes) = bytes {
        let buffer = JsArrayBuffer::from_byte_block(AlignedVec::from_iter(0, bytes), context)?;
        let bytes = JsUint8Array::from_array_buffer(buffer, context)?;
        object.set(JsString::from("bytes"), bytes, true, context)?;
    }
    Ok(object.into())
}

fn synchronous_request(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = script_host(context)?;
    let text = argument_string(args, context)?;
    let request = serde_json::from_str::<NetRequest>(&text)
        .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
    let (payload, bytes) = nagoya::block_on(host.perform(request));
    js_response(&payload, bytes, context)
}

fn cookie_get(_: &JsValue, _: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = script_host(context)?;
    let value = host
        .base
        .as_ref()
        .map(|url| host.net.cookie_store().script_cookies(url))
        .unwrap_or_default();
    Ok(JsString::from(value).into())
}

fn cookie_set(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = script_host(context)?;
    let assignment = argument_string(args, context)?;
    if let Some(url) = &host.base {
        host.net
            .cookie_store()
            .set_script_cookie(url, &assignment)
            .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
    }
    cookie_get(&JsValue::undefined(), &[], context)
}

pub fn install(
    document: &mut blitz_script::ScriptDocument,
    net: Arc<NetProvider>,
    deadline: Duration,
    base: Option<Url>,
) {
    let handler_net = Arc::clone(&net);
    let abort = blitz_traits::net::AbortController::default();
    let signal = abort.signal.clone();
    document.add_teardown_hook(move || abort.abort());
    let host = ScriptHost {

        net,
        base,
        deadline,
        signal,
        lifetime: document.teardown_receipt(),
    };
    document.with_js_context(|context| {
        context.insert_data(host.clone());
        for (name, length, function) in [
            (
                "__chuzzNetSync",
                1,
                synchronous_request as fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>,
            ),
            ("__chuzzCookieGet", 0, cookie_get),
            ("__chuzzCookieSet", 1, cookie_set),
        ] {
            context
                .register_global_builtin_callable(
                    JsString::from(name),
                    length,
                    NativeFunction::from_fn_ptr(function),
                )
                .expect("page network native bridge");
        }
    });
    document.eval_named(include_str!("network.ts"), "<chuzz-network>");
    document.eval(crate::ws_bridge::WEBSOCKET_SHIM);

    let wake = PageWake::new(Arc::clone(&document.inner().shell_provider));
    let gpu = crate::webgpu::Bridge::new(wake.clone());
    gpu.install_inspector(document);
    gpu.install_bytes(document);
    gpu.install_canvas_readback(document);
    document.eval_named(crate::webgpu::WEBGPU_SHIM, "<chuzz-webgpu>");

    let mailbox = Mailbox {
        entries: Arc::new(Mutex::new(Vec::new())),
        wake: wake.clone(),
    };
    let handler_mailbox = mailbox.clone();
    let sockets = crate::ws_bridge::Bridge::with_lifetime(wake.clone(), document.teardown_receipt());
    let retiring_sockets = sockets.clone();
    let retiring_wake = wake.clone();
    document.add_teardown_hook(move || { retiring_sockets.retire(); retiring_wake.retire(); });
    let handler_sockets = sockets.clone();
    let handler_gpu = gpu.clone();
    let initial_scroll = document.inner().viewport_scroll();
    let mut last_scroll = (initial_scroll.x, initial_scroll.y);

    document.set_ipc_handler(move |message| {
        if handler_sockets.handle(&message) || handler_gpu.handle(&message) {
            return;
        }
        let Ok(request) = serde_json::from_str::<NetRequest>(&message) else {
            return;
        };
        let host = host.clone();
        let mailbox = handler_mailbox.clone();
        let id = request.id;
        drop(nagoya::spawn(async move {
            let (payload, bytes) = host.perform(request).await;
            mailbox.post(Delivery { id, payload, bytes });
        }));
    });

    document.add_poll_hook(move |document, task_context| {
        wake.bind(task_context.map(|context| context.waker()));
        let mut changed = sockets.drain_into(document) | gpu.drain_into(document);
        for message in handler_net.take_blocked_requests() {
            if let Ok(encoded) = serde_json::to_string(&message) {
                document.eval(&format!("globalThis.console.warn({encoded});"));
                changed = true;
            }
        }
        for delivery in mailbox.drain() {
            document.with_js_context(|context| {
                let resolve = context
                    .global_object()
                    .get(JsString::from("__chuzzNetResolve"), context)
                    .ok()
                    .and_then(|value| value.as_object())
                    .filter(|function| function.is_callable());
                let Some(resolve) = resolve else {
                    eprintln!("Network resolve hook is unavailable");
                    return;
                };
                let result = match js_response(&delivery.payload, delivery.bytes, context) {
                    Ok(result) => result,
                    Err(error) => {
                        eprintln!("Network response: {error}");
                        return;
                    }
                };
                if let Err(error) = resolve.call(
                    &JsValue::undefined(),
                    &[JsValue::from(delivery.id as f64), result],
                    context,
                ) {
                    eprintln!("Network response callback: {error}");
                }
                if let Err(error) = context.run_jobs() {
                    eprintln!("Network response jobs: {error}");
                }
            });
            changed = true;
        }
        // The viewport moved (scrollTo from script, wheel, keys, automation):
        // fire one scroll event per poll, at the document, bubbling to the
        // window, as browsers do once per frame. scrollX/scrollY read the
        // native position directly, so nothing needs mirroring.
        let position = document.inner().viewport_scroll();
        if (position.x, position.y) != last_scroll {
            document.eval(
                "if (typeof globalThis.Event === 'function') globalThis.document.dispatchEvent(new Event('scroll', { bubbles: true }));",
            );
            last_scroll = (position.x, position.y);
            changed = true;
        }
        changed
    });

    // Image discovery can reserve requests during shim installation. Release
    // them only after IPC admission and reply polling are both installed.
    document.eval(
        "if (globalThis.__chuzzGpuImageBridge) globalThis.__chuzzGpuImageBridge.imagesReady();",
    );
}
