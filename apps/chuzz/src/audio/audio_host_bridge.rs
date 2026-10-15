use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::task::Waker;
use std::time::Instant;

use crate::script_host::{
    ChuzzScriptHost, HostCallbackId, HostError, HostInstallContext, HostObject, HostValue,
    from_json, number_arg, string_arg,
};

const STATE_KEY: &str = "chuzz.audio";
const COMMAND_LIMIT: usize = 65_536;
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

struct NativeAudio {
    page: Arc<super::Page>,
    callbacks: Rc<RefCell<Vec<HostCallbackId>>>,
}

fn native_error(message: impl Into<String>) -> HostError {
    let message = message.into();
    match message.split_once(": ") {
        Some((name, message)) if name.ends_with("Error") => HostError::new(name, message),
        _ => HostError::new("TypeError", message),
    }
}

fn native_state(host: &dyn ChuzzScriptHost) -> Result<NativeAudioState, HostError> {
    let object = host
        .state(STATE_KEY)
        .ok_or_else(|| HostError::new("InvalidStateError", "audio host is unavailable"))?;
    let native = object
        .downcast_ref::<NativeAudio>()
        .ok_or_else(|| HostError::new("InvalidStateError", "audio host state is invalid"))?;
    Ok(NativeAudioState {
        page: Arc::clone(&native.page),
        callbacks: Rc::clone(&native.callbacks),
    })
}

struct NativeAudioState {
    page: Arc<super::Page>,
    callbacks: Rc<RefCell<Vec<HostCallbackId>>>,
}

pub fn media_epoch(host: &dyn ChuzzScriptHost) -> Result<Instant, HostError> {
    Ok(native_state(host)?.page.epoch)
}

pub fn registered_track(
    host: &dyn ChuzzScriptHost,
    id: u64,
) -> Result<Arc<crate::audio::pcm::Track>, HostError> {
    native_state(host)?.page.track(id).map_err(native_error)
}

pub fn register_track(
    host: &dyn ChuzzScriptHost,
    track: Arc<crate::audio::pcm::Track>,
) -> Result<u64, HostError> {
    native_state(host)?
        .page
        .register_track(track)
        .map_err(native_error)
}

fn command(host: &mut dyn ChuzzScriptHost, args: &[HostValue]) -> Result<HostValue, HostError> {
    let page = native_state(host)?.page;
    let message = string_arg(args, 0)?;
    if message.len() > COMMAND_LIMIT {
        return Err(native_error("QuotaExceededError: audio command limit"));
    }
    let command = serde_json::from_str::<super::Command>(&message)
        .map_err(|error| native_error(format!("TypeError: invalid audio command: {error}")))?;
    let value = super::execute(&page, command).map_err(native_error)?;
    Ok(from_json(value))
}

fn integer_arg(args: &[HostValue], index: usize) -> Result<u64, HostError> {
    let value = number_arg(args, index)?;
    if !value.is_finite() || value <= 0.0 || value.fract() != 0.0 || value > MAX_SAFE_INTEGER {
        return Err(native_error("TypeError: invalid audio identifier"));
    }
    Ok(value as u64)
}

fn read(host: &mut dyn ChuzzScriptHost, args: &[HostValue]) -> Result<HostValue, HostError> {
    let page = native_state(host)?.page;
    let context_id = integer_arg(args, 0)?;
    let node = integer_arg(args, 1)?;
    let kind = string_arg(args, 2)?;
    let bytes = super::pcm::lock(&page.engine(context_id).map_err(native_error)?.graph)
        .analysis_bytes(node, &kind)
        .map_err(native_error)?;
    Ok(HostValue::Buffer(crate::script_host::HostBuffer::new(
        bytes,
    )))
}

fn subscribe(host: &mut dyn ChuzzScriptHost, args: &[HostValue]) -> Result<HostValue, HostError> {
    let Some(HostValue::Object(object)) = args.first() else {
        return Err(native_error("a callback root is required"));
    };
    let callback = object
        .downcast_ref::<HostCallbackId>()
        .cloned()
        .ok_or_else(|| native_error("invalid audio callback root"))?;
    let state = native_state(host)?;
    let previous = state.callbacks.borrow_mut().pop();
    state.callbacks.borrow_mut().push(callback);
    if let Some(previous) = previous {
        host.release_callback(previous)?;
    }
    Ok(HostValue::Undefined)
}

pub fn poll(host: &mut dyn ChuzzScriptHost, waker: Option<&Waker>) -> Result<bool, HostError> {
    let state = native_state(host)?;
    if let Some(waker) = waker {
        *super::pcm::lock(&state.page.reader) = Some(waker.clone());
    }
    let events: Vec<_> = {
        let mut queue = super::pcm::lock(&state.page.events);
        let count = queue.len().min(64);
        let events = queue.drain(..count).collect();
        if !queue.is_empty()
            && let Some(reader) = super::pcm::lock(&state.page.reader).as_ref()
        {
            reader.wake_by_ref();
        }
        events
    };
    if events.is_empty() {
        return Ok(false);
    }
    let callbacks = state.callbacks.borrow().clone();
    for (context_id, node) in events {
        let args = [
            HostValue::Number(context_id as f64),
            HostValue::Number(node as f64),
        ];
        for callback in &callbacks {
            host.dispatch_callback(callback.clone(), &args)?;
        }
    }
    Ok(true)
}

pub fn install(
    host: &mut dyn ChuzzScriptHost,
    _config: &HostInstallContext,
) -> Result<(), HostError> {
    let epoch = crate::media::native_host::capture_host(host)?.media_epoch();
    let page = Arc::new(super::Page {
        epoch,
        state: Mutex::new(super::State::default()),
        events: Mutex::new(std::collections::VecDeque::new()),
        reader: Mutex::new(None),
    });
    let native = NativeAudio {
        page,
        callbacks: Rc::new(RefCell::new(Vec::new())),
    };
    host.set_state(STATE_KEY, HostObject::new(native))?;
    host.register_intrinsic("__chuzzAudioCall", 1, command)?;
    host.register_intrinsic("__chuzzAudioRead", 3, read)?;
    host.register_intrinsic("__chuzzAudioSubscribe", 1, subscribe)?;
    Ok(())
}
