use std::sync::Arc;

use blitz_script::ScriptDocument;
use blitz_script::boa_engine::object::builtins::JsArrayBuffer;
use blitz_script::boa_engine::{
    Context, JsNativeError, JsResult, JsString, JsValue, NativeFunction,
};
use serde_json::json;

use super::{DOCUMENT_LIMIT, Error, Host, MAX_ID, Request, UPLOAD_LIMIT};

struct NativeHost(Arc<Host>);

impl Drop for NativeHost {
    fn drop(&mut self) {
        if let Ok(store) = self.0.store.lock() {
            for source in store.sources.values() {
                if let Ok(mut source) = source.lock() {
                    source.closed = true;
                }
            }
        }
    }
}

fn host(context: &Context) -> JsResult<Arc<Host>> {
    context
        .get_data::<NativeHost>()
        .map(|host| Arc::clone(&host.0))
        .ok_or_else(|| {
            JsNativeError::typ()
                .with_message("MSE host is unavailable")
                .into()
        })
}

fn integer(value: Option<&JsValue>, context: &mut Context) -> JsResult<u64> {
    let value = value.cloned().unwrap_or_default().to_number(context)?;
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 || value > MAX_ID as f64 {
        return Err(JsNativeError::typ()
            .with_message("invalid MSE byte range")
            .into());
    }
    Ok(value as u64)
}

fn put(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let offset = integer(args.get(1), context)?;
    let length = integer(args.get(2), context)?;
    let object = args
        .first()
        .and_then(JsValue::as_object)
        .ok_or_else(|| JsNativeError::typ().with_message("expected an ArrayBuffer"))?;
    let buffer = JsArrayBuffer::from_object(object)?;
    let bytes = buffer
        .data()
        .ok_or_else(|| JsNativeError::typ().with_message("detached MSE appendBuffer"))?;
    let end = offset
        .checked_add(length)
        .filter(|end| *end <= bytes.len() as u64)
        .ok_or_else(|| JsNativeError::typ().with_message("MSE byte range is out of bounds"))?;
    let mut store = host
        .store
        .lock()
        .map_err(|_| JsNativeError::typ().with_message("MSE store was poisoned"))?;
    if length > UPLOAD_LIMIT as u64
        || store.uploads.len() >= 128
        || store.memory().saturating_add(length as usize) > DOCUMENT_LIMIT
        || store.next_upload >= MAX_ID
    {
        return Err(JsNativeError::typ()
            .with_message("MSE quota exceeded")
            .into());
    }
    store.next_upload += 1;
    let id = store.next_upload;
    store
        .uploads
        .insert(id, Arc::new(bytes[offset as usize..end as usize].to_vec()));
    Ok(JsValue::from(id as f64))
}

fn release(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let id = integer(args.first(), context)?;
    host.store
        .lock()
        .map_err(|_| JsNativeError::typ().with_message("MSE store was poisoned"))?
        .uploads
        .remove(&id);
    Ok(JsValue::undefined())
}

fn control(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = host(context)?;
    let text = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let result = serde_json::from_str::<Request>(&text)
        .map_err(|_| Error::state("invalid MSE command"))
        .and_then(|request| host.control(request));
    let reply = match result {
        Ok(value) => json!({ "ok": true, "value": value }),
        Err(error) => json!({ "ok": false, "error": error }),
    };
    Ok(JsString::from(reply.to_string()).into())
}

pub(crate) fn install(document: &mut ScriptDocument) -> Arc<Host> {
    let host = Arc::new(Host::default());
    document.with_js_context(|context| {
        context.insert_data(NativeHost(Arc::clone(&host)));
        type Callback = fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>;
        let functions: [(&str, usize, Callback); 3] = [
            ("__chuzzMsePut", 3, put),
            ("__chuzzMseRelease", 1, release),
            ("__chuzzMseControl", 1, control),
        ];
        for (name, length, callback) in functions {
            context
                .register_global_builtin_callable(
                    JsString::from(name),
                    length,
                    NativeFunction::from_fn_ptr(callback),
                )
                .expect("MSE native bridge");
        }
    });
    host
}
