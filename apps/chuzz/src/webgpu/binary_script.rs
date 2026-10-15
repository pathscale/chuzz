use std::sync::Arc;

use blitz_script::boa_engine::object::builtins::{AlignedVec, JsArray, JsArrayBuffer};
use blitz_script::boa_engine::{
    Context, JsNativeError, JsResult, JsString, JsValue, NativeFunction,
};

use super::super::{Bridge, GpuError};
use super::{MAX_ID, RESOURCE_LIMIT, Table};

struct NativeHost {
    bridge: Bridge,
    table: Arc<Table>,
}

impl Drop for NativeHost {
    fn drop(&mut self) {
        // Worker clones of Bridge must not prolong a departed page's bytes.
        self.table.close();
    }
}

fn native_error(error: GpuError) -> blitz_script::boa_engine::JsError {
    JsNativeError::typ().with_message(error.message).into()
}

fn table(context: &Context) -> JsResult<Arc<Table>> {
    context
        .get_data::<NativeHost>()
        .map(|host| Arc::clone(&host.table))
        .ok_or_else(|| {
            JsNativeError::typ()
                .with_message("WebGPU byte host is unavailable")
                .into()
        })
}

fn integer(args: &[JsValue], index: usize, context: &mut Context) -> JsResult<u64> {
    let value = args
        .get(index)
        .cloned()
        .unwrap_or_default()
        .to_number(context)?;
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 || value > MAX_ID as f64 {
        return Err(JsNativeError::typ()
            .with_message("invalid WebGPU byte range")
            .into());
    }
    Ok(value as u64)
}

fn put(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let table = table(context)?;
    let offset = integer(args, 1, context)?;
    let length = integer(args, 2, context)?;
    let object = args
        .first()
        .and_then(JsValue::as_object)
        .ok_or_else(|| JsNativeError::typ().with_message("expected an ArrayBuffer"))?;
    let buffer = JsArrayBuffer::from_object(object)?;
    let data = buffer
        .data()
        .ok_or_else(|| JsNativeError::typ().with_message("detached WebGPU upload"))?;
    let end = offset
        .checked_add(length)
        .filter(|end| *end <= data.len() as u64)
        .ok_or_else(|| JsNativeError::typ().with_message("WebGPU data range is out of bounds"))?;
    let id = table
        .upload(&data[offset as usize..end as usize])
        .map_err(native_error)?;
    Ok(JsValue::from(id as f64))
}

fn take(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let table = table(context)?;
    let id = integer(args, 0, context)?;
    let block = table.take_output(id).map_err(native_error)?;
    let bytes = AlignedVec::from_iter(0, block.data().iter().copied());
    Ok(JsArrayBuffer::from_byte_block(bytes, context)?.into())
}

fn release(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let table = table(context)?;
    let id = integer(args, 0, context)?;
    table.release_staged(id);
    Ok(JsValue::undefined())
}

fn send(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let bridge = context
        .get_data::<NativeHost>()
        .map(|host| host.bridge.clone())
        .ok_or_else(|| JsNativeError::typ().with_message("WebGPU byte host is unavailable"))?;
    let table = table(context)?;
    let object = args
        .get(1)
        .and_then(JsValue::as_object)
        .ok_or_else(|| JsNativeError::typ().with_message("expected byte handles"))?;
    let uploads = JsArray::from_object(object)?;
    let count = uploads.length(context)?;
    if count > RESOURCE_LIMIT as u64 {
        return Err(JsNativeError::typ()
            .with_message("too many byte handles")
            .into());
    }
    let mut ids = Vec::with_capacity(count as usize);
    for index in 0..count {
        let value = uploads.get(index, context)?;
        ids.push(integer(&[value], 0, context)?);
    }
    let message = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let handled = bridge.handle(&message);
    // Unknown descriptor fields and rejected admission cannot leave uploads
    // behind. Claimed handles are released by their command lease instead.
    for id in ids {
        table.release_staged(id);
    }
    if !handled {
        return Err(JsNativeError::typ()
            .with_message("invalid WebGPU command")
            .into());
    }
    Ok(JsValue::undefined())
}

pub(super) fn install(bridge: &Bridge, document: &mut blitz_script::ScriptDocument) {
    document.with_js_context(|context| {
        context.insert_data(NativeHost {
            bridge: bridge.clone(),
            table: bridge.byte_table(),
        });
        type Callback = fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>;
        let functions: [(&str, usize, Callback); 4] = [
            ("__chuzzGpuPutBytes", 3, put),
            ("__chuzzGpuTakeBytes", 1, take),
            ("__chuzzGpuReleaseBytes", 1, release),
            ("__chuzzGpuSend", 2, send),
        ];
        for (name, length, function) in functions {
            context
                .register_global_builtin_callable(
                    JsString::from(name),
                    length,
                    NativeFunction::from_fn_ptr(function),
                )
                .expect("WebGPU byte bridge");
        }
    });
}
