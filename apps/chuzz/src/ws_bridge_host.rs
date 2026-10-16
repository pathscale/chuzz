use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::task::Waker;

use serde_json::json;

use crate::script_host::{
    ChuzzScriptHost, HostCallbackId, HostError, HostInstallContext, HostObject, HostValue,
    from_json, number_arg, string_arg,
};

use super::{Bridge, Event, SocketReply, SocketRequest};

const STATE_KEY: &str = "chuzz.websocket";

struct WebSocketHostState {
    bridge: Bridge,
    callbacks: HashMap<u64, HostCallbackId>,
}

impl Drop for WebSocketHostState {
    fn drop(&mut self) {
        self.bridge.retire();
    }
}

fn state(host: &dyn ChuzzScriptHost) -> Result<Rc<RefCell<WebSocketHostState>>, HostError> {
    host.state(STATE_KEY)
        .and_then(|object| {
            object
                .downcast_ref::<Rc<RefCell<WebSocketHostState>>>()
                .cloned()
        })
        .ok_or_else(|| HostError::new("Error", "WebSocket host is unavailable"))
}

fn unsigned_arg(args: &[HostValue], index: usize, name: &str) -> Result<u64, HostError> {
    let number = number_arg(args, index)?;
    if !number.is_finite()
        || number < 0.0
        || number.fract() != 0.0
        || number > 9_007_199_254_740_991.0
    {
        return Err(HostError::new(
            "TypeError",
            format!("{name} must be a non-negative integer"),
        ));
    }
    Ok(number as u64)
}

fn optional_string(args: &[HostValue], index: usize) -> Result<Option<String>, HostError> {
    match args.get(index) {
        None | Some(HostValue::Undefined | HostValue::Null) => Ok(None),
        Some(_) => string_arg(args, index).map(Some),
    }
}

fn optional_code(args: &[HostValue], index: usize) -> Result<Option<u16>, HostError> {
    match args.get(index) {
        None | Some(HostValue::Undefined | HostValue::Null) => Ok(None),
        Some(_) => {
            let number = number_arg(args, index)?;
            if !number.is_finite()
                || number.fract() != 0.0
                || !(0.0..=u16::MAX as f64).contains(&number)
            {
                return Err(HostError::new(
                    "TypeError",
                    "close code must be an integer from 0 through 65535",
                ));
            }
            Ok(Some(number as u16))
        }
    }
}

fn protocols_arg(args: &[HostValue], index: usize) -> Result<Vec<String>, HostError> {
    match args.get(index) {
        None | Some(HostValue::Undefined | HostValue::Null) => Ok(Vec::new()),
        Some(HostValue::Array(values)) => values
            .iter()
            .map(|value| string_arg(std::slice::from_ref(value), 0))
            .collect(),
        Some(_) => Err(HostError::new(
            "TypeError",
            "WebSocket protocols must be an array of strings",
        )),
    }
}

fn connect(host: &mut dyn ChuzzScriptHost, args: &[HostValue]) -> Result<HostValue, HostError> {
    let id = unsigned_arg(args, 0, "socket id")?;
    let url = optional_string(args, 1)?;
    let protocols = protocols_arg(args, 2)?;
    let state = state(host)?;
    let state = state.borrow();
    if let Err(error) = state
        .bridge
        .operate(SocketRequest::Connect { id, url, protocols })
    {
        state.bridge.reject(id, error);
    }
    Ok(HostValue::Undefined)
}

fn send(host: &mut dyn ChuzzScriptHost, args: &[HostValue]) -> Result<HostValue, HostError> {
    let id = unsigned_arg(args, 0, "socket id")?;
    let data = optional_string(args, 1)?;
    let state = state(host)?;
    let state = state.borrow();
    if let Err(error) = state.bridge.operate(SocketRequest::Send { id, data }) {
        state.bridge.reject(id, error);
    }
    Ok(HostValue::Undefined)
}

fn close(host: &mut dyn ChuzzScriptHost, args: &[HostValue]) -> Result<HostValue, HostError> {
    let id = unsigned_arg(args, 0, "socket id")?;
    let code = optional_code(args, 1)?;
    let state = state(host)?;
    let state = state.borrow();
    if let Err(error) = state.bridge.operate(SocketRequest::Close { id, code }) {
        state.bridge.reject(id, error);
    }
    Ok(HostValue::Undefined)
}

fn subscribe(host: &mut dyn ChuzzScriptHost, args: &[HostValue]) -> Result<HostValue, HostError> {
    let id = unsigned_arg(args, 0, "socket id")?;
    let callback = match args.get(1) {
        Some(HostValue::Object(object)) => object
            .downcast_ref::<HostCallbackId>()
            .cloned()
            .ok_or_else(|| {
                HostError::new(
                    "TypeError",
                    "WebSocket subscription requires a retained callback",
                )
            })?,
        _ => {
            return Err(HostError::new(
                "TypeError",
                "WebSocket subscription requires a retained callback",
            ));
        }
    };

    let incoming = callback.clone();
    let state = state(host)?;
    let previous = {
        let mut state = state.borrow_mut();
        if state.callbacks.len() >= 1024 && !state.callbacks.contains_key(&id) {
            return Err(HostError::new(
                "QuotaExceededError",
                "too many socket subscriptions",
            ));
        }
        state.callbacks.insert(id, callback)
    };
    if let Some(previous) = previous.filter(|previous| !previous.same_callback(&incoming)) {
        host.release_callback(previous)?;
    }
    Ok(HostValue::Undefined)
}

pub fn install(
    host: &mut dyn ChuzzScriptHost,
    config: &HostInstallContext,
) -> Result<(), HostError> {
    let bridge = Bridge {
        events: Arc::new(Mutex::new(Vec::new())),
        senders: Arc::new(Mutex::new(HashMap::new())),
        wake: config.wake.clone(),
        lifetime: Some(Arc::clone(&config.lifetime)),
        signal: config.signal.clone(),
        initiator: config.net.initiator(),
    };
    let state = Rc::new(RefCell::new(WebSocketHostState {
        bridge,
        callbacks: HashMap::new(),
    }));

    host.set_state(STATE_KEY, HostObject::new(state))?;
    host.register_intrinsic("__chuzzWsConnect", 3, connect)?;
    host.register_intrinsic("__chuzzWsSend", 2, send)?;
    host.register_intrinsic("__chuzzWsClose", 2, close)?;
    host.register_intrinsic("__chuzzWsSubscribe", 2, subscribe)?;
    Ok(())
}

fn reply_value(reply: &SocketReply) -> serde_json::Value {
    match reply {
        SocketReply::Open { protocol } => json!({"type": "open", "protocol": protocol}),
        SocketReply::Message { data } => json!({"type": "message", "data": data}),
        SocketReply::Error { message } => json!({"type": "error", "message": message}),
        SocketReply::Close { code, reason } => {
            json!({"type": "close", "code": code, "reason": reason})
        }
    }
}

/// Bind the page waker and dispatch pending socket events on the host thread.
pub fn poll(host: &mut dyn ChuzzScriptHost, waker: Option<&Waker>) -> Result<bool, HostError> {
    let state = state(host)?;
    let (bridge, callbacks) = {
        let state = state.borrow();
        (state.bridge.clone(), state.callbacks.clone())
    };

    bridge.wake.bind(waker);
    let ready = bridge
        .events
        .lock()
        .map_err(|_| HostError::new("Error", "socket event queue poisoned"))
        .map(|mut events| std::mem::take(&mut *events))?;

    if ready.is_empty() {
        return Ok(false);
    }

    let mut closed = Vec::new();
    let mut first_error = None;
    for Event { id, reply } in ready {
        if let SocketReply::Close { .. } = reply {
            closed.push(id);
        }
        let Some(callback) = callbacks.get(&id).cloned() else {
            continue;
        };
        let event = from_json(reply_value(&reply));
        if let Err(error) = host.dispatch_callback(callback, &[event])
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    }

    if !closed.is_empty() {
        for id in closed {
            let callback = {
                let mut state = state.borrow_mut();
                if state.callbacks.get(&id) == callbacks.get(&id) {
                    state.callbacks.remove(&id)
                } else {
                    None
                }
            };
            if let Some(callback) = callback
                && let Err(error) = host.release_callback(callback)
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
    }

    if let Some(error) = first_error {
        return Err(error);
    }
    Ok(true)
}
