use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use crate::script_host::{
    ChuzzScriptHost, HostCallbackId, HostError, HostInstallContext, HostObject, HostValue,
    from_json, number_arg, to_json,
};

const MSE_STATE: &str = "chuzz.video.mse";
const VIDEO_STATE: &str = "chuzz.video.native";

struct MseNative(Arc<super::mse::Host>);

impl Drop for MseNative {
    fn drop(&mut self) {
        self.0.close_all();
    }
}

struct VideoNative {
    frames: Arc<super::Frames>,
    streams: RefCell<super::stream::Host>,
    entries: RefCell<HashMap<u64, super::Entry>>,
    net: Arc<crate::document_loader::NetProvider>,
    base: Option<url::Url>,
}

impl Drop for VideoNative {
    fn drop(&mut self) {
        self.frames.close();
    }
}

fn type_error(message: impl Into<String>) -> HostError {
    HostError::new("TypeError", message)
}

fn mse_error(error: super::mse::Error) -> HostError {
    HostError::new(error.name, error.message)
}

fn mse_host(host: &dyn ChuzzScriptHost) -> Result<Arc<super::mse::Host>, HostError> {
    host.state(MSE_STATE)
        .and_then(|state| {
            state
                .downcast_ref::<MseNative>()
                .map(|native| Arc::clone(&native.0))
        })
        .ok_or_else(|| HostError::new("InvalidStateError", "MSE host is unavailable"))
}

fn video_state(host: &dyn ChuzzScriptHost) -> Result<HostObject, HostError> {
    host.state(VIDEO_STATE)
        .ok_or_else(|| HostError::new("InvalidStateError", "video host is unavailable"))
}

pub(crate) fn frames(host: &dyn ChuzzScriptHost) -> Result<Arc<super::Frames>, HostError> {
    let object = video_state(host)?;
    let state = object
        .downcast_ref::<VideoNative>()
        .ok_or_else(|| type_error("invalid video state"))?;
    Ok(Arc::clone(&state.frames))
}

fn acquire_frame(
    host: &mut dyn ChuzzScriptHost,
    args: &[HostValue],
) -> Result<HostValue, HostError> {
    let id = strict_u64(number_arg(args, 0)?, "invalid video identifier")?;
    let generation = strict_u64(number_arg(args, 1)?, "invalid video generation")?;
    let frame = frames(host)?.acquire(id, generation).map_err(type_error)?;
    Ok(from_json(
        serde_json::to_value(frame).map_err(|error| type_error(error.to_string()))?,
    ))
}

fn record(value: &HostValue) -> Result<&std::collections::BTreeMap<String, HostValue>, HostError> {
    match value {
        HostValue::Record(fields) => Ok(fields),
        _ => Err(type_error("expected a native record")),
    }
}

fn strict_u64(value: f64, message: &str) -> Result<u64, HostError> {
    if !value.is_finite()
        || value < 0.0
        || value.fract() != 0.0
        || value > super::mse::MAX_ID as f64
    {
        return Err(type_error(message));
    }
    Ok(value as u64)
}

fn append_bytes(value: &HostValue) -> Result<&[u8], HostError> {
    match value {
        HostValue::Buffer(buffer) => buffer
            .as_bytes()
            .map_err(|_| type_error("detached MSE appendBuffer")),
        _ => Err(type_error("expected an ArrayBuffer")),
    }
}

fn put(host: &mut dyn ChuzzScriptHost, args: &[HostValue]) -> Result<HostValue, HostError> {
    let bytes = args
        .first()
        .ok_or_else(|| type_error("expected an ArrayBuffer"))?;
    let bytes = append_bytes(bytes)?;
    let offset = strict_u64(number_arg(args, 1)?, "invalid MSE byte range")?;
    let length = strict_u64(number_arg(args, 2)?, "invalid MSE byte range")?;
    let end = offset
        .checked_add(length)
        .filter(|end| *end <= bytes.len() as u64)
        .ok_or_else(|| type_error("MSE byte range is out of bounds"))?;
    let id = mse_host(host)?
        .put_bytes(&bytes[offset as usize..end as usize])
        .map_err(mse_error)?;
    Ok(HostValue::Number(id as f64))
}

fn release(host: &mut dyn ChuzzScriptHost, args: &[HostValue]) -> Result<HostValue, HostError> {
    let id = strict_u64(number_arg(args, 0)?, "invalid MSE byte range")?;
    mse_host(host)?.release_upload(id).map_err(mse_error)?;
    Ok(HostValue::Undefined)
}

fn command_json(
    fields: &std::collections::BTreeMap<String, HostValue>,
) -> Result<(serde_json::Value, Option<Vec<u8>>), HostError> {
    let mut object = serde_json::Map::new();
    let mut bytes = None;
    for (name, value) in fields {
        if name == "bytes" {
            bytes = Some(append_bytes(value)?.to_vec());
        } else {
            object.insert(name.clone(), to_json(value)?);
        }
    }
    Ok((serde_json::Value::Object(object), bytes))
}

fn control(host: &mut dyn ChuzzScriptHost, args: &[HostValue]) -> Result<HostValue, HostError> {
    let fields = record(
        args.first()
            .ok_or_else(|| type_error("expected an MSE command record"))?,
    )?;
    let (value, bytes) = command_json(fields)?;
    let op = value
        .get("op")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| HostError::new("InvalidStateError", "invalid MSE command"))?;
    if op != "append" && bytes.is_some() {
        return Err(HostError::new("InvalidStateError", "invalid MSE command"));
    }
    let mut request = serde_json::from_value::<super::mse::Request>(value)
        .map_err(|_| HostError::new("InvalidStateError", "invalid MSE command"))?;
    if let Some(bytes) = bytes {
        if request.handle.is_some() {
            return Err(type_error("MSE append accepts bytes or a byte handle"));
        }
        request.handle = Some(mse_host(host)?.put_bytes(&bytes).map_err(mse_error)?);
    }
    let result = mse_host(host)?.control(request).map_err(mse_error)?;
    Ok(from_json(
        serde_json::json!({ "ok": true, "value": result }),
    ))
}

fn video_command(
    host: &mut dyn ChuzzScriptHost,
    args: &[HostValue],
) -> Result<HostValue, HostError> {
    let value = args
        .first()
        .ok_or_else(|| type_error("expected a video command record"))?;
    let json = to_json(value)?;
    let command = serde_json::from_value::<super::Command>(json)
        .map_err(|error| type_error(error.to_string()))?;
    let state = video_state(host)?;
    let native = state
        .downcast_ref::<VideoNative>()
        .ok_or_else(|| HostError::new("InvalidStateError", "video host is unavailable"))?;
    let mut streams = native
        .streams
        .try_borrow_mut()
        .map_err(|_| HostError::new("InvalidStateError", "video host is busy"))?;
    if command.op == "load" || command.op == "release" {
        streams.remove_native(command.id);
        native.frames.remove(command.id);
        native.entries.borrow_mut().remove(&command.id);
        if command.op == "release" {
            return Ok(HostValue::Boolean(true));
        }
        if command.tracks.is_some() {
            streams.load(&command, Instant::now());
            return Ok(HostValue::Boolean(true));
        }
        let Some(src) = command.src.as_deref().filter(|src| !src.is_empty()) else {
            return Ok(HostValue::Boolean(false));
        };
        let url = url::Url::options()
            .base_url(native.base.as_ref())
            .parse(src)
            .map_err(|error| type_error(error.to_string()))?;
        // Native state uses the script media ID. The renderer owns DOM nodes.
        let mut entry = super::Entry::new(
            super::NodeId::from_u64(command.id),
            url,
            &command,
            Instant::now(),
        );
        if let Some(source) = command.mse {
            entry.requested = true;
            match mse_host(host)?.decoder(source) {
                Ok(decoder) => {
                    entry.decoder = Some(decoder);
                    entry.configure_decoder();
                }
                Err(error) => entry.fail(error),
            }
        } else if !matches!(entry.url.scheme(), "http" | "https" | "file" | "data") {
            entry.fail(super::MediaError::unsupported("unsupported video URL"));
        }
        native.entries.borrow_mut().insert(command.id, entry);
        return Ok(HostValue::Boolean(true));
    }
    if streams.command(&command) {
        return Ok(HostValue::Boolean(true));
    }
    let mut entries = native.entries.borrow_mut();
    if let Some(entry) = entries.get_mut(&command.id) {
        if entry.generation == command.generation
            && let Err(error) = entry.command(&command)
        {
            entry.fail(error);
        }
        return Ok(HostValue::Boolean(true));
    }
    Ok(HostValue::Boolean(false))
}

fn subscribe(host: &mut dyn ChuzzScriptHost, args: &[HostValue]) -> Result<HostValue, HostError> {
    let callback = match args.first() {
        Some(HostValue::Object(object)) => object.downcast_ref::<HostCallbackId>().cloned(),
        _ => None,
    }
    .ok_or_else(|| type_error("expected a video callback root"))?;
    let previous = host.state("chuzz.video.callback");
    host.set_state("chuzz.video.callback", HostObject::new(callback))?;
    if let Some(previous) =
        previous.and_then(|object| object.downcast_ref::<HostCallbackId>().cloned())
    {
        host.release_callback(previous)?;
    }
    Ok(HostValue::Undefined)
}

/// Advance native decoders and deliver metadata plus an optional raster image.
/// The renderer attaches the image to its element and schedules the next frame.
pub fn poll(
    host: &mut dyn ChuzzScriptHost,
    _waker: Option<&std::task::Waker>,
) -> Result<bool, HostError> {
    let object = video_state(host)?;
    let native = object
        .downcast_ref::<VideoNative>()
        .ok_or_else(|| type_error("invalid video state"))?;
    let now = Instant::now();
    let mut updates = native.streams.borrow_mut().poll_native(now);
    for (&id, entry) in native.entries.borrow_mut().iter_mut() {
        match entry.tick(&native.net, now) {
            Ok(Some(frame)) => {
                let mut pixels = Vec::new();
                match super::planar::i420(
                    frame.width,
                    frame.height,
                    8,
                    [&frame.picture.y, &frame.picture.u, &frame.picture.v],
                    &mut pixels,
                ) {
                    Ok(()) => {
                        entry.pixels = Arc::new(pixels);
                        if let Err(error) = native.frames.publish(
                            id,
                            entry.generation,
                            frame.width,
                            frame.height,
                            Arc::clone(&entry.pixels),
                        ) {
                            entry.fail(super::MediaError::decode(error));
                        } else {
                            entry.total_frames += 1;
                        }
                    }
                    Err(error) => entry.fail(error),
                }
            }
            Ok(None) => {}
            Err(error) => entry.fail(error),
        }
        if entry.dirty || !entry.events.is_empty() {
            let image = entry.has_frame.then(|| {
                Arc::new(blitz_dom::node::RasterImageData::new(
                    entry.width,
                    entry.height,
                    Arc::clone(&entry.pixels),
                ))
            });
            updates.push(super::stream::NativeUpdate {
                id,
                generation: entry.generation,
                value: entry.take_update(),
                image,
            });
        }
    }
    let changed = !updates.is_empty();
    if let Some(callback) = host
        .state("chuzz.video.callback")
        .and_then(|object| object.downcast_ref::<HostCallbackId>().cloned())
    {
        for update in updates {
            host.dispatch_callback(
                callback.clone(),
                &[
                    HostValue::Number(update.id as f64),
                    HostValue::Number(update.generation as f64),
                    from_json(update.value),
                    update.image.map_or(HostValue::Null, |image| {
                        HostValue::Object(HostObject::new(image))
                    }),
                ],
            )?;
        }
    }
    Ok(changed)
}

pub fn install(
    host: &mut dyn ChuzzScriptHost,
    config: &HostInstallContext,
) -> Result<(), HostError> {
    host.set_state(
        MSE_STATE,
        HostObject::new(MseNative(Arc::new(super::mse::Host::default()))),
    )?;

    let capture = crate::media::native_host::capture_host(host)?;
    let frames = Arc::new(super::Frames::default());
    let streams = super::stream::Host::new(Arc::clone(&capture), Arc::clone(&frames));
    host.set_state(
        VIDEO_STATE,
        HostObject::new(VideoNative {
            frames,
            streams: RefCell::new(streams),
            entries: RefCell::new(HashMap::new()),
            net: Arc::clone(&config.net),
            base: config.base.clone(),
        }),
    )?;

    host.register_intrinsic("__chuzzMsePut", 3, put)?;
    host.register_intrinsic("__chuzzMseRelease", 1, release)?;
    host.register_intrinsic("__chuzzMseControl", 1, control)?;
    host.register_intrinsic("__chuzzVideoCommand", 1, video_command)?;
    host.register_intrinsic("__chuzzVideoSubscribe", 1, subscribe)?;
    host.register_intrinsic("__chuzzVideoFrame", 2, acquire_frame)?;
    Ok(())
}
