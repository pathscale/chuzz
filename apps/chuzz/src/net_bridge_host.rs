use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use crate::script_host::{
    ChuzzScriptHost, HostBuffer, HostCallbackId, HostError, HostInstallContext, HostObject,
    HostValue,
};

const STATE_KEY: &str = "chuzz.net";

type CompletionQueue = Arc<Mutex<Vec<(u64, Result<super::NetReply, super::NetworkError>)>>>;

struct NetRealmState {
    network: super::NetworkHost,
    wake: super::PageWake,
    next_id: u64,
    callbacks: HashMap<u64, HostCallbackId>,
    pending: HashSet<u64>,
    completions: CompletionQueue,
}

fn type_error(message: impl Into<String>) -> HostError {
    HostError::new("TypeError", message)
}

fn state(host: &dyn ChuzzScriptHost) -> Result<Rc<RefCell<NetRealmState>>, HostError> {
    let object = host
        .state(STATE_KEY)
        .ok_or_else(|| type_error("page network host is unavailable"))?;
    object
        .downcast_ref::<Rc<RefCell<NetRealmState>>>()
        .cloned()
        .ok_or_else(|| type_error("page network host state is invalid"))
}

fn record_value(fields: impl IntoIterator<Item = (String, HostValue)>) -> HostValue {
    HostValue::Record(fields.into_iter().collect::<BTreeMap<_, _>>())
}

fn string_value(value: impl Into<String>) -> HostValue {
    HostValue::String(Rc::<str>::from(value.into()))
}

fn strict_string(value: &HostValue, field: &str) -> Result<String, HostError> {
    match value {
        HostValue::String(value) => Ok(value.to_string()),
        HostValue::Utf16(units) => String::from_utf16(units)
            .map_err(|_| type_error(format!("{field} contains invalid UTF-16"))),
        _ => Err(type_error(format!("{field} must be a string"))),
    }
}

fn field_string(
    fields: &BTreeMap<String, HostValue>,
    key: &str,
    default: Option<&str>,
) -> Result<String, HostError> {
    match fields.get(key) {
        Some(value) => strict_string(value, key),
        None => default
            .map(str::to_owned)
            .ok_or_else(|| type_error(format!("missing {key}"))),
    }
}

fn byte_value(value: &HostValue) -> Result<u8, HostError> {
    match value {
        HostValue::Number(number)
            if number.is_finite()
                && *number >= 0.0
                && *number <= f64::from(u8::MAX)
                && number.fract() == 0.0 =>
        {
            Ok(*number as u8)
        }
        _ => Err(type_error("request body contains an invalid byte")),
    }
}

fn parse_body(value: Option<&HostValue>) -> Result<Option<Vec<u8>>, HostError> {
    match value {
        None | Some(HostValue::Undefined | HostValue::Null) => Ok(None),
        Some(HostValue::Buffer(buffer)) => buffer
            .as_bytes()
            .map(|bytes| Some(bytes.to_vec()))
            .map_err(|error| type_error(error.to_string())),
        Some(HostValue::Array(values)) => values
            .iter()
            .map(byte_value)
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        _ => Err(type_error(
            "body must be a buffer, byte array, null, or undefined",
        )),
    }
}

fn parse_headers(value: Option<&HostValue>) -> Result<Vec<(String, String)>, HostError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let HostValue::Array(entries) = value else {
        return Err(type_error("headers must be an array of name/value pairs"));
    };
    entries
        .iter()
        .map(|entry| {
            let HostValue::Array(pair) = entry else {
                return Err(type_error("each header must be a name/value pair"));
            };
            if pair.len() != 2 {
                return Err(type_error("each header must contain exactly two values"));
            }
            Ok((
                strict_string(&pair[0], "header name")?,
                strict_string(&pair[1], "header value")?,
            ))
        })
        .collect()
}

fn parse_request(value: &HostValue) -> Result<super::NetRequest, HostError> {
    let HostValue::Record(fields) = value else {
        return Err(type_error("request must be a record"));
    };
    let id = match fields.get("id") {
        None => 0,
        Some(HostValue::Number(number))
            if number.is_finite()
                && *number >= 0.0
                && *number <= u64::MAX as f64
                && number.fract() == 0.0 =>
        {
            *number as u64
        }
        Some(_) => return Err(type_error("id must be a non-negative integer")),
    };
    Ok(super::NetRequest {
        id,
        url: field_string(fields, "url", None)?,
        method: field_string(fields, "method", Some("GET"))?,
        headers: parse_headers(fields.get("headers"))?,
        body: parse_body(fields.get("body"))?,
        credentials: field_string(fields, "credentials", Some("same-origin"))?,
    })
}

fn response_value(reply: Result<super::NetReply, super::NetworkError>) -> HostValue {
    match reply {
        Ok(reply) => record_value([
            ("ok".to_owned(), HostValue::Boolean(true)),
            (
                "status".to_owned(),
                HostValue::Number(f64::from(reply.status)),
            ),
            ("statusText".to_owned(), string_value(reply.status_text)),
            ("url".to_owned(), string_value(reply.url.as_str())),
            ("responseURL".to_owned(), string_value(reply.url.as_str())),
            (
                "redirected".to_owned(),
                HostValue::Boolean(reply.redirected),
            ),
            (
                "headers".to_owned(),
                HostValue::Array(
                    reply
                        .headers
                        .into_iter()
                        .map(|(name, value)| {
                            HostValue::Array(vec![string_value(name), string_value(value)])
                        })
                        .collect(),
                ),
            ),
            ("body".to_owned(), string_value(reply.body)),
            (
                "bytes".to_owned(),
                HostValue::Buffer(HostBuffer::new(reply.bytes)),
            ),
        ]),
        Err(error) => record_value([
            ("ok".to_owned(), HostValue::Boolean(false)),
            ("error".to_owned(), string_value(error.to_string())),
        ]),
    }
}

fn network_sync(
    host: &mut dyn ChuzzScriptHost,
    args: &[HostValue],
) -> Result<HostValue, HostError> {
    let request = parse_request(
        args.first()
            .ok_or_else(|| type_error("request argument is required"))?,
    )?;
    let state = state(host)?;
    let network = state.borrow().network.clone();
    Ok(response_value(nagoya::block_on(network.perform(request))))
}

fn cookie_get(host: &mut dyn ChuzzScriptHost, _args: &[HostValue]) -> Result<HostValue, HostError> {
    let state = state(host)?;
    let value = state.borrow().network.cookie_get();
    Ok(string_value(value))
}

fn cookie_set(host: &mut dyn ChuzzScriptHost, args: &[HostValue]) -> Result<HostValue, HostError> {
    let assignment = args
        .first()
        .ok_or_else(|| type_error("cookie assignment is required"))
        .and_then(|value| strict_string(value, "cookie assignment"))?;
    let state = state(host)?;
    let network = state.borrow().network.clone();
    network
        .cookie_set(&assignment)
        .map(string_value)
        .map_err(|error| type_error(error.to_string()))
}

fn next_id(state: &mut NetRealmState) -> u64 {
    loop {
        let id = state.next_id;
        state.next_id = if id >= (1_u64 << 53) - 1 { 1 } else { id + 1 };
        if id != 0 && !state.pending.contains(&id) {
            return id;
        }
    }
}

fn release_callback_if_unused(
    host: &mut dyn ChuzzScriptHost,
    state: &Rc<RefCell<NetRealmState>>,
    callback: HostCallbackId,
) -> Result<(), HostError> {
    let still_pending = state
        .borrow()
        .callbacks
        .values()
        .any(|pending| pending.same_callback(&callback));
    if still_pending {
        Ok(())
    } else {
        host.release_callback(callback)
    }
}

fn network_async(
    host: &mut dyn ChuzzScriptHost,
    args: &[HostValue],
) -> Result<HostValue, HostError> {
    let request = parse_request(
        args.first()
            .ok_or_else(|| type_error("request argument is required"))?,
    )?;
    let callback = match args.get(1) {
        Some(HostValue::Object(object)) => object
            .downcast_ref::<HostCallbackId>()
            .cloned()
            .ok_or_else(|| type_error("callback object is invalid"))?,
        _ => return Err(type_error("callback object is required")),
    };

    let state = state(host)?;
    let (id, network, completions, wake) = {
        let mut state = state.borrow_mut();
        if state.pending.len() >= 1024 {
            return Err(HostError::new(
                "QuotaExceededError",
                "too many pending network requests",
            ));
        }
        let id = next_id(&mut state);
        state.callbacks.insert(id, callback);
        state.pending.insert(id);
        (
            id,
            state.network.clone(),
            Arc::clone(&state.completions),
            state.wake.clone(),
        )
    };

    drop(nagoya::spawn(async move {
        let reply = network.perform(request).await;
        completions
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push((id, reply));
        wake.wake();
    }));

    Ok(HostValue::Number(id as f64))
}

fn network_forget(
    host: &mut dyn ChuzzScriptHost,
    args: &[HostValue],
) -> Result<HostValue, HostError> {
    let id = match args.first() {
        Some(HostValue::Number(number))
            if number.is_finite()
                && *number > 0.0
                && *number <= ((1_u64 << 53) - 1) as f64
                && number.fract() == 0.0 =>
        {
            *number as u64
        }
        _ => return Err(type_error("request id must be a positive integer")),
    };
    let state = state(host)?;
    let callback = state.borrow_mut().callbacks.remove(&id);
    if let Some(callback) = callback {
        release_callback_if_unused(host, &state, callback)?;
    }
    Ok(HostValue::Undefined)
}

/// Install native networking and cookie intrinsics into one script realm.
pub fn install(
    host: &mut dyn ChuzzScriptHost,
    config: &HostInstallContext,
) -> Result<(), HostError> {
    let network = super::NetworkHost::new(
        Arc::clone(&config.net),
        config.base.clone(),
        config.deadline,
        config.signal.clone(),
        Arc::clone(&config.lifetime),
    );
    let completions = Arc::new(Mutex::new(Vec::new()));
    host.set_state(
        STATE_KEY,
        HostObject::new(Rc::new(RefCell::new(NetRealmState {
            network,
            wake: config.wake.clone(),
            next_id: 1,
            callbacks: HashMap::new(),
            pending: HashSet::new(),
            completions,
        }))),
    )?;
    host.register_intrinsic("__chuzzNetSync", 1, network_sync)?;
    host.register_intrinsic("__chuzzNetAsync", 2, network_async)?;
    host.register_intrinsic("__chuzzNetForget", 1, network_forget)?;
    host.register_intrinsic("__chuzzCookieGet", 0, cookie_get)?;
    host.register_intrinsic("__chuzzCookieSet", 1, cookie_set)?;
    Ok(())
}

/// Drain completed requests and invoke their rooted callbacks on the caller thread.
pub fn poll(
    host: &mut dyn ChuzzScriptHost,
    waker: Option<&std::task::Waker>,
) -> Result<bool, HostError> {
    let state = state(host)?;
    let (wake, completions) = {
        let state = state.borrow();
        (state.wake.clone(), Arc::clone(&state.completions))
    };
    wake.bind(waker);
    let deliveries = std::mem::take(
        &mut *completions
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()),
    );
    let mut changed = false;
    let mut first_error = None;
    for (id, reply) in deliveries {
        state.borrow_mut().pending.remove(&id);
        let callback = state.borrow_mut().callbacks.remove(&id);
        let Some(callback) = callback else {
            continue;
        };
        let result = response_value(reply);
        let dispatch = host.dispatch_callback(callback.clone(), &[result]);
        let release = release_callback_if_unused(host, &state, callback);
        if let Err(error) = dispatch.and(release.map(|()| HostValue::Undefined)) {
            first_error.get_or_insert(error);
        }
        changed = true;
    }
    first_error.map_or(Ok(changed), Err)
}
