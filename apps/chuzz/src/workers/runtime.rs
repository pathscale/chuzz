//! The worker event loop. No document, window or DOM runtime is constructed.

use std::collections::VecDeque;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use blitz_script::boa_engine::builtins::promise::PromiseState;
use blitz_script::boa_engine::object::builtins::{AlignedVec, JsArrayBuffer};
use blitz_script::boa_engine::{
    Context, JsError, JsNativeError, JsResult, JsString, JsValue, Module, Source,
};
use blitz_script::boa_runtime;
use blitz_traits::net::http::header::{HeaderName, HeaderValue};
use blitz_traits::net::{Body, Method, Request, Url};

use crate::document_loader::NetProvider;

use super::modules::Loader;
use super::{
    DEADLINE, Endpoint, Input, Output, Page, argument, call, cancellable, clone, register, string,
};

#[derive(Clone)]
struct Host {
    endpoint: Arc<Endpoint>,
    page: Weak<Page>,
    net: Arc<NetProvider>,
    loader: Rc<Loader>,
    url: Url,
    module: bool,
    started: Instant,
}

fn host(context: &Context) -> JsResult<Host> {
    context.get_data::<Host>().cloned().ok_or_else(|| {
        JsNativeError::typ()
            .with_message("worker scope is unavailable")
            .into()
    })
}

fn post(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let wire = clone::encode(argument(args, 0), argument(args, 1), context)?;
    host.endpoint.post(&host.page, Output::Message(wire));
    Ok(JsValue::undefined())
}

fn close(_: &JsValue, _: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    // close() finishes the current task and its microtask checkpoint.
    // terminate() instead sets the VM's interruption flag.
    host.endpoint.closing.store(true, Ordering::Release);
    host.endpoint.inbox.wake();
    Ok(JsValue::undefined())
}

fn now(_: &JsValue, _: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(
        host(context)?.started.elapsed().as_secs_f64() * 1000.0,
    ))
}

fn report_value(host: &Host, value: JsValue, context: &mut Context) {
    if host.endpoint.canceled.load(Ordering::Acquire) {
        return;
    }
    let message = value
        .to_string(context)
        .map(|value| value.to_std_string_escaped())
        .unwrap_or_else(|_| "worker threw".to_owned());
    let mut name = "Error".to_owned();
    let mut stack = String::new();
    let mut lineno = 0.0;
    let mut colno = 0.0;
    if let Some(object) = value.as_object() {
        if let Ok(value) = object.get(JsString::from("name"), context)
            && let Ok(value) = string(&value, context)
        {
            name = value;
        }
        if let Ok(value) = object.get(JsString::from("stack"), context)
            && let Ok(value) = string(&value, context)
        {
            stack = value;
        }
        lineno = object
            .get(JsString::from("lineNumber"), context)
            .ok()
            .and_then(|value| value.as_number())
            .unwrap_or_default();
        colno = object
            .get(JsString::from("columnNumber"), context)
            .ok()
            .and_then(|value| value.as_number())
            .unwrap_or_default();
    }
    host.endpoint.post(
        &host.page,
        Output::Error(serde_json::json!({
            "message": message,
            "filename": host.url.as_str(),
            "lineno": lineno,
            "colno": colno,
            "name": name,
            "stack": stack,
        })),
    );
}

fn report(host: &Host, error: JsError, context: &mut Context) {
    if !host.endpoint.canceled.load(Ordering::Acquire) {
        let value = error.into_opaque(context).unwrap_or_default();
        report_value(host, value, context);
    }
}

fn report_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    report_value(&host, argument(args, 0), context);
    Ok(JsValue::undefined())
}

fn import_scripts(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    if host.module {
        return Err(JsNativeError::typ()
            .with_message("importScripts is unavailable in module workers")
            .into());
    }
    let addresses = args
        .iter()
        .map(|value| string(value, context))
        .collect::<JsResult<Vec<_>>>()?;
    let urls = addresses
        .iter()
        .map(|address| {
            host.url
                .join(address)
                .map_err(|error| JsNativeError::typ().with_message(error.to_string()).into())
        })
        .collect::<JsResult<Vec<_>>>()?;
    for url in urls {
        let (resolved, source) = host.loader.source(&url)?;
        let path = resolved.to_string();
        context.eval(Source::from_bytes(source.as_bytes()).with_path(Path::new(&path)))?;
    }
    Ok(JsValue::undefined())
}

#[derive(serde::Deserialize)]
struct FetchOptions {
    id: u64,
    url: String,
    method: String,
    headers: Vec<(String, String)>,
}

fn fetch(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let description = string(&argument(args, 0), context)?;
    let options: FetchOptions = serde_json::from_str(&description)
        .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
    let url = host
        .url
        .join(&options.url)
        .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(JsNativeError::typ()
            .with_message("worker fetch requires an HTTP URL")
            .into());
    }
    let mut request = Request::get(url);
    request.method = Method::from_bytes(options.method.as_bytes())
        .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
    for (name, value) in options.headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
        let value = HeaderValue::from_str(&value)
            .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
        request.headers.append(name, value);
    }
    let body = argument(args, 1);
    if let Some(object) = body.as_object() {
        let buffer = JsArrayBuffer::from_object(object)?;
        let bytes = buffer
            .to_vec()
            .ok_or_else(|| JsNativeError::typ().with_message("detached request body"))?;
        request.body = Body::Bytes(bytes.into());
    }
    let endpoint = Arc::clone(&host.endpoint);
    let net = Arc::clone(&host.net);
    let _ = nagoya::spawn(async move {
        let result = cancellable(
            &endpoint,
            nagoya::timeout(DEADLINE, net.fetch_response_async(request)),
        )
        .await;
        if endpoint.stopped() || endpoint.closing.load(Ordering::Acquire) {
            return;
        }
        let (metadata, body) = match result {
            Ok(Ok(Ok(response))) => {
                let headers = response
                    .headers
                    .iter()
                    .map(|(name, value)| {
                        (
                            name.as_str().to_owned(),
                            value.to_str().unwrap_or_default().to_owned(),
                        )
                    })
                    .collect::<Vec<_>>();
                (
                    serde_json::json!({
                        "ok": true,
                        "status": response.status.as_u16(),
                        "statusText": response.status.canonical_reason().unwrap_or_default(),
                        "url": response.url.as_str(),
                        "headers": headers,
                    }),
                    AlignedVec::from_iter(0, response.body.iter().copied()),
                )
            }
            Ok(Ok(Err(error))) => (
                serde_json::json!({"ok": false, "error": format!("{error:?}")}),
                AlignedVec::from_iter(0, []),
            ),
            Ok(Err(_)) => (
                serde_json::json!({"ok": false, "error": "fetch timed out"}),
                AlignedVec::from_iter(0, []),
            ),
            Err(error) => (
                serde_json::json!({"ok": false, "error": error}),
                AlignedVec::from_iter(0, []),
            ),
        };
        endpoint.inbox.push(Input::Fetch {
            id: options.id,
            metadata: metadata.to_string(),
            body,
        });
    });
    Ok(JsValue::undefined())
}

fn checkpoint(host: &Host, context: &mut Context) {
    if let Err(error) = context.run_jobs() {
        report(host, error, context);
    }
}

fn dispatch(host: &Host, wire: clone::Wire, context: &mut Context) {
    let result = match clone::decode(wire, context) {
        Ok(value) => call(context, "__chuzzWorkerMessage", &[value]),
        Err(_) => call(context, "__chuzzWorkerMessageError", &[]),
    };
    if let Err(error) = result {
        report(host, error, context);
    }
    checkpoint(host, context);
}

pub(super) fn run(
    endpoint: Arc<Endpoint>,
    page: Weak<Page>,
    net: Arc<NetProvider>,
    url: Url,
    module: bool,
    name: String,
) {
    if endpoint.stopped() {
        return;
    }
    let loader = Rc::new(Loader::new(
        Arc::clone(&net),
        Arc::clone(&endpoint),
        url.clone(),
    ));
    let mut context = match Context::builder().module_loader(Rc::clone(&loader)).build() {
        Ok(context) => context,
        Err(error) => {
            endpoint.post(
                &page,
                Output::Error(serde_json::json!({
                    "message": error.to_string(),
                    "filename": url.as_str(),
                    "lineno": 0,
                    "colno": 0,
                })),
            );
            return;
        }
    };
    context.set_interrupt_flag(Arc::clone(&endpoint.canceled));
    context.runtime_limits_mut().set_recursion_limit(8192);
    context
        .runtime_limits_mut()
        .set_stack_size_limit(1024 * 100);
    let host = Host {
        endpoint: Arc::clone(&endpoint),
        page,
        net,
        loader: Rc::clone(&loader),
        url: url.clone(),
        module,
        started: Instant::now(),
    };
    context.insert_data(host.clone());

    let bootstrap = (|| -> JsResult<()> {
        boa_runtime::Console::register_with_logger(boa_runtime::DefaultLogger, &mut context)?;
        boa_runtime::register_extensions(
            (
                boa_runtime::extensions::UrlExtension,
                boa_runtime::extensions::EncodingExtension,
            ),
            None,
            &mut context,
        )?;
        context.eval(Source::from_bytes(include_str!("events.ts")))?;
        clone::install(&mut context)?;
        register(&mut context, "__chuzzWorkerSend", 2, post)?;
        register(&mut context, "__chuzzWorkerClose", 0, close)?;
        register(&mut context, "__chuzzWorkerNow", 0, now)?;
        register(&mut context, "__chuzzWorkerImport", 0, import_scripts)?;
        register(&mut context, "__chuzzWorkerFetch", 2, fetch)?;
        register(&mut context, "__chuzzWorkerReport", 1, report_native)?;
        context.eval(Source::from_bytes(include_str!("globals.ts")))?;
        call(
            &mut context,
            "__chuzzWorkerInitialize",
            &[
                JsString::from(name).into(),
                JsString::from(url.as_str()).into(),
            ],
        )?;
        Ok(())
    })();
    if let Err(error) = bootstrap {
        report(&host, error, &mut context);
        return;
    }

    let (resolved, source) = match loader.source(&url) {
        Ok(source) => source,
        Err(error) => {
            report(&host, error, &mut context);
            return;
        }
    };
    if url.scheme() != "data" && resolved.origin() != url.origin() {
        report(
            &host,
            JsNativeError::typ()
                .with_message("worker redirected across origins")
                .into(),
            &mut context,
        );
        return;
    }
    let path = resolved.to_string();
    let mut startup = if module {
        let module = match Module::parse(
            Source::from_bytes(source.as_bytes()).with_path(Path::new(&path)),
            None,
            &mut context,
        ) {
            Ok(module) => module,
            Err(error) => {
                report(&host, error, &mut context);
                return;
            }
        };
        loader.register(&url, module.clone());
        loader.register(&resolved, module.clone());
        Some(module.load_link_evaluate(&mut context))
    } else {
        if let Err(error) =
            context.eval(Source::from_bytes(source.as_bytes()).with_path(Path::new(&path)))
        {
            report(&host, error, &mut context);
            return;
        }
        None
    };
    checkpoint(&host, &mut context);
    let mut deferred = VecDeque::new();

    while !endpoint.stopped() && !endpoint.closing.load(Ordering::Acquire) {
        if let Some(promise) = &startup {
            match promise.state() {
                PromiseState::Pending => {}
                PromiseState::Fulfilled(_) => startup = None,
                PromiseState::Rejected(value) => {
                    report_value(&host, value, &mut context);
                    break;
                }
            }
        }
        if startup.is_none()
            && let Some(wire) = deferred.pop_front()
        {
            dispatch(&host, wire, &mut context);
            continue;
        }

        let delay = call(&mut context, "__chuzzWorkerNextTimer", &[])
            .ok()
            .and_then(|value| value.as_number())
            .filter(|delay| delay.is_finite())
            .unwrap_or(1000.0)
            .clamp(0.0, 1000.0);
        let input = nagoya::block_on(nagoya::timeout(
            Duration::from_secs_f64(delay / 1000.0),
            endpoint.inbox.recv(),
        ));
        if endpoint.stopped() {
            break;
        }
        match input {
            Ok(Input::Message(wire)) if startup.is_some() => deferred.push_back(wire),
            Ok(Input::Message(wire)) => dispatch(&host, wire, &mut context),
            Ok(Input::Fetch { id, metadata, body }) => {
                let result = (|| -> JsResult<JsValue> {
                    let body = JsArrayBuffer::from_byte_block(body, &mut context)?;
                    call(
                        &mut context,
                        "__chuzzWorkerFetchResult",
                        &[
                            JsValue::from(id as f64),
                            JsString::from(metadata).into(),
                            body.into(),
                        ],
                    )
                })();
                if let Err(error) = result {
                    report(&host, error, &mut context);
                }
                checkpoint(&host, &mut context);
            }
            Err(_) => {}
        }
        if endpoint.stopped() || endpoint.closing.load(Ordering::Acquire) {
            break;
        }
        if let Err(error) = call(&mut context, "__chuzzWorkerRunTimer", &[]) {
            report(&host, error, &mut context);
        }
        checkpoint(&host, &mut context);
        context.clear_kept_objects();
    }
}
