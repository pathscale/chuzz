use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;

use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256, Sha384, Sha512};

use crate::script_host::{
    ChuzzScriptHost, HostCallbackId, HostError, HostInstallContext, HostObject, HostValue,
    from_json, to_json,
};

const STATE_KEY: &str = "chuzz.webapi";
const LIMIT: usize = 64 * 1024 * 1024;
const COMPLETION_LIMIT: usize = 64;

struct Realm {
    native: Arc<super::Realm>,
    callbacks: RefCell<std::collections::HashMap<u64, HostCallbackId>>,
    pending: RefCell<VecDeque<(u64, Arc<Json>)>>,
}

fn type_error(message: impl std::fmt::Display) -> HostError {
    HostError::new("TypeError", message.to_string())
}

fn realm(host: &dyn ChuzzScriptHost) -> Result<Rc<Realm>, HostError> {
    let object = host
        .state(STATE_KEY)
        .ok_or_else(|| type_error("web API realm is unavailable"))?;
    object
        .downcast_ref::<Rc<Realm>>()
        .cloned()
        .ok_or_else(|| type_error("web API realm state is invalid"))
}

fn random_bytes() -> Result<[u8; 16], HostError> {
    let provider = rustls::crypto::CryptoProvider::get_default()
        .ok_or_else(|| type_error("secure random provider is unavailable"))?;
    let mut bytes = [0_u8; 16];
    provider
        .secure_random
        .fill(&mut bytes)
        .map_err(|_| type_error("secure random generation failed"))?;
    Ok(bytes)
}

fn reply_json(reply: super::Reply) -> Json {
    match reply {
        super::Reply::Info(info) => json!({
            "origin": info.origin,
            "secure": info.secure,
            "platform": info.platform,
            "uaPlatform": info.ua_platform,
            "architecture": info.architecture,
            "bitness": info.bitness,
            "concurrency": info.concurrency,
            "version": info.version,
            "languages": info.languages,
        }),
        super::Reply::Random(bytes) => json!(bytes),
        super::Reply::Id(id) => json!(id),
        super::Reply::Null => Json::Null,
        super::Reply::BroadcastDeliveries(deliveries) => Json::Array(
            deliveries
                .into_iter()
                .map(|delivery| {
                    json!({
                        "id": delivery.id,
                        "wire": delivery.wire.as_ref(),
                    })
                })
                .collect(),
        ),
        super::Reply::LockQuery { held, pending } => {
            let entries = |entries: Vec<super::LockEntry>| {
                entries
                    .into_iter()
                    .map(|entry| {
                        json!({
                            "name": entry.name,
                            "mode": entry.mode,
                            "clientId": entry.client_id,
                        })
                    })
                    .collect::<Vec<_>>()
            };
            json!({
                "held": entries(held),
                "pending": entries(pending),
            })
        }
        super::Reply::Boolean(value) => json!(value),
        super::Reply::Webauthn(value) => value,
    }
}

fn operation_from_arg(args: &[HostValue]) -> Result<super::Operation, HostError> {
    let value = match args.first() {
        Some(HostValue::String(_)) | Some(HostValue::Utf16(_)) => {
            let wire = crate::script_host::string_arg(args, 0)?;
            if wire.len() > LIMIT {
                return Err(type_error("web API message exceeds the allocation limit"));
            }
            serde_json::from_str(&wire).map_err(type_error)?
        }
        Some(_) => {
            let value = to_json(&args[0])?;
            if value.to_string().len() > LIMIT {
                return Err(type_error("web API message exceeds the allocation limit"));
            }
            value
        }
        None => return Err(type_error("web API message is required")),
    };
    serde_json::from_value(value).map_err(type_error)
}

/// Dispatch a native web API operation represented as its existing JSON command
/// or as a structured host value.
pub fn dispatch(
    host: &mut dyn ChuzzScriptHost,
    args: &[HostValue],
) -> Result<HostValue, HostError> {
    let realm = realm(host)?;
    let operation = operation_from_arg(args)?;
    let reply = super::dispatch(&realm.native, operation).map_err(type_error)?;
    match reply {
        super::Reply::Random(bytes) => Ok(HostValue::Buffer(crate::script_host::HostBuffer::new(
            bytes.to_vec(),
        ))),
        reply => Ok(from_json(reply_json(reply))),
    }
}

fn codec(_host: &mut dyn ChuzzScriptHost, args: &[HostValue]) -> Result<HostValue, HostError> {
    let format = crate::script_host::string_arg(args, 0)?;
    if !matches!(format.as_str(), "gzip" | "deflate" | "deflate-raw") {
        return Err(type_error("unsupported compression format"));
    }
    let input = match args.get(1) {
        Some(HostValue::Buffer(buffer)) => buffer
            .as_bytes()
            .map_err(|_| type_error("detached compression input"))?,
        _ => return Err(type_error("compression requires an ArrayBuffer")),
    };
    let decode = match args.get(2) {
        None => false,
        Some(HostValue::Boolean(value)) => *value,
        Some(_) => return Err(type_error("compression decode flag must be a boolean")),
    };
    let output = super::compress(&format, input, decode).map_err(type_error)?;
    Ok(HostValue::Buffer(crate::script_host::HostBuffer::new(
        output,
    )))
}

fn digest(_host: &mut dyn ChuzzScriptHost, args: &[HostValue]) -> Result<HostValue, HostError> {
    let algorithm = crate::script_host::string_arg(args, 0)?.to_ascii_uppercase();
    let input = match args.get(1) {
        Some(HostValue::Buffer(buffer)) => buffer
            .as_bytes()
            .map_err(|_| type_error("detached digest input"))?,
        _ => return Err(type_error("digest requires an ArrayBuffer")),
    };
    if input.len() > LIMIT {
        return Err(type_error("digest allocation limit exceeded"));
    }
    let output = match algorithm.as_str() {
        "SHA-256" => Sha256::digest(input).to_vec(),
        "SHA-384" => Sha384::digest(input).to_vec(),
        "SHA-512" => Sha512::digest(input).to_vec(),
        _ => return Err(type_error("unsupported digest algorithm")),
    };
    Ok(HostValue::Buffer(crate::script_host::HostBuffer::new(
        output,
    )))
}

fn random(_host: &mut dyn ChuzzScriptHost, _args: &[HostValue]) -> Result<HostValue, HostError> {
    Ok(HostValue::Buffer(crate::script_host::HostBuffer::new(
        random_bytes()?.to_vec(),
    )))
}

fn subscribe_webauthn(
    host: &mut dyn ChuzzScriptHost,
    args: &[HostValue],
) -> Result<HostValue, HostError> {
    let realm = realm(host)?;
    let id = match args.first() {
        Some(HostValue::Number(value))
            if value.is_finite()
                && *value >= 0.0
                && *value <= 9_007_199_254_740_991.0
                && value.fract() == 0.0 =>
        {
            *value as u64
        }
        _ => {
            return Err(type_error(
                "WebAuthn request id must be a non-negative integer",
            ));
        }
    };
    let callback = match args.get(1) {
        Some(HostValue::Object(object)) => object
            .downcast_ref::<HostCallbackId>()
            .cloned()
            .ok_or_else(|| type_error("WebAuthn callback root is invalid"))?,
        _ => return Err(type_error("WebAuthn callback root is required")),
    };
    let previous = {
        let mut callbacks = realm.callbacks.borrow_mut();
        if callbacks.len() >= COMPLETION_LIMIT && !callbacks.contains_key(&id) {
            return Err(HostError::new(
                "QuotaExceededError",
                "too many WebAuthn subscriptions",
            ));
        }
        callbacks.insert(id, callback)
    };
    if let Some(previous) = previous {
        host.release_callback(previous)?;
    }
    Ok(HostValue::Null)
}

fn register(
    host: &mut dyn ChuzzScriptHost,
    name: &'static str,
    arity: u8,
    callback: crate::script_host::HostIntrinsic,
) -> Result<(), HostError> {
    host.register_intrinsic(name, arity, callback)
}

/// Install the engine-neutral web API operations into one realm.
pub fn install(
    host: &mut dyn ChuzzScriptHost,
    config: &HostInstallContext,
) -> Result<(), HostError> {
    let native = super::new_realm(config.base.as_ref(), config.wake.clone());
    host.set_state(
        STATE_KEY,
        HostObject::new(Rc::new(Realm {
            native,
            callbacks: RefCell::default(),
            pending: RefCell::default(),
        })),
    )?;
    register(host, "__chuzzPlatformCall", 1, dispatch)?;
    register(host, "__chuzzCodec", 3, codec)?;
    register(host, "__chuzzRandomBytes", 0, random)?;
    register(host, "__chuzzDigest", 2, digest)?;
    register(host, "__chuzzWebauthnSubscribe", 2, subscribe_webauthn)?;
    Ok(())
}

/// Bind the realm waker and dispatch completed WebAuthn results on the caller
/// thread. The renderer must register each request callback with
/// `__chuzzWebauthnSubscribe`.
pub fn poll(
    host: &mut dyn ChuzzScriptHost,
    waker: Option<&std::task::Waker>,
) -> Result<bool, HostError> {
    let realm = realm(host)?;
    realm.native.notifications.bind(waker);

    let deliveries = realm.native.notifications.take(realm.native.id);
    {
        let mut pending = realm.pending.borrow_mut();
        for delivery in deliveries {
            if pending.len() >= COMPLETION_LIMIT {
                return Err(type_error("WebAuthn completion limit reached"));
            }
            pending.push_back(delivery);
        }
    }

    let mut changed = false;
    loop {
        let delivery = {
            let mut pending = realm.pending.borrow_mut();
            let Some(position) = pending
                .iter()
                .position(|(id, _)| realm.callbacks.borrow().contains_key(id))
            else {
                break;
            };
            pending.remove(position)
        };
        let Some((id, result)) = delivery else {
            break;
        };
        let callback = realm
            .callbacks
            .borrow_mut()
            .remove(&id)
            .ok_or_else(|| type_error("WebAuthn completion callback is unavailable"))?;
        let dispatch = host.dispatch_callback(
            callback.clone(),
            &[HostValue::Number(id as f64), from_json((*result).clone())],
        );
        let release = host.release_callback(callback);
        dispatch?;
        release?;
        changed = true;
    }
    Ok(changed)
}
