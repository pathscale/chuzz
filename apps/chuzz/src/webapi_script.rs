use std::sync::Arc;

use blitz_script::boa_engine::object::builtins::{AlignedVec, JsArrayBuffer};
use blitz_script::boa_engine::{
    Context, JsNativeError, JsResult, JsString, JsValue, NativeFunction, Source,
};
use blitz_traits::net::Url;

use super::{InfoReply, LIMIT, Operation, Realm, Reply};

#[derive(Clone)]
struct Host(Arc<Realm>);

fn text(args: &[JsValue], index: usize, context: &mut Context) -> JsResult<String> {
    Ok(args
        .get(index)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped())
}

fn native_error(message: impl std::fmt::Display) -> blitz_script::boa_engine::JsError {
    JsNativeError::typ()
        .with_message(message.to_string())
        .into()
}

fn platform(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = context
        .get_data::<Host>()
        .cloned()
        .ok_or_else(|| native_error("web API realm is unavailable"))?;
    let description = text(args, 0, context)?;
    if description.len() > LIMIT {
        return Err(native_error("web API message exceeds the allocation limit"));
    }
    let operation: Operation = serde_json::from_str(&description).map_err(native_error)?;
    let reply = super::dispatch(&host.0, operation).map_err(native_error)?;
    JsValue::from_json(&reply_value(reply), context)
}

fn reply_value(reply: Reply) -> serde_json::Value {
    match reply {
        Reply::Info(InfoReply {
            origin,
            secure,
            platform,
            ua_platform,
            architecture,
            bitness,
            concurrency,
            version,
            languages,
        }) => serde_json::json!({
            "origin": origin,
            "secure": secure,
            "platform": platform,
            "uaPlatform": ua_platform,
            "architecture": architecture,
            "bitness": bitness,
            "concurrency": concurrency,
            "version": version,
            "languages": languages,
        }),
        Reply::Random(bytes) => serde_json::json!(bytes),
        Reply::Id(id) => serde_json::json!(id),
        Reply::Null => serde_json::Value::Null,
        Reply::BroadcastDeliveries(deliveries) => serde_json::Value::Array(
            deliveries
                .into_iter()
                .map(|delivery| {
                    serde_json::json!({
                        "id": delivery.id,
                        "wire": delivery.wire.as_ref(),
                    })
                })
                .collect(),
        ),
        Reply::LockQuery { held, pending } => {
            let entries = |entries: Vec<super::LockEntry>| {
                entries
                    .into_iter()
                    .map(|entry| {
                        serde_json::json!({
                            "name": entry.name,
                            "mode": entry.mode,
                            "clientId": entry.client_id,
                        })
                    })
                    .collect::<Vec<_>>()
            };
            serde_json::json!({
                "held": entries(held),
                "pending": entries(pending),
            })
        }
        Reply::Boolean(value) => serde_json::json!(value),
        Reply::Webauthn(value) => value,
    }
}

fn codec(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let format = text(args, 0, context)?;
    if !matches!(format.as_str(), "gzip" | "deflate" | "deflate-raw") {
        return Err(native_error("unsupported compression format"));
    }
    let object = args
        .get(1)
        .and_then(JsValue::as_object)
        .ok_or_else(|| native_error("compression requires an ArrayBuffer"))?;
    let input = JsArrayBuffer::from_object(object)?
        .to_vec()
        .ok_or_else(|| native_error("detached compression input"))?;
    let decode = args.get(2).is_some_and(JsValue::to_boolean);
    let output = super::compress(&format, &input, decode).map_err(native_error)?;
    Ok(JsArrayBuffer::from_byte_block(AlignedVec::from_iter(0, output), context)?.into())
}

pub(crate) fn notifications(context: &Context) -> Option<Arc<crate::webauthn::Notifications>> {
    context
        .get_data::<Host>()
        .map(|host| Arc::clone(&host.0.notifications))
}

pub(crate) fn drain_notifications(context: &mut Context) -> bool {
    let Some(host) = context.get_data::<Host>().cloned() else {
        return false;
    };
    let deliveries = host.0.notifications.take(host.0.id);
    let changed = !deliveries.is_empty();
    for (id, result) in deliveries {
        let delivery = (|| -> JsResult<()> {
            let callback = context
                .global_object()
                .get(JsString::from("__chuzzWebauthnResolve"), context)?
                .as_object()
                .filter(|function| function.is_callable())
                .ok_or_else(|| native_error("WebAuthn completion hook unavailable"))?;
            let value = JsValue::from_json(&result, context)?;
            callback.call(
                &JsValue::undefined(),
                &[JsValue::from(id as f64), value],
                context,
            )?;
            context.run_jobs()
        })();
        if let Err(error) = delivery {
            eprintln!("WebAuthn completion: {error}");
        }
    }
    changed
}

pub(crate) fn install_context(context: &mut Context, url: Option<&Url>) -> JsResult<()> {
    install_context_with_wake(context, url, crate::net_bridge::PageWake::default())
}

pub(super) fn install_context_with_wake(
    context: &mut Context,
    url: Option<&Url>,
    wake: crate::net_bridge::PageWake,
) -> JsResult<()> {
    let realm = super::new_realm(url, wake);
    context.insert_data(Host(realm));
    for (name, length, function) in [
        (
            "__chuzzPlatformCall",
            1,
            platform as fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>,
        ),
        ("__chuzzCodec", 3, codec),
    ] {
        context.register_global_builtin_callable(
            JsString::from(name),
            length,
            NativeFunction::from_fn_ptr(function),
        )?;
    }
    context.eval(Source::from_bytes(include_str!("platform/common.ts")))?;
    Ok(())
}

pub(crate) fn install(document: &mut blitz_script::ScriptDocument) {
    use blitz_dom::Document as _;
    let url = document.current_url().cloned();
    let wake = crate::net_bridge::PageWake::new(Arc::clone(&document.inner().shell_provider));
    document.with_js_context(|context| {
        install_context_with_wake(context, url.as_ref(), wake).expect("web API native bootstrap");
        for source in [
            include_str!("platform/streams.ts"),
            include_str!("platform/body.ts"),
            include_str!("platform/window.ts"),
        ] {
            context
                .eval(Source::from_bytes(source))
                .expect("web API page bootstrap");
        }
    });
    document.add_poll_hook(|document, task_context| {
        document.with_js_context(|context| {
            if let Some(notifications) = notifications(context) {
                notifications.bind(task_context.map(|context| context.waker()));
            }
            drain_notifications(context)
        })
    });
}
