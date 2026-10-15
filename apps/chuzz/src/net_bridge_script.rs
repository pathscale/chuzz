use std::sync::{Arc, Mutex};
use std::time::Duration;

use blitz_dom::Document as _;
use blitz_script::boa_engine::object::builtins::{AlignedVec, JsArrayBuffer, JsUint8Array};
use blitz_script::boa_engine::{
    Context, JsNativeError, JsResult, JsString, JsValue, NativeFunction,
};
use blitz_traits::net::Url;

use crate::document_loader::NetProvider;

use super::{Delivery, Mailbox, NetReply, NetRequest, NetworkError, NetworkHost, PageWake};

fn script_host(context: &Context) -> JsResult<NetworkHost> {
    context.get_data::<NetworkHost>().cloned().ok_or_else(|| {
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

fn reply_payload(reply: Result<NetReply, NetworkError>) -> (serde_json::Value, Option<Vec<u8>>) {
    match reply {
        Ok(reply) => (
            serde_json::json!({
                "ok": true,
                "status": reply.status,
                "statusText": reply.status_text,
                "url": reply.url.as_str(),
                "headers": reply.headers,
                "body": reply.body,
            }),
            Some(reply.bytes),
        ),
        Err(error) => (
            serde_json::json!({
                "ok": false,
                "error": error.to_string(),
            }),
            None,
        ),
    }
}

/// The response as a script object. The body travels as a Uint8Array over one
/// buffer, never as source text the script parser would have to read.
fn js_response(reply: Result<NetReply, NetworkError>, context: &mut Context) -> JsResult<JsValue> {
    let (payload, bytes) = reply_payload(reply);
    let object = JsValue::from_json(&payload, context)?
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
    js_response(nagoya::block_on(host.perform(request)), context)
}

fn cookie_get(_: &JsValue, _: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = script_host(context)?;
    Ok(JsString::from(host.cookie_get()).into())
}

fn cookie_set(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = script_host(context)?;
    let assignment = argument_string(args, context)?;
    host.cookie_set(&assignment)
        .map(JsString::from)
        .map(Into::into)
        .map_err(|error| JsNativeError::typ().with_message(error.to_string()).into())
}

pub(crate) async fn worker_fetch(
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

    let host = NetworkHost::new(net, base, Duration::ZERO, signal, lifetime);
    let reply = host
        .perform_unbounded(request)
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

pub(super) fn install(
    document: &mut blitz_script::ScriptDocument,
    net: Arc<NetProvider>,
    deadline: Duration,
    base: Option<Url>,
) {
    let handler_net = Arc::clone(&net);
    let abort = blitz_traits::net::AbortController::default();
    let signal = abort.signal.clone();
    document.add_teardown_hook(move || abort.abort());
    let host = NetworkHost::new(net, base, deadline, signal, document.teardown_receipt());
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
    let sockets =
        crate::ws_bridge::Bridge::with_lifetime(wake.clone(), document.teardown_receipt())
            .with_initiator(host.net.initiator());
    let retiring_sockets = sockets.clone();
    let retiring_wake = wake.clone();
    document.add_teardown_hook(move || {
        retiring_sockets.retire();
        retiring_wake.retire();
    });
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
            let reply = host.perform(request).await;
            mailbox.post(Delivery { id, reply });
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
                let result = match js_response(delivery.reply, context) {
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
