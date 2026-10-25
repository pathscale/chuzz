//! Persistent bridge from the Ekojit page realm to Chuzz's native page host.
//!
//! This adapter borrows the document's existing realm. Guest callbacks are
//! always queued by the Chuzz host and are entered only from the realm poll
//! hook, after native host polling has returned.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use blitz_script_ekojit::embedder::{
    HostHandle as EkojitHandle, HostValue as EkojitValue, RealmHost,
};
use ekojit::native::NativeFailure;

use crate::page_script_host::ChuzzPageRuntimeHost;
use crate::script_host::{HostBuffer, HostCallbackId, HostError, HostObject, HostValue};

use crate::page_ekojit_channels as channels;

const STATE_KEY: &str = "chuzz.page.runtime.bridge";

// Keep this protocol in lockstep with Izumo's local IPC shim. The native
// document driver calls `install_ipc` before it polls deferred chrome scripts,
// so this object is present before any chrome module chooses its API client.
const IZUMO_IPC_SHIM: &str = r#"
(function (global) {
  "use strict";
  var pending = new Map();
  var listeners = new Map();
  var nextRequestId = 1;
  var nextListenerId = 1;

  function invoke(cmd, args) {
    if (typeof cmd !== "string" || cmd.trim().length === 0) {
      return Promise.reject(new TypeError("izumo command must be nonempty"));
    }
    if (nextRequestId > Number.MAX_SAFE_INTEGER) {
      return Promise.reject(new Error("izumo request ids exhausted"));
    }
    var id = nextRequestId++;
    return new Promise(function (resolve, reject) {
      pending.set(id, { resolve: resolve, reject: reject });
      try {
        global.ipc.postMessage(JSON.stringify({
          cmd: cmd,
          id: id,
          args: args === undefined ? {} : args
        }));
      } catch (error) {
        pending.delete(id);
        reject(error);
      }
    });
  }

  function listen(event, callback) {
    if (typeof event !== "string" || event.trim().length === 0) {
      return Promise.reject(new TypeError("izumo event must be nonempty"));
    }
    if (typeof callback !== "function") {
      return Promise.reject(new TypeError("izumo listener must be a function"));
    }
    if (nextListenerId > Number.MAX_SAFE_INTEGER) {
      return Promise.reject(new Error("izumo listener ids exhausted"));
    }
    var id = nextListenerId++;
    var callbacks = listeners.get(event);
    if (!callbacks) {
      callbacks = new Map();
      listeners.set(event, callbacks);
    }
    callbacks.set(id, callback);
    var active = true;
    return Promise.resolve(function () {
      if (!active) return;
      active = false;
      callbacks.delete(id);
      if (callbacks.size === 0) listeners.delete(event);
    });
  }

  function reply(id, success, payload) {
    var request = pending.get(id);
    if (!request) return;
    pending.delete(id);
    if (success) request.resolve(payload);
    else request.reject(payload);
  }

  function emit(event, payload) {
    var callbacks = listeners.get(event);
    if (!callbacks) return;
    var entries = Array.from(callbacks.entries());
    for (var i = 0; i < entries.length; i++) {
      var id = entries[i][0];
      var callback = entries[i][1];
      if (!callbacks.has(id)) continue;
      try {
        callback({ event: event, id: id, payload: payload });
      } catch (error) {
        if (global.console && typeof global.console.error === "function") {
          global.console.error("izumo event listener failed", error);
        }
      }
    }
  }

  global.ipc = {
    postMessage: function (message) {
      __chuzzIpcPostMessage(String(message));
    }
  };
  global.__IZUMO__ = Object.freeze({ invoke: invoke, listen: listen, _reply: reply, _emit: emit });
})(globalThis);
"#;

#[derive(Clone)]
struct OpaqueGuestHandle(EkojitHandle);

#[derive(Clone)]
struct RetainedGuestCallback {
    host_callback: HostCallbackId,
    guest_callback: EkojitHandle,
}

struct QueuedInvocation {
    callback: EkojitHandle,
    args: Vec<HostValue>,
}

struct BridgeState {
    started: std::time::Instant,
    time_origin_ms: f64,
    host: Rc<RefCell<ChuzzPageRuntimeHost>>,
    callback_queue: RefCell<VecDeque<QueuedInvocation>>,
    callbacks: RefCell<HashMap<String, RetainedGuestCallback>>,
    objects: RefCell<HashMap<String, HostObject>>,
    channels_pump: RefCell<Option<EkojitHandle>>,
}

impl BridgeState {
    fn new(host: Rc<RefCell<ChuzzPageRuntimeHost>>) -> Self {
        Self {
            started: std::time::Instant::now(),
            time_origin_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0.0, |value| value.as_secs_f64() * 1000.0),
            host,
            callback_queue: RefCell::new(VecDeque::new()),
            callbacks: RefCell::new(HashMap::new()),
            objects: RefCell::new(HashMap::new()),
            channels_pump: RefCell::new(None),
        }
    }
}

/// Install all Chuzz intrinsics and persistent callbacks on this document's
/// existing Ekojit realm, using exactly the supplied page runtime host.
pub fn install(
    document: &mut blitz_script_ekojit::EkojitDocument,
    host: Rc<RefCell<ChuzzPageRuntimeHost>>,
) -> Result<(), String> {
    let state = Rc::new(BridgeState::new(host));
    let bindings = state.host.borrow().intrinsic_bindings();
    document
        .with_realm_host(|realm| {
            realm.set_state(STATE_KEY, Rc::clone(&state))?;
            realm.set_poll_hook(Some(poll_hook))?;
            realm.register_intrinsic("__ekojitDocumentCookieGet", 0, cookie_get)?;
            realm.register_intrinsic("__ekojitDocumentCookieSet", 1, cookie_set)?;
            realm.register_intrinsic("__chuzzFormatDecimal", 2, format_decimal)?;
            realm.register_intrinsic("__chuzzMonotonicNow", 0, monotonic_now)?;
            realm.register_intrinsic("__chuzzTimeOrigin", 0, time_origin)?;
            realm.register_intrinsic("__chuzzRuntimeDiagnostics", 0, runtime_diagnostics)?;
            realm.register_intrinsic("__chuzzBrowserEnvironment", 0, browser_environment)?;
            realm.register_intrinsic("__chuzzDecodeUtf8", 1, decode_utf8)?;
            realm.register_intrinsic("__chuzzEncodeUtf8", 1, encode_utf8)?;
            realm.register_intrinsic("__chuzzCopyBytes", 1, copy_bytes)?;
            realm.register_intrinsic("__chuzzIsByteBuffer", 1, is_byte_buffer)?;
            realm.register_intrinsic("__chuzzBytesArray", 1, bytes_array)?;
            for binding in bindings {
                realm.register_intrinsic(binding.name, binding.arity, intrinsic_trampoline)?;
            }
            realm.register_intrinsic("__chuzzChannelsRegisterPump", 1, channels_register_pump)?;
            Ok(())
        })
        .map_err(native_error)?;
    channels::install(document)
}

fn intrinsic_trampoline(
    realm: &mut RealmHost<'_>,
    args: &[EkojitValue],
) -> Result<EkojitValue, NativeFailure> {
    let name = realm
        .intrinsic_name()
        .ok_or_else(|| refused("intrinsic name is unavailable"))?
        .to_owned();
    let state = bridge_state(realm)?;
    let args = args
        .iter()
        .map(|value| to_chuzz(realm, &state, value))
        .collect::<Result<Vec<_>, _>>()?;
    let result = state
        .host
        .borrow_mut()
        .invoke_intrinsic(&name, &args)
        .map_err(host_error)?;
    if name == "__chuzzPlatformCall" {
        let value = crate::script_host::to_json(&result).map_err(host_error)?;
        let wire = serde_json::to_string(&value)
            .map_err(|error| refused(&format!("platform result encoding failed: {error}")))?;
        return Ok(EkojitValue::Utf16(wire.encode_utf16().collect()));
    }
    to_ekojit(&state, &result)
}

fn poll_hook(
    realm: &mut RealmHost<'_>,
    waker: Option<&std::task::Waker>,
) -> Result<bool, NativeFailure> {
    let state = bridge_state(realm)?;
    let mut changed = state.host.borrow_mut().poll(waker).map_err(host_error)?;

    // Native host polling has returned and released its borrow. The retained
    // guest pump drains bounded native queues through the same platform host.
    if changed && let Some(callback) = state.channels_pump.borrow().clone() {
        realm.dispatch_callback(&callback, &[])?;
    }
    for _ in 0..256 {
        let invocation = state.callback_queue.borrow_mut().pop_front();
        let Some(invocation) = invocation else { break };
        let args = invocation
            .args
            .iter()
            .map(|value| to_ekojit(&state, value))
            .collect::<Result<Vec<_>, _>>()?;
        realm.dispatch_callback(&invocation.callback, &args)?;
        changed = true;
    }
    state.callbacks.borrow_mut().retain(|_, callback| {
        state
            .host
            .borrow()
            .callback_is_live(&callback.host_callback)
    });
    if !state.callback_queue.borrow().is_empty()
        && let Some(waker) = waker
    {
        waker.wake_by_ref();
    }
    Ok(changed)
}

fn channels_register_pump(
    realm: &mut RealmHost<'_>,
    args: &[EkojitValue],
) -> Result<EkojitValue, NativeFailure> {
    let Some(callback) = args.first() else {
        return Err(refused("guest channel pump callback is required"));
    };
    let EkojitValue::Handle(_) = callback else {
        return Err(refused("guest channel pump must be callable"));
    };
    let retained = realm.retain_callback(callback)?;
    let state = bridge_state(realm)?;
    *state.channels_pump.borrow_mut() = Some(retained);
    Ok(EkojitValue::Undefined)
}

fn bridge_state(realm: &RealmHost<'_>) -> Result<Rc<BridgeState>, NativeFailure> {
    realm
        .state(STATE_KEY)
        .ok_or_else(|| refused("page runtime bridge state is unavailable"))
}

fn to_chuzz(
    realm: &mut RealmHost<'_>,
    state: &Rc<BridgeState>,
    value: &EkojitValue,
) -> Result<HostValue, NativeFailure> {
    Ok(match value {
        EkojitValue::Undefined => HostValue::Undefined,
        EkojitValue::Null => HostValue::Null,
        EkojitValue::Boolean(value) => HostValue::Boolean(*value),
        EkojitValue::Number(value) => HostValue::Number(*value),
        EkojitValue::Utf16(units) => HostValue::Utf16(Rc::from(units.as_slice())),
        EkojitValue::Bytes(bytes) => HostValue::Buffer(HostBuffer::new(bytes.clone())),
        EkojitValue::Array(values) => HostValue::Array(
            values
                .iter()
                .map(|value| to_chuzz(realm, state, value))
                .collect::<Result<_, _>>()?,
        ),
        EkojitValue::Record(fields) => {
            let mut result = std::collections::BTreeMap::new();
            for (key, value) in fields {
                let key = String::from_utf16(key)
                    .map_err(|_| refused("record key contains invalid UTF-16"))?;
                result.insert(key, to_chuzz(realm, state, value)?);
            }
            HostValue::Record(result)
        }
        EkojitValue::Handle(handle) => {
            // A callable guest function becomes a Chuzz retained callback. The
            // callback only queues native data; it never enters Ekojit.
            match realm.retain_callback(value) {
                Ok(guest_callback) => {
                    let id = handle_id(handle);
                    let existing = {
                        let callbacks = state.callbacks.borrow();
                        let mut existing = None;
                        for callback in callbacks.values() {
                            if realm.same_guest_identity(&callback.guest_callback, handle)? {
                                existing = Some(callback.clone());
                                break;
                            }
                        }
                        existing
                    };
                    let callback = match existing {
                        Some(callback) => callback,
                        None => {
                            let weak_state = Rc::downgrade(state);
                            let guest_handle = handle.clone();
                            let host_callback = state
                                .host
                                .borrow_mut()
                                .retain_callback(move |_host, args| {
                                    let Some(state) = weak_state.upgrade() else {
                                        return Err(HostError::new(
                                            "InvalidStateError",
                                            "page runtime bridge has been retired",
                                        ));
                                    };
                                    state
                                        .callback_queue
                                        .borrow_mut()
                                        .push_back(QueuedInvocation {
                                            callback: guest_handle.clone(),
                                            args: args
                                                .iter()
                                                .map(clone_host_value)
                                                .collect::<Result<_, _>>()?,
                                        });
                                    Ok(HostValue::Undefined)
                                })
                                .map_err(host_error)?;
                            let callback = RetainedGuestCallback {
                                host_callback,
                                guest_callback: guest_callback.clone(),
                            };
                            state.callbacks.borrow_mut().insert(id, callback.clone());
                            callback
                        }
                    };
                    HostValue::Object(HostObject::new(callback.host_callback))
                }
                Err(_) => {
                    let id = handle_id(handle);
                    let object = state
                        .objects
                        .borrow()
                        .get(&id)
                        .cloned()
                        .unwrap_or_else(|| HostObject::new(OpaqueGuestHandle(handle.clone())));
                    state.objects.borrow_mut().insert(id, object.clone());
                    HostValue::Object(object)
                }
            }
        }
    })
}

fn to_ekojit(state: &Rc<BridgeState>, value: &HostValue) -> Result<EkojitValue, NativeFailure> {
    Ok(match value {
        HostValue::Undefined => EkojitValue::Undefined,
        HostValue::Null => EkojitValue::Null,
        HostValue::Boolean(value) => EkojitValue::Boolean(*value),
        HostValue::Number(value) => EkojitValue::Number(*value),
        HostValue::Utf16(units) => EkojitValue::Utf16(units.as_ref().to_vec()),
        HostValue::String(value) => EkojitValue::Utf16(value.encode_utf16().collect()),
        HostValue::Buffer(buffer) => EkojitValue::Bytes(
            buffer
                .as_bytes()
                .map_err(|_| refused("detached byte buffer"))?
                .to_vec(),
        ),
        HostValue::Array(values) => EkojitValue::Array(
            values
                .iter()
                .map(|value| to_ekojit(state, value))
                .collect::<Result<_, _>>()?,
        ),
        HostValue::Record(fields) => EkojitValue::Record(
            fields
                .iter()
                .map(|(key, value)| Ok((key.encode_utf16().collect(), to_ekojit(state, value)?)))
                .collect::<Result<_, NativeFailure>>()?,
        ),
        HostValue::Object(object) => {
            if let Some(callback) = object.downcast_ref::<HostCallbackId>() {
                let callbacks = state.callbacks.borrow();
                let retained = callbacks
                    .values()
                    .find(|entry| entry.host_callback.same_callback(callback))
                    .ok_or_else(|| refused("unrecognized host callback"))?;
                EkojitValue::Handle(retained.guest_callback.clone())
            } else if let Some(opaque) = object.downcast_ref::<OpaqueGuestHandle>() {
                EkojitValue::Handle(opaque.0.clone())
            } else {
                return Err(refused("opaque host object cannot cross into Ekojit"));
            }
        }
    })
}

fn clone_host_value(value: &HostValue) -> Result<HostValue, HostError> {
    Ok(match value {
        HostValue::Undefined => HostValue::Undefined,
        HostValue::Null => HostValue::Null,
        HostValue::Boolean(value) => HostValue::Boolean(*value),
        HostValue::Number(value) => HostValue::Number(*value),
        HostValue::Utf16(value) => HostValue::Utf16(Rc::clone(value)),
        HostValue::String(value) => HostValue::String(Rc::clone(value)),
        HostValue::Buffer(value) => HostValue::Buffer(HostBuffer::new(
            value
                .as_bytes()
                .map_err(|_| HostError::new("InvalidStateError", "callback buffer is detached"))?
                .to_vec(),
        )),
        HostValue::Object(value) => HostValue::Object(value.clone()),
        HostValue::Array(values) => HostValue::Array(
            values
                .iter()
                .map(clone_host_value)
                .collect::<Result<_, _>>()?,
        ),
        HostValue::Record(values) => HostValue::Record(
            values
                .iter()
                .map(|(key, value)| Ok((key.clone(), clone_host_value(value)?)))
                .collect::<Result<_, HostError>>()?,
        ),
    })
}

fn handle_id(handle: &EkojitHandle) -> String {
    // This is a storage key for one retention, not a guest equality test.
    // Callback reuse uses the realm's validated guest identity comparison.
    format!("{handle:?}")
}

fn host_error(error: HostError) -> NativeFailure {
    refused(&format!("{}: {}", error.name, error.message))
}

fn native_error(error: NativeFailure) -> String {
    format!("Ekojit page bridge installation failed: {error:?}")
}

fn refused(message: &str) -> NativeFailure {
    NativeFailure::Refused(format!("CHUZZ_EKO_BRIDGE: {message}"))
}

#[cfg(feature = "gui")]
struct IpcHandler(Box<dyn Fn(String)>);

#[cfg(feature = "gui")]
pub(super) fn install_ipc(
    document: &mut blitz_script_ekojit::EkojitDocument,
    handler: Box<dyn Fn(String)>,
) -> Result<(), String> {
    document
        .with_realm_host(|realm| {
            realm.set_state("chuzz.ipc", Rc::new(IpcHandler(handler)))?;
            realm.register_intrinsic("__chuzzIpcPostMessage", 1, ipc_post_message)
        })
        .map_err(native_error)?;
    document
        .eval_source(std::sync::Arc::new(blitz_script_ekojit::PageSource {
            text: std::sync::Arc::from(IZUMO_IPC_SHIM),
            url: std::sync::Arc::from("chuzz:ipc-install"),
        }))
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(feature = "gui")]
fn ipc_post_message(
    realm: &mut RealmHost<'_>,
    args: &[EkojitValue],
) -> Result<EkojitValue, NativeFailure> {
    let handler = realm
        .state::<IpcHandler>("chuzz.ipc")
        .ok_or_else(|| refused("IPC handler is unavailable"))?;
    let Some(EkojitValue::Utf16(units)) = args.first() else {
        return Err(refused("IPC message must be a string"));
    };
    let message =
        String::from_utf16(units).map_err(|_| refused("IPC message has invalid UTF-16"))?;
    (handler.0)(message);
    Ok(EkojitValue::Undefined)
}

/// Refresh receipt data outside a guest borrow. The intrinsic reads execution
/// counters directly from its owning realm at the time of the diagnostic call.
pub(super) fn refresh_diagnostics(
    document: &mut blitz_script_ekojit::EkojitDocument,
) -> Result<(), String> {
    let receipts = document
        .native_receipts()
        .iter()
        .map(|receipt| {
            record([
                (
                    "sourceUrl",
                    EkojitValue::Utf16(receipt.source_url.encode_utf16().collect()),
                ),
                (
                    "backend",
                    EkojitValue::Utf16(receipt.backend.encode_utf16().collect()),
                ),
                (
                    "entryAbi",
                    EkojitValue::Utf16(receipt.entry_abi.encode_utf16().collect()),
                ),
                (
                    "nativeEntries",
                    EkojitValue::Number(receipt.native_entries as f64),
                ),
                ("hostCalls", EkojitValue::Number(receipt.host_calls as f64)),
                ("codeBytes", EkojitValue::Number(receipt.code_bytes as f64)),
            ])
        })
        .collect::<Vec<_>>();
    document
        .with_realm_host(|realm| realm.set_state("chuzz.runtime.receipts", Rc::new(receipts)))
        .map_err(native_error)
}

fn record<const N: usize>(fields: [(&str, EkojitValue); N]) -> EkojitValue {
    EkojitValue::Record(
        fields
            .into_iter()
            .map(|(key, value)| (key.encode_utf16().collect(), value))
            .collect(),
    )
}

fn runtime_diagnostics(
    realm: &mut RealmHost<'_>,
    _args: &[EkojitValue],
) -> Result<EkojitValue, NativeFailure> {
    let (compilations, entries) = realm.native_counts();
    let receipts = realm.state::<Vec<EkojitValue>>("chuzz.runtime.receipts");
    Ok(record([
        (
            "engine",
            EkojitValue::Utf16("ekojit/ps-codegen".encode_utf16().collect()),
        ),
        ("compilations", EkojitValue::Number(compilations as f64)),
        ("nativeEntries", EkojitValue::Number(entries as f64)),
        (
            "receipts",
            EkojitValue::Array(receipts.map_or_else(Vec::new, |value| (*value).clone())),
        ),
    ]))
}

fn browser_environment(
    realm: &mut RealmHost<'_>,
    _args: &[EkojitValue],
) -> Result<EkojitValue, NativeFailure> {
    let state = bridge_state(realm)?;
    let host = state.host.borrow();
    let user_agent = host.net_provider().map_err(host_error)?.user_agent();
    Ok(record([
        (
            "userAgent",
            EkojitValue::Utf16(user_agent.encode_utf16().collect()),
        ),
        (
            "language",
            EkojitValue::Utf16("en-US".encode_utf16().collect()),
        ),
        (
            "languages",
            EkojitValue::Array(vec![
                EkojitValue::Utf16("en-US".encode_utf16().collect()),
                EkojitValue::Utf16("en".encode_utf16().collect()),
            ]),
        ),
        (
            "hardwareConcurrency",
            EkojitValue::Number(
                std::thread::available_parallelism().map_or(1, |value| value.get()) as f64,
            ),
        ),
    ]))
}

fn decode_utf8(
    _realm: &mut RealmHost<'_>,
    args: &[EkojitValue],
) -> Result<EkojitValue, NativeFailure> {
    let bytes = match args.first() {
        Some(EkojitValue::Bytes(bytes)) => bytes.clone(),
        Some(EkojitValue::Array(values)) => values
            .iter()
            .map(|value| match value {
                EkojitValue::Number(value)
                    if value.is_finite()
                        && *value >= 0.0
                        && *value <= 255.0
                        && value.fract() == 0.0 =>
                {
                    Ok(*value as u8)
                }
                _ => Err(refused("UTF-8 source must contain bytes")),
            })
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err(refused("UTF-8 source must be a byte buffer")),
    };
    Ok(EkojitValue::Utf16(
        String::from_utf8_lossy(&bytes).encode_utf16().collect(),
    ))
}

fn encode_utf8(
    _realm: &mut RealmHost<'_>,
    args: &[EkojitValue],
) -> Result<EkojitValue, NativeFailure> {
    let Some(EkojitValue::Utf16(units)) = args.first() else {
        return Err(refused("UTF-8 input must be a string"));
    };
    Ok(EkojitValue::Bytes(
        String::from_utf16_lossy(units).into_bytes(),
    ))
}
fn copy_bytes(
    _realm: &mut RealmHost<'_>,
    args: &[EkojitValue],
) -> Result<EkojitValue, NativeFailure> {
    match args.first() {
        Some(EkojitValue::Bytes(bytes)) => Ok(EkojitValue::Bytes(bytes.clone())),
        Some(EkojitValue::Array(values)) => values
            .iter()
            .map(|value| match value {
                EkojitValue::Number(value)
                    if value.is_finite()
                        && *value >= 0.0
                        && *value <= 255.0
                        && value.fract() == 0.0 =>
                {
                    Ok(*value as u8)
                }
                _ => Err(refused("byte array contains a non-byte")),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(EkojitValue::Bytes),
        _ => Err(refused("byte buffer required")),
    }
}
fn is_byte_buffer(
    _realm: &mut RealmHost<'_>,
    args: &[EkojitValue],
) -> Result<EkojitValue, NativeFailure> {
    Ok(EkojitValue::Boolean(matches!(
        args.first(),
        Some(EkojitValue::Bytes(_))
    )))
}
fn bytes_array(
    realm: &mut RealmHost<'_>,
    args: &[EkojitValue],
) -> Result<EkojitValue, NativeFailure> {
    let EkojitValue::Bytes(bytes) = copy_bytes(realm, args)? else {
        unreachable!()
    };
    Ok(EkojitValue::Array(
        bytes
            .into_iter()
            .map(|byte| EkojitValue::Number(f64::from(byte)))
            .collect(),
    ))
}

fn monotonic_now(
    realm: &mut RealmHost<'_>,
    _args: &[EkojitValue],
) -> Result<EkojitValue, NativeFailure> {
    Ok(EkojitValue::Number(
        bridge_state(realm)?.started.elapsed().as_secs_f64() * 1000.0,
    ))
}
fn time_origin(
    realm: &mut RealmHost<'_>,
    _args: &[EkojitValue],
) -> Result<EkojitValue, NativeFailure> {
    Ok(EkojitValue::Number(bridge_state(realm)?.time_origin_ms))
}

fn cookie_get(
    realm: &mut RealmHost<'_>,
    _args: &[EkojitValue],
) -> Result<EkojitValue, NativeFailure> {
    let state = bridge_state(realm)?;
    let network = state.host.borrow().script_network().map_err(host_error)?;
    Ok(EkojitValue::Utf16(
        network.cookie_get().encode_utf16().collect(),
    ))
}
fn cookie_set(
    realm: &mut RealmHost<'_>,
    args: &[EkojitValue],
) -> Result<EkojitValue, NativeFailure> {
    let assignment = match args.first() {
        Some(EkojitValue::Utf16(value)) => String::from_utf16_lossy(value),
        _ => return Err(refused("cookie assignment must be a string")),
    };
    let state = bridge_state(realm)?;
    let network = state.host.borrow().script_network().map_err(host_error)?;
    network
        .cookie_set(&assignment)
        .map_err(|error| refused(&error.to_string()))?;
    Ok(EkojitValue::Undefined)
}

// ICU data supplies locale digits, signs and grouping rather than a locale table in JS.
fn format_decimal(
    _realm: &mut RealmHost<'_>,
    args: &[EkojitValue],
) -> Result<EkojitValue, NativeFailure> {
    let Some(EkojitValue::Utf16(locale)) = args.first() else {
        return Err(refused("number locale must be a string"));
    };
    let Some(EkojitValue::Utf16(number)) = args.get(1) else {
        return Err(refused("decimal input must be a string"));
    };
    let locale: icu_locale::Locale = String::from_utf16(locale)
        .map_err(|_| refused("invalid locale UTF-16"))?
        .parse()
        .map_err(|_| refused("invalid number locale"))?;
    let decimal: icu_decimal::input::Decimal = String::from_utf16(number)
        .map_err(|_| refused("invalid decimal UTF-16"))?
        .parse()
        .map_err(|_| refused("invalid finite decimal"))?;
    let formatter = icu_decimal::DecimalFormatter::try_new(locale.into(), Default::default())
        .map_err(|_| refused("number locale data unavailable"))?;
    Ok(EkojitValue::Utf16(
        formatter
            .format_to_string(&decimal)
            .encode_utf16()
            .collect(),
    ))
}
