use std::collections::BTreeMap;
use std::sync::Arc;

use blitz_dom::node::RasterImageData;

use crate::script_host::{
    ChuzzScriptHost, HostBuffer, HostCallbackId, HostError, HostFrame, HostFrameSource,
    HostInstallContext, HostIntrinsic, HostObject, HostValue, from_json, number_arg, to_json,
};

const STATE_KEY: &str = "chuzz.webgpu";
const MAX_ID: u64 = 9_007_199_254_740_991;
const RESOURCE_LIMIT: usize = 8192;

fn subscribe(host: &mut dyn ChuzzScriptHost, args: &[HostValue]) -> Result<HostValue, HostError> {
    let callback = match args.first() {
        Some(HostValue::Object(object)) => object.downcast_ref::<HostCallbackId>().cloned(),
        _ => None,
    }
    .ok_or_else(|| type_error("expected a WebGPU callback root"))?;
    let previous = host.state("chuzz.webgpu.callback");
    let same_callback = previous
        .as_ref()
        .and_then(|object| object.downcast_ref::<HostCallbackId>())
        .is_some_and(|previous| previous.same_callback(&callback));
    host.set_state("chuzz.webgpu.callback", HostObject::new(callback))?;
    if !same_callback
        && let Some(previous) =
            previous.and_then(|object| object.downcast_ref::<HostCallbackId>().cloned())
    {
        host.release_callback(previous)?;
    }
    Ok(HostValue::Undefined)
}

fn prepare(native: &NativeHost) {
    let incoming = {
        let mut state = native.bridge.state();
        let live = state
            .in_flight
            .keys()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        state.images.snapshots.retain(|id, _| live.contains(id));
        std::mem::take(&mut state.incoming)
    };
    for request in incoming {
        native.bridge.prepare_image_request(request, |marker| {
            canvas2d::native_source(&native.canvas, marker)
        });
    }
}

/// Drain native replies and raster frames. The renderer applies frames only to
/// the matching canvas generation and schedules subsequent animation turns.
pub fn poll(
    host: &mut dyn ChuzzScriptHost,
    waker: Option<&std::task::Waker>,
) -> Result<bool, HostError> {
    let object = host_object(host)?;
    let native = object
        .downcast_ref::<NativeHost>()
        .expect("checked WebGPU host type");
    native.bridge.state().wake.bind(waker);
    prepare(native);
    native.bridge.expire_requests();
    native.bridge.poll_devices();
    let (ready, events, frames, images) = {
        let mut state = native.bridge.state();
        (
            std::mem::take(&mut state.ready),
            std::mem::take(&mut state.events),
            std::mem::take(&mut state.frames),
            std::mem::take(&mut state.images.paint),
        )
    };
    let changed =
        !ready.is_empty() || !events.is_empty() || !frames.is_empty() || !images.is_empty();
    let callback = host
        .state("chuzz.webgpu.callback")
        .and_then(|object| object.downcast_ref::<HostCallbackId>().cloned());
    let mut error = None;
    for (kind, deliveries) in [("reply", ready), ("event", events)] {
        for (id, payload) in deliveries {
            if let Some(callback) = &callback {
                let callback_payload = if kind == "reply" {
                    match materialize_output(&native.bridge, payload.clone()) {
                        Ok(value) => value,
                        Err(cause) => from_json(serde_json::json!({
                            "ok": false,
                            "error": cause.message,
                        })),
                    }
                } else {
                    from_json(payload.clone())
                };
                let result = host.dispatch_callback(
                    callback.clone(),
                    &[
                        HostValue::String(kind.into()),
                        HostValue::Number(id as f64),
                        callback_payload,
                    ],
                );
                if let Err(cause) = result {
                    error.get_or_insert(cause);
                }
            }
            if kind == "reply"
                && let Some(value) = payload.get("value")
            {
                native.bridge.release_reply(value);
            }
        }
    }
    for frame in frames.into_values() {
        let image = frame.rgba.as_ref().map(|rgba| {
            Arc::new(RasterImageData::new(
                frame.width,
                frame.height,
                Arc::clone(rgba),
            ))
        });
        if let Err(cause) = host.deliver_frame(HostFrame {
            source: HostFrameSource::WebGpu,
            target: frame.canvas,
            generation: frame.generation,
            image,
        }) {
            error.get_or_insert(cause);
        }
        if let Some(callback) = &callback {
            let mut value = BTreeMap::new();
            value.insert(
                "generation".into(),
                HostValue::Number(frame.generation as f64),
            );
            value.insert("width".into(), HostValue::Number(frame.width.into()));
            value.insert("height".into(), HostValue::Number(frame.height.into()));
            value.insert(
                "bytes".into(),
                frame.rgba.map_or(HostValue::Null, |bytes| {
                    HostValue::Buffer(HostBuffer::new(bytes.as_ref().clone()))
                }),
            );
            if let Err(cause) = host.dispatch_callback(
                callback.clone(),
                &[
                    HostValue::String("frame".into()),
                    HostValue::Number(frame.canvas as f64),
                    HostValue::Record(value),
                ],
            ) {
                error.get_or_insert(cause);
            }
        }
    }
    for (id, image) in images {
        if let Err(cause) = host.deliver_frame(HostFrame {
            source: HostFrameSource::Canvas,
            target: id,
            generation: 0,
            image: Some(Arc::clone(&image)),
        }) {
            error.get_or_insert(cause);
        }
        if let Some(callback) = &callback
            && let Err(cause) = host.dispatch_callback(
                callback.clone(),
                &[
                    HostValue::String("image".into()),
                    HostValue::Number(id as f64),
                    HostValue::Object(HostObject::new(image)),
                ],
            )
        {
            error.get_or_insert(cause);
        }
    }
    error.map_or(Ok(changed), Err)
}

use super::resources::GpuError;
use super::{Bridge, binary, canvas2d, external, readback};

struct NativeHost {
    bridge: Bridge,
    canvas: canvas2d::Host,
}

impl Drop for NativeHost {
    fn drop(&mut self) {
        self.bridge.byte_table().close();
    }
}

fn host_object(host: &dyn ChuzzScriptHost) -> Result<HostObject, HostError> {
    let object = host
        .state(STATE_KEY)
        .ok_or_else(|| HostError::new("TypeError", "WebGPU host is unavailable"))?;
    if object.downcast_ref::<NativeHost>().is_none() {
        return Err(HostError::new(
            "TypeError",
            "WebGPU host state has an invalid type",
        ));
    }
    Ok(object)
}

fn native_bridge(host: &dyn ChuzzScriptHost) -> Result<Bridge, HostError> {
    let object = host_object(host)?;
    Ok(object
        .downcast_ref::<NativeHost>()
        .expect("checked WebGPU host type")
        .bridge
        .clone())
}

fn type_error(message: impl Into<String>) -> HostError {
    HostError::new("TypeError", message)
}

fn range_error(message: impl Into<String>) -> HostError {
    HostError::new("RangeError", message)
}

fn gpu_error(error: GpuError) -> HostError {
    type_error(error.message)
}

fn debug_error(error: blitz_control_protocol::DebugError) -> HostError {
    HostError::new(error.code, error.message)
}

fn integer_arg(args: &[HostValue], index: usize, name: &str) -> Result<u64, HostError> {
    let value = number_arg(args, index)?;
    if !value.is_finite() || value.fract() != 0.0 || !(0.0..=MAX_ID as f64).contains(&value) {
        return Err(range_error(name));
    }
    Ok(value as u64)
}

fn json_value(value: &HostValue) -> Result<serde_json::Value, HostError> {
    to_json(value)
}

fn json_result<T: serde::Serialize>(value: &T) -> Result<HostValue, HostError> {
    serde_json::to_value(value)
        .map(from_json)
        .map_err(|error| type_error(error.to_string()))
}

fn materialize_output(bridge: &Bridge, value: serde_json::Value) -> Result<HostValue, HostError> {
    match value {
        serde_json::Value::Object(mut fields)
            if fields.len() == 1 && fields.contains_key("byteId") =>
        {
            let id = fields
                .remove("byteId")
                .and_then(|value| value.as_u64())
                .ok_or_else(|| type_error("invalid WebGPU output handle"))?;
            match bridge.byte_table().take_output(id).map_err(gpu_error)? {
                binary::Block::Buffer(bytes) => {
                    Ok(HostValue::Buffer(HostBuffer::new(bytes.as_ref().clone())))
                }
                binary::Block::Image(image) => {
                    let mut fields = BTreeMap::new();
                    fields.insert("width".into(), HostValue::Number(image.width.into()));
                    fields.insert("height".into(), HostValue::Number(image.height.into()));
                    fields.insert(
                        "data".into(),
                        HostValue::Buffer(HostBuffer::new(image.data.data().to_vec())),
                    );
                    Ok(HostValue::Record(fields))
                }
            }
        }
        serde_json::Value::Object(fields) => fields
            .into_iter()
            .map(|(key, value)| materialize_output(bridge, value).map(|value| (key, value)))
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map(HostValue::Record),
        serde_json::Value::Array(values) => values
            .into_iter()
            .map(|value| materialize_output(bridge, value))
            .collect::<Result<Vec<_>, _>>()
            .map(HostValue::Array),
        value => Ok(from_json(value)),
    }
}

fn put_bytes_intrinsic(
    host: &mut dyn ChuzzScriptHost,
    args: &[HostValue],
) -> Result<HostValue, HostError> {
    let HostValue::Buffer(buffer) = args
        .first()
        .ok_or_else(|| type_error("expected an ArrayBuffer"))?
    else {
        return Err(type_error("expected an ArrayBuffer"));
    };
    let offset = integer_arg(args, 1, "invalid WebGPU byte range")?;
    let length = integer_arg(args, 2, "invalid WebGPU byte range")?;
    let bytes = buffer
        .as_bytes()
        .map_err(|_| type_error("detached WebGPU upload"))?;
    let end = offset
        .checked_add(length)
        .filter(|end| *end <= bytes.len() as u64)
        .ok_or_else(|| type_error("WebGPU data range is out of bounds"))?;
    let id = native_bridge(host)?
        .byte_table()
        .upload(&bytes[offset as usize..end as usize])
        .map_err(gpu_error)?;
    Ok(HostValue::Number(id as f64))
}

fn take_bytes(host: &mut dyn ChuzzScriptHost, args: &[HostValue]) -> Result<HostValue, HostError> {
    let id = integer_arg(args, 0, "invalid WebGPU byte range")?;
    match native_bridge(host)?
        .byte_table()
        .take_output(id)
        .map_err(gpu_error)?
    {
        binary::Block::Buffer(bytes) => {
            Ok(HostValue::Buffer(HostBuffer::new(bytes.as_ref().clone())))
        }
        binary::Block::Image(image) => {
            let mut fields = BTreeMap::new();
            fields.insert("width".to_owned(), HostValue::Number(image.width.into()));
            fields.insert("height".to_owned(), HostValue::Number(image.height.into()));
            fields.insert(
                "data".to_owned(),
                HostValue::Buffer(HostBuffer::new(image.data.data().to_vec())),
            );
            Ok(HostValue::Record(fields))
        }
    }
}

fn release_bytes(
    host: &mut dyn ChuzzScriptHost,
    args: &[HostValue],
) -> Result<HostValue, HostError> {
    let id = integer_arg(args, 0, "invalid WebGPU byte range")?;
    native_bridge(host)?.byte_table().release_staged(id);
    Ok(HostValue::Undefined)
}

fn discard_bytes(
    host: &mut dyn ChuzzScriptHost,
    args: &[HostValue],
) -> Result<HostValue, HostError> {
    let id = integer_arg(args, 0, "invalid WebGPU byte range")?;
    native_bridge(host)?.byte_table().discard(id);
    Ok(HostValue::Undefined)
}

fn send(host: &mut dyn ChuzzScriptHost, args: &[HostValue]) -> Result<HostValue, HostError> {
    let command = args
        .first()
        .ok_or_else(|| type_error("invalid WebGPU command"))?;
    let command = json_value(command)?;
    let uploads = match args.get(1) {
        Some(HostValue::Array(values)) => values,
        _ => return Err(type_error("expected byte handles")),
    };
    if uploads.len() > RESOURCE_LIMIT {
        return Err(type_error("too many byte handles"));
    }
    let table = native_bridge(host)?.byte_table();
    let mut ids = Vec::with_capacity(uploads.len());
    for value in uploads {
        ids.push(integer_arg(
            std::slice::from_ref(value),
            0,
            "invalid WebGPU byte range",
        )?);
    }
    let message = serde_json::to_string(&command).map_err(|error| type_error(error.to_string()))?;
    let handled = native_bridge(host)?.handle(&message);
    for id in ids {
        table.release_staged(id);
    }
    if !handled {
        return Err(type_error("invalid WebGPU command"));
    }
    Ok(HostValue::Undefined)
}

fn readback(host: &mut dyn ChuzzScriptHost, args: &[HostValue]) -> Result<HostValue, HostError> {
    let source_value = args
        .first()
        .ok_or_else(|| type_error("invalid synchronous canvas image source"))?;
    let source_json = json_value(source_value)?;
    let source = serde_json::from_value::<external::Source>(source_json)
        .map_err(|error| type_error(format!("invalid synchronous canvas image source: {error}")))?;
    let id = integer_arg(args, 1, "invalid canvas readback handle")?;
    if id == 0 {
        return Err(type_error("invalid canvas readback handle"));
    }
    let object = host_object(host)?;
    let native = object
        .downcast_ref::<NativeHost>()
        .expect("checked WebGPU host type");
    let image = readback::read_image(&native.bridge, source, id, |marker| {
        canvas2d::native_source(&native.canvas, marker)
    })
    .map_err(gpu_error)?
    .map_err(gpu_error)?;
    Ok(HostValue::Object(HostObject::new(image)))
}

fn canvas2d(host: &mut dyn ChuzzScriptHost, args: &[HostValue]) -> Result<HostValue, HostError> {
    let request = args
        .first()
        .ok_or_else(|| type_error("invalid canvas command"))?;
    let request = json_value(request)?;
    let buffers = match args.get(1) {
        None | Some(HostValue::Undefined) | Some(HostValue::Null) => &[][..],
        Some(HostValue::Array(values)) => {
            let object = host_object(host)?;
            let native = object
                .downcast_ref::<NativeHost>()
                .expect("checked WebGPU host type");
            let mut inputs = Vec::with_capacity(values.len());
            if values.len() > 1024 {
                return Err(type_error("canvas command limit reached in chuzz"));
            }
            let mut total = 0usize;
            for value in values {
                let HostValue::Buffer(buffer) = value else {
                    return Err(type_error("expected canvas ArrayBuffer"));
                };
                let bytes = buffer
                    .as_bytes()
                    .map_err(|_| type_error("detached canvas image"))?;
                total = total.saturating_add(bytes.len());
                if total > 64 * 1024 * 1024 {
                    return Err(type_error("canvas transfer limit reached in chuzz"));
                }
                inputs.push(Arc::new(bytes.to_vec()));
            }
            return canvas2d_reply(
                canvas2d::native_execute(&native.canvas, request, inputs).map_err(type_error)?,
            );
        }
        _ => return Err(type_error("expected canvas byte buffers")),
    };
    let object = host_object(host)?;
    let native = object
        .downcast_ref::<NativeHost>()
        .expect("checked WebGPU host type");
    canvas2d_reply(
        canvas2d::native_execute(&native.canvas, request, buffers.to_vec()).map_err(type_error)?,
    )
}

fn canvas2d_reply(reply: canvas2d::CanvasReply) -> Result<HostValue, HostError> {
    match reply {
        canvas2d::CanvasReply::Empty => Ok(HostValue::Undefined),
        canvas2d::CanvasReply::Bytes(bytes) => {
            Ok(HostValue::Buffer(HostBuffer::new(bytes.as_ref().clone())))
        }
        canvas2d::CanvasReply::Text(value) => match serde_json::from_str(&value) {
            Ok(value) => Ok(from_json(value)),
            Err(_) => Ok(from_json(serde_json::Value::String(value))),
        },
        canvas2d::CanvasReply::Bool(value) => Ok(HostValue::Boolean(value)),
    }
}

fn canvas_source(
    host: &mut dyn ChuzzScriptHost,
    args: &[HostValue],
) -> Result<HostValue, HostError> {
    let id = integer_arg(args, 0, "invalid native canvas source handle")?;
    let object = args
        .get(1)
        .and_then(|value| match value {
            HostValue::Object(object) => Some(object),
            _ => None,
        })
        .ok_or_else(|| type_error("expected a native raster image"))?;
    let image = object
        .downcast_ref::<Arc<RasterImageData>>()
        .cloned()
        .ok_or_else(|| type_error("expected a native raster image"))?;
    let native = host_object(host)?;
    let native = native
        .downcast_ref::<NativeHost>()
        .expect("checked WebGPU host type");
    let (width, height) =
        canvas2d::native_set_source(&native.canvas, id, image).map_err(type_error)?;
    let mut fields = BTreeMap::new();
    fields.insert("width".to_owned(), HostValue::Number(width.into()));
    fields.insert("height".to_owned(), HostValue::Number(height.into()));
    Ok(HostValue::Record(fields))
}

fn encode_png(_: &mut dyn ChuzzScriptHost, args: &[HostValue]) -> Result<HostValue, HostError> {
    let width = integer_arg(args, 0, "invalid canvas width")?;
    let height = integer_arg(args, 1, "invalid canvas height")?;
    let HostValue::Buffer(buffer) = args
        .get(2)
        .ok_or_else(|| type_error("expected canvas pixels"))?
    else {
        return Err(type_error("expected canvas pixels"));
    };
    let pixels = buffer
        .as_bytes()
        .map_err(|_| type_error("detached canvas pixels"))?;
    let width = u32::try_from(width).map_err(|_| type_error("invalid canvas width"))?;
    let height = u32::try_from(height).map_err(|_| type_error("invalid canvas height"))?;
    let png = canvas2d::encode_pixels(width, height, pixels).map_err(type_error)?;
    Ok(HostValue::Buffer(HostBuffer::new(png)))
}

fn inspect(host: &mut dyn ChuzzScriptHost, _: &[HostValue]) -> Result<HostValue, HostError> {
    json_result(&native_bridge(host)?.inspect())
}

fn capture(host: &mut dyn ChuzzScriptHost, args: &[HostValue]) -> Result<HostValue, HostError> {
    let target = crate::script_host::string_arg(args, 0)?;
    let object = host_object(host)?;
    prepare(
        object
            .downcast_ref::<NativeHost>()
            .expect("checked WebGPU host type"),
    );
    let value = native_bridge(host)?
        .capture_and_wait(&target)
        .map_err(debug_error)?;
    json_result(&value)
}

fn capture_buffer(
    host: &mut dyn ChuzzScriptHost,
    args: &[HostValue],
) -> Result<HostValue, HostError> {
    let target = crate::script_host::string_arg(args, 0)?;
    let offset = integer_arg(args, 1, "invalid buffer capture offset")?;
    let length = match args.get(2) {
        None | Some(HostValue::Undefined) | Some(HostValue::Null) => None,
        Some(HostValue::Number(value))
            if value.is_finite()
                && value.fract() == 0.0
                && (0.0..=MAX_ID as f64).contains(value) =>
        {
            Some(*value as u64)
        }
        _ => return Err(type_error("invalid buffer capture length")),
    };
    let bridge = native_bridge(host)?;
    let object = host_object(host)?;
    prepare(
        object
            .downcast_ref::<NativeHost>()
            .expect("checked WebGPU host type"),
    );
    bridge.inspection_session().renew();
    let value = bridge
        .sample_buffer(&target, offset, length)
        .map_err(debug_error)?;
    json_result(&value)
}

pub fn install(
    host: &mut dyn ChuzzScriptHost,
    config: &HostInstallContext,
) -> Result<(), HostError> {
    let bridge = Bridge::new(config.wake.clone());
    bridge.state().media_frames = crate::video::native_host::frames(host)?;
    let canvas = canvas2d::native_host(bridge.clone());
    host.set_state(STATE_KEY, HostObject::new(NativeHost { bridge, canvas }))?;

    let intrinsics: [(&'static str, u8, HostIntrinsic); 12] = [
        ("__chuzzGpuPutBytes", 3, put_bytes_intrinsic),
        ("__chuzzGpuTakeBytes", 1, take_bytes),
        ("__chuzzGpuReleaseBytes", 1, release_bytes),
        ("__chuzzGpuDiscardBytes", 1, discard_bytes),
        ("__chuzzGpuSend", 2, send),
        ("__chuzzCanvasReadback", 2, readback),
        ("__chuzzCanvas2D", 2, canvas2d),
        ("__chuzzCanvasSource", 2, canvas_source),
        ("__chuzzEncodePng", 3, encode_png),
        ("__chuzzGpuInspect", 0, inspect),
        ("__chuzzGpuCapture", 1, capture),
        ("__chuzzGpuCaptureBuffer", 3, capture_buffer),
    ];
    for (name, arity, callback) in intrinsics {
        host.register_intrinsic(name, arity, callback)?;
    }
    host.register_intrinsic("__chuzzGpuSubscribe", 1, subscribe)?;
    Ok(())
}
