use std::sync::Arc;

use blitz_script::boa_engine::object::builtins::{
    JsArray, JsArrayBuffer, JsUint8Array, JsUint8ClampedArray,
};
use blitz_script::boa_engine::{
    Context, JsNativeError, JsResult, JsString, JsValue, NativeFunction,
};

use super::*;

fn encode_png(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let failure = |message: &str| JsNativeError::typ().with_message(message.to_owned());
    let width = args.first().cloned().unwrap_or_default().to_u32(context)?;
    let height = args.get(1).cloned().unwrap_or_default().to_u32(context)?;
    let object = args
        .get(2)
        .and_then(JsValue::as_object)
        .ok_or_else(|| failure("expected canvas pixels"))?;
    let buffer = JsArrayBuffer::from_object(object.clone())?;
    let pixels = buffer
        .data()
        .ok_or_else(|| failure("detached canvas pixels"))?;
    let png = super::encode_pixels(width, height, &pixels).map_err(|error| failure(&error))?;
    Ok(JsUint8Array::from_iter(png, context)?.into())
}

fn native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = context
        .get_data::<Host>()
        .cloned()
        .ok_or_else(|| JsNativeError::typ().with_message("canvas host is unavailable"))?;
    let json = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    if json.len() > 8 * 1024 * 1024 {
        return Err(JsNativeError::typ()
            .with_message("canvas descriptor limit reached in chuzz")
            .into());
    }
    let mut request: Request = serde_json::from_str(&json)
        .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
    if request.action == "path" {
        let commands = paths::svg_commands(&request.value)
            .map_err(|error| JsNativeError::typ().with_message(error))?;
        let json = serde_json::to_string(&commands)
            .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
        return Ok(JsString::from(json).into());
    }
    if request.action == "color" {
        return Ok(JsValue::from(color(&request.value).is_ok()));
    }
    if request.action == "font" {
        let font = text::parse_font(&request.value, request.font_base, request.root_base)
            .map_err(|error| JsNativeError::typ().with_message(error))?;
        return Ok(JsString::from(font).into());
    }
    if request.action == "stats" {
        let cache = lock(&host.image_cache);
        let value = serde_json::json!({
            "transferredBytes": *lock(&host.transferred),
            "cacheBytes": cache.retained_bytes(),
            "cacheLimit": cache.byte_limit(),
            "cacheEntries": cache.len(),
            "cacheHits": cache.hits(),
            "cacheMisses": cache.misses(),
        });
        return Ok(JsString::from(value.to_string()).into());
    }
    if request.action == "release" {
        lock(&host.canvases).remove(&request.id);
        if let Some(image) = lock(&host.sources).remove(&request.id) {
            lock(&host.image_cache).remove(image.data.id());
        }
        host.bridge.state().images.paint.remove(&request.id);
        return Ok(JsValue::undefined());
    }
    if request.action == "source" {
        if request.id == 0 || request.id > 9_007_199_254_740_991 {
            return Err(JsNativeError::typ()
                .with_message("invalid native canvas source handle")
                .into());
        }
        let source = request
            .source
            .take()
            .ok_or_else(|| JsNativeError::typ().with_message("missing native canvas source"))?;
        let image = match source {
            super::super::external::Source::Element { handle } => {
                blitz_script::raster_image_for_attribute(
                    context,
                    "data-chuzz-gpu-source",
                    &handle.to_string(),
                )
                .map(Arc::new)
                .ok_or_else(|| JsNativeError::typ().with_message("image is not decoded"))?
            }
            source => super::super::readback::image(&host.bridge, source, request.serial, context)?
                .map_err(|error| JsNativeError::typ().with_message(error.message))?,
        };
        let size = byte_count(image.width, image.height)
            .map_err(|error| JsNativeError::typ().with_message(error))?;
        if size != image.data.len() {
            return Err(JsNativeError::typ()
                .with_message("invalid native canvas source pixels")
                .into());
        }
        {
            let mut sources = lock(&host.sources);
            let previous = sources.get(&request.id).map_or(0, |image| image.data.len());
            let allocated = sources
                .values()
                .map(|image| image.data.len())
                .sum::<usize>();
            if (!sources.contains_key(&request.id) && sources.len() >= 8192)
                || allocated.saturating_sub(previous).saturating_add(size) > DOCUMENT_LIMIT / 3
            {
                return Err(JsNativeError::typ()
                    .with_message("native canvas source allocation limit reached")
                    .into());
            }
            if let Some(previous) = sources.insert(request.id, raster_image(&image))
                && previous.data.id() != image.data.id()
            {
                lock(&host.image_cache).remove(previous.data.id());
            }
        }
        return Ok(JsArray::from_iter(
            [JsValue::from(image.width), JsValue::from(image.height)],
            context,
        )
        .into());
    }
    let array = args
        .get(1)
        .and_then(JsValue::as_object)
        .ok_or_else(|| JsNativeError::typ().with_message("expected canvas byte buffers"))?;
    let array = JsArray::from_object(array)?;
    let count = array.length(context)?;
    if count > 1024 || request.commands.len() > COMMAND_LIMIT {
        return Err(JsNativeError::typ()
            .with_message("canvas command limit reached in chuzz")
            .into());
    }
    let mut inputs = Vec::with_capacity(count as usize);
    let mut input_size = 0usize;
    for index in 0..count {
        let value = array.get(index, context)?;
        let object = value
            .as_object()
            .ok_or_else(|| JsNativeError::typ().with_message("expected canvas ArrayBuffer"))?;
        let buffer = JsArrayBuffer::from_object(object)?;
        let bytes = buffer
            .data()
            .ok_or_else(|| JsNativeError::typ().with_message("detached canvas image"))?;
        input_size = input_size.saturating_add(bytes.len());
        if input_size > IMAGE_LIMIT {
            return Err(JsNativeError::typ()
                .with_message("canvas transfer limit reached in chuzz")
                .into());
        }
        inputs.push(Arc::new(bytes.to_vec()));
    }
    {
        let mut transferred = lock(&host.transferred);
        *transferred = transferred.saturating_add(input_size as u64);
    }
    for command in &mut request.commands {
        let Some(image) = command
            .brush
            .as_mut()
            .and_then(|brush| brush.image.as_mut())
        else {
            continue;
        };
        let references = [image.video, image.canvas, image.bitmap, image.native]
            .iter()
            .filter(|handle| handle.is_some())
            .count();
        if references > 1 {
            return Err(JsNativeError::typ()
                .with_message("ambiguous native canvas image source")
                .into());
        }
        if let Some(handle) = image.video.take() {
            let frame = crate::video::take_frame(context, handle)
                .map_err(|error| JsNativeError::typ().with_message(error))?;
            image.resolved = Some(raster_image(&frame.image));
        } else if let Some(handle) = image.bitmap {
            let bitmap = host
                .bridge
                .state()
                .images
                .bitmaps
                .get(&handle)
                .cloned()
                .ok_or_else(|| {
                    JsNativeError::typ().with_message("ImageBitmap is closed or invalid")
                })?;
            image.resolved = Some(raster_image(&bitmap));
        } else if let Some(handle) = image.native {
            image.resolved = Some(lock(&host.sources).get(&handle).cloned().ok_or_else(|| {
                JsNativeError::typ().with_message("native canvas source was released")
            })?);
        }
        if image.resolved.is_some() {
            image_data(image, &inputs).map_err(|error| JsNativeError::typ().with_message(error))?;
        }
    }
    let reply = execute(&host, &mut request, &inputs)
        .map_err(|error| JsNativeError::typ().with_message(error))?;
    match reply {
        CanvasReply::Empty => Ok(JsValue::undefined()),
        CanvasReply::Bytes(bytes) => {
            JsUint8ClampedArray::from_iter(bytes.iter().copied(), context).map(Into::into)
        }
        CanvasReply::Text(value) => Ok(JsString::from(value).into()),
        CanvasReply::Bool(value) => Ok(JsValue::from(value)),
    }
}

pub(crate) fn install(document: &mut blitz_script::ScriptDocument, bridge: Bridge) {
    document.with_js_context(|context| {
        let fonts = context
            .get_data::<Fonts>()
            .cloned()
            .unwrap_or_else(|| Fonts(Arc::new(Mutex::new(default_fonts()))));
        let image_cache = Arc::clone(&bridge.state().images.image_cache);
        context.insert_data(Host {
            canvases: Arc::new(Mutex::new(HashMap::new())),
            sources: Arc::new(Mutex::new(HashMap::new())),
            image_cache,
            transferred: Arc::new(Mutex::new(0)),
            fonts,
            bridge,
        });
        context
            .register_global_builtin_callable(
                JsString::from("__chuzzCanvas2D"),
                2,
                NativeFunction::from_fn_ptr(native),
            )
            .expect("canvas command bridge");
        context
            .register_global_builtin_callable(
                JsString::from("__chuzzEncodePng"),
                3,
                NativeFunction::from_fn_ptr(encode_png),
            )
            .expect("canvas PNG encoder");
    });
}
