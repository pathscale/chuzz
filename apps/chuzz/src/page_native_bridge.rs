//! Typed bridge between one guest realm and its page native host.
//!
//! This module deliberately keeps guest callbacks as values. Host callbacks only
//! enqueue jobs; execution resumes later while RuntimeBridge::poll holds the live
//! document borrow.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::task::Waker;
use std::time::Duration;

use crate::page_script_host::ChuzzPageRuntimeHost;
use crate::script_host::{ChuzzScriptHost, HostBuffer, HostCallbackId, HostError, HostValue};
use blitz_dom::BaseDocument;
use blitz_script::native::{HostJob, Property, RuntimeBridge, Value};

const MAX_CONVERSION_DEPTH: usize = 128;
const MAX_CONVERSION_ITEMS: usize = 65_536;
const MAX_STORAGE_ENTRIES: usize = 4096;
const MAX_STORAGE_UNITS: usize = 1_048_576;
const MAX_BUFFER_BYTES: usize = 64 * 1024 * 1024;

fn typed_array_bytes(kind: &str, items: &[Value]) -> blitz_script::native::Result<Option<Vec<u8>>> {
    use blitz_script::native::Error;
    let width = match kind {
        "Uint8Array" | "Uint8ClampedArray" | "Int8Array" => 1,
        "Uint16Array" | "Int16Array" => 2,
        "Uint32Array" | "Int32Array" | "Float32Array" => 4,
        "Float64Array" => 8,
        _ => return Ok(None),
    };
    let length = items
        .len()
        .checked_mul(width)
        .filter(|length| *length <= MAX_BUFFER_BYTES)
        .ok_or_else(|| Error::new("RangeError", "typed array exceeds the host transfer limit"))?;
    let mut bytes = Vec::with_capacity(length);
    for item in items {
        let number = item.number_value();
        let part = match kind {
            "Uint8Array" | "Uint8ClampedArray" | "Int8Array" => vec![if number.is_finite() {
                number.trunc().rem_euclid(256.0) as u8
            } else {
                0
            }],
            "Uint16Array" => (if number.is_finite() {
                number.trunc().rem_euclid(65_536.0) as u16
            } else {
                0
            })
            .to_le_bytes()
            .to_vec(),
            "Int16Array" => (if number.is_finite() { number as i16 } else { 0 })
                .to_le_bytes()
                .to_vec(),
            "Uint32Array" => (if number.is_finite() {
                number.trunc().rem_euclid(4_294_967_296.0) as u32
            } else {
                0
            })
            .to_le_bytes()
            .to_vec(),
            "Int32Array" => (if number.is_finite() { number as i32 } else { 0 })
                .to_le_bytes()
                .to_vec(),
            "Float32Array" => (number as f32).to_le_bytes().to_vec(),
            "Float64Array" => number.to_le_bytes().to_vec(),
            _ => Vec::new(),
        };
        bytes.extend(part);
    }
    Ok(Some(bytes))
}

/// The bridge is the sole owner of the exact host supplied to page construction.
pub struct ChuzzBridge {
    pub host: ChuzzPageRuntimeHost,
    /// Shared with retained host closures so they never re-enter Guest.
    pub(super) pending: Rc<RefCell<Vec<HostJob>>>,
    completed_timers: Rc<RefCell<Vec<u64>>>,
    /// Identity roots for host objects exposed as guest objects.
    host_objects: Vec<(crate::script_host::HostObject, Value)>,
    /// Local storage is page-realm state; session storage remains host-owned.
    local_storage: Rc<RefCell<BTreeMap<Vec<u16>, Vec<u16>>>>,
    storage_units: usize,
    timer_callbacks: BTreeMap<u64, (HostCallbackId, bool)>,
    pub(super) state: BTreeMap<String, Value>,
    torn_down: bool,
}

impl ChuzzBridge {
    pub fn new(host: ChuzzPageRuntimeHost) -> Result<Self, String> {
        Ok(Self {
            host,
            pending: Rc::new(RefCell::new(Vec::new())),
            completed_timers: Rc::new(RefCell::new(Vec::new())),
            host_objects: Vec::new(),
            local_storage: Rc::new(RefCell::new(BTreeMap::new())),
            storage_units: 0,
            timer_callbacks: BTreeMap::new(),
            state: BTreeMap::new(),
            torn_down: false,
        })
    }

    /// Invoke a named page intrinsic with direct typed values.
    pub(super) fn invoke(
        &mut self,
        name: &str,
        args: Vec<Value>,
    ) -> blitz_script::native::Result<Value> {
        let host_args = args
            .into_iter()
            .map(|value| self.encode_host_value(value))
            .collect::<blitz_script::native::Result<Vec<_>>>()?;
        self.host
            .invoke_intrinsic(name, &host_args)
            .map(|value| self.decode_host_value(value))
            .map_err(host_error)
    }

    /// Convert without JSON, preserving UTF-16 code units and object roots.
    pub(super) fn encode_host_value(
        &mut self,
        value: Value,
    ) -> blitz_script::native::Result<HostValue> {
        self.encode_host_value_inner(value, 0)
    }

    fn encode_host_value_inner(
        &mut self,
        value: Value,
        depth: usize,
    ) -> blitz_script::native::Result<HostValue> {
        use blitz_script::native::Error;
        if depth > MAX_CONVERSION_DEPTH {
            return Err(Error::new(
                "RangeError",
                "host value nesting limit exceeded",
            ));
        }
        match value {
            Value::Undefined => Ok(HostValue::Undefined),
            Value::Null => Ok(HostValue::Null),
            Value::Bool(value) => Ok(HostValue::Boolean(value)),
            Value::Number(value) => Ok(HostValue::Number(value)),
            Value::String(units) => Ok(HostValue::Utf16(Rc::clone(&units))),
            Value::Object(object) => {
                let borrowed = object.borrow();
                if let Some(bytes) = typed_array_bytes(&borrowed.kind, &borrowed.indexed)? {
                    return Ok(HostValue::Buffer(HostBuffer::new(bytes)));
                }
                if let Some(root) = borrowed
                    .payload
                    .as_ref()
                    .and_then(|p| p.downcast_ref::<crate::script_host::HostObject>())
                {
                    return Ok(HostValue::Object(root.clone()));
                }
                if let Some(callback) = borrowed
                    .payload
                    .as_ref()
                    .and_then(|p| p.downcast_ref::<HostCallbackId>())
                {
                    return Ok(HostValue::Object(crate::script_host::HostObject::new(
                        callback.clone(),
                    )));
                }
                if let Some(buffer) = borrowed
                    .payload
                    .as_ref()
                    .and_then(|p| p.downcast_ref::<RefCell<HostBuffer>>())
                {
                    let buffer = buffer.borrow();
                    let bytes = buffer
                        .as_bytes()
                        .map_err(|_| Error::type_error("detached host buffer"))?
                        .to_vec();
                    return Ok(HostValue::Buffer(HostBuffer::new(bytes)));
                }
                if let Some(buffer) = borrowed
                    .payload
                    .as_ref()
                    .and_then(|p| p.downcast_ref::<HostBuffer>())
                {
                    let bytes = buffer
                        .as_bytes()
                        .map_err(|_| Error::type_error("detached host buffer"))?
                        .to_vec();
                    return Ok(HostValue::Buffer(HostBuffer::new(bytes)));
                }
                let items = borrowed.indexed.clone();
                let props = borrowed
                    .props
                    .iter()
                    .filter(|(_, property)| {
                        property.enumerable && property.get.is_none() && property.set.is_none()
                    })
                    .map(|(key, property)| (key.clone(), property.value.clone()))
                    .collect::<Vec<_>>();
                let is_array = borrowed.kind == "Array" || borrowed.kind == "NodeList";
                drop(borrowed);
                if is_array {
                    if items.len() > MAX_CONVERSION_ITEMS {
                        return Err(Error::new("RangeError", "host array item limit exceeded"));
                    }
                    return items
                        .into_iter()
                        .map(|item| self.encode_host_value_inner(item, depth + 1))
                        .collect::<blitz_script::native::Result<Vec<_>>>()
                        .map(HostValue::Array);
                }
                if props.len() > MAX_CONVERSION_ITEMS {
                    return Err(Error::new("RangeError", "host record item limit exceeded"));
                }
                let mut record = BTreeMap::new();
                for (key, item) in props {
                    record.insert(key, self.encode_host_value_inner(item, depth + 1)?);
                }
                Ok(HostValue::Record(record))
            }
        }
    }

    /// Convert a typed host value while retaining opaque object identity.
    pub(super) fn decode_host_value(&mut self, value: HostValue) -> Value {
        match value {
            HostValue::Undefined => Value::Undefined,
            HostValue::Null => Value::Null,
            HostValue::Boolean(value) => Value::Bool(value),
            HostValue::Number(value) => Value::Number(value),
            HostValue::Utf16(value) => Value::String(value),
            HostValue::String(value) => Value::string(value.as_ref()),
            HostValue::Buffer(buffer) => {
                let bytes = match buffer.as_bytes() {
                    Ok(bytes) => bytes.to_vec(),
                    Err(_) => return Value::error("TypeError", "host supplied a detached buffer"),
                };
                let value = Value::object("Uint8Array");
                if let Some(object) = value.as_object() {
                    let mut object = object.borrow_mut();
                    object.indexed = bytes
                        .iter()
                        .map(|byte| Value::Number(f64::from(*byte)))
                        .collect();
                    object.payload = Some(Rc::new(RefCell::new(buffer)) as Rc<dyn Any>);
                }
                value
            }
            HostValue::Object(root) => {
                if let Some((_, value)) = self
                    .host_objects
                    .iter()
                    .find(|(known, _)| known.same_identity(&root))
                {
                    return value.clone();
                }
                let value = Value::object("HostObject");
                if let Some(object) = value.as_object() {
                    object.borrow_mut().payload = Some(Rc::new(root.clone()) as Rc<dyn Any>);
                }
                self.host_objects.push((root, value.clone()));
                value
            }
            HostValue::Array(values) => Value::array(
                values
                    .into_iter()
                    .map(|item| self.decode_host_value(item))
                    .collect(),
            ),
            HostValue::Record(values) => {
                let value = Value::object("Object");
                let Some(object) = value.as_object() else {
                    return Value::Undefined;
                };
                let mut object = object.borrow_mut();
                for (key, item) in values {
                    object
                        .props
                        .insert(key, Property::data(self.decode_host_value(item)));
                }
                drop(object);
                value
            }
        }
    }

    fn storage_value(&self, session: bool) -> Value {
        let kind = if session {
            "SessionStorage"
        } else {
            "LocalStorage"
        };
        let value = Value::object(kind);
        let Some(object) = value.as_object() else {
            return Value::Undefined;
        };
        let mut object = object.borrow_mut();
        for (method, intrinsic) in [
            (
                "getItem",
                if session {
                    "__chuzzSessionGetItem"
                } else {
                    "__bridgeLocalGet"
                },
            ),
            (
                "setItem",
                if session {
                    "__chuzzSessionSetItem"
                } else {
                    "__bridgeLocalSet"
                },
            ),
            (
                "removeItem",
                if session {
                    "__chuzzSessionRemoveItem"
                } else {
                    "__bridgeLocalRemove"
                },
            ),
            (
                "clear",
                if session {
                    "__chuzzSessionClear"
                } else {
                    "__bridgeLocalClear"
                },
            ),
            (
                "key",
                if session {
                    "__chuzzSessionKey"
                } else {
                    "__bridgeLocalKey"
                },
            ),
        ] {
            object
                .props
                .insert(method.into(), Property::data(Value::native(intrinsic)));
        }
        drop(object);
        value
    }

    fn local_storage_call(
        &mut self,
        name: &str,
        args: Vec<Value>,
    ) -> blitz_script::native::Result<Option<Value>> {
        use blitz_script::native::Error;
        let mut map = self.local_storage.borrow_mut();
        let units = |value: Option<&Value>| match value {
            Some(Value::String(units)) => units.to_vec(),
            value => value
                .unwrap_or(&Value::Undefined)
                .text()
                .encode_utf16()
                .collect::<Vec<_>>(),
        };
        let key = || units(args.first());
        match name {
            "__bridgeLocalGet" => {
                let key = key();
                Ok(Some(
                    map.get(&key)
                        .map(|value| Value::String(Rc::from(value.clone())))
                        .unwrap_or(Value::Null),
                ))
            }
            "__bridgeLocalSet" => {
                let key = key();
                let value = units(args.get(1));
                let old = map.get(&key).map_or(0, Vec::len);
                let previous = if map.contains_key(&key) {
                    key.len() + old
                } else {
                    0
                };
                let next = self
                    .storage_units
                    .saturating_sub(previous)
                    .saturating_add(key.len())
                    .saturating_add(value.len());
                if map.len() >= MAX_STORAGE_ENTRIES && !map.contains_key(&key)
                    || next > MAX_STORAGE_UNITS
                {
                    return Err(Error::new("QuotaExceededError", "storage quota exceeded"));
                }
                map.insert(key, value);
                self.storage_units = next;
                Ok(Some(Value::Undefined))
            }
            "__bridgeLocalRemove" => {
                let key = key();
                if let Some(value) = map.remove(&key) {
                    self.storage_units = self.storage_units.saturating_sub(key.len() + value.len());
                }
                Ok(Some(Value::Undefined))
            }
            "__bridgeLocalClear" => {
                map.clear();
                self.storage_units = 0;
                Ok(Some(Value::Undefined))
            }
            "__bridgeLocalKey" => {
                let index = args
                    .first()
                    .map(Value::number_value)
                    .unwrap_or(0.0)
                    .max(0.0) as usize;
                Ok(Some(
                    map.keys()
                        .nth(index)
                        .map(|key| Value::String(Rc::from(key.clone())))
                        .unwrap_or(Value::Null),
                ))
            }
            _ => Ok(None),
        }
    }

    fn schedule_guest_callback(
        &mut self,
        callback: Value,
        this: Value,
        delay_ms: f64,
        repeat: bool,
        arguments: Vec<Value>,
    ) -> blitz_script::native::Result<Value> {
        use blitz_script::native::Error;
        if !delay_ms.is_finite() || delay_ms < 0.0 {
            return Err(Error::new(
                "RangeError",
                "timer delay must be a finite nonnegative number",
            ));
        }
        if !matches!(callback.kind().as_str(), "Function" | "Native") {
            return Err(Error::type_error("timer callback must be callable"));
        }
        let queue = Rc::clone(&self.pending);
        let completed = Rc::clone(&self.completed_timers);
        let timer_identity = Rc::new(Cell::new(None));
        let callback_timer_identity = Rc::clone(&timer_identity);
        let job_callback = callback.clone();
        let job_this = this.clone();
        let callback_id = self
            .host
            .retain_callback(move |_host, args| {
                let values = if args.is_empty() {
                    arguments.clone()
                } else {
                    args.iter().map(host_callback_value).collect()
                };
                queue.borrow_mut().push(HostJob {
                    callback: job_callback.clone(),
                    this: job_this.clone(),
                    args: values,
                });
                if let Some(id) = callback_timer_identity.get() {
                    completed.borrow_mut().push(id);
                }
                Ok(HostValue::Undefined)
            })
            .map_err(host_error)?;
        let delay = Duration::from_millis(delay_ms.min(u64::MAX as f64) as u64);
        let timer_id = self
            .host
            .schedule_timer(callback_id.clone(), delay, repeat)
            .map_err(host_error)?;
        timer_identity.set(Some(timer_id));
        self.timer_callbacks.insert(timer_id, (callback_id, repeat));
        Ok(Value::Number(timer_id as f64))
    }

    fn cancel_guest_timer(&mut self, value: &Value) -> blitz_script::native::Result<Value> {
        let id = value.number_value().max(0.0) as u64;
        let cancelled = self.host.cancel_timer(id).map_err(host_error)?;
        if let Some((callback, _)) = self.timer_callbacks.remove(&id) {
            let _ = self.host.release_callback(callback);
        }
        Ok(Value::Bool(cancelled))
    }

    fn environment_get(&self, doc: &BaseDocument, receiver: &Value, key: &str) -> Option<Value> {
        match (receiver.kind().as_str(), key) {
            ("Navigator", "userAgent") => {
                Some(Value::string(self.host.net_provider().ok()?.user_agent()))
            }
            ("Location", "href") => Some(Value::string(doc.url().as_str())),
            ("Location", "origin") => Some(Value::string(doc.url().origin().ascii_serialization())),
            ("Location", "pathname") => Some(Value::string(doc.url().path())),
            ("Location", "search") => Some(Value::string(if doc.url().query().is_some() {
                format!("?{}", doc.url().query().unwrap_or_default())
            } else {
                String::new()
            })),
            ("Location", "hash") => Some(Value::string(if doc.url().fragment().is_some() {
                format!("#{}", doc.url().fragment().unwrap_or_default())
            } else {
                String::new()
            })),
            _ => None,
        }
    }
}

impl RuntimeBridge for ChuzzBridge {
    fn wait_for_work(&mut self) -> blitz_script::native::Result<()> {
        nagoya::block_on(nagoya::sleep(Duration::from_millis(1)));
        Ok(())
    }
    fn install(&mut self, global: &Value) -> blitz_script::native::Result<()> {
        self.state.insert("realm.global".into(), global.clone());
        let navigator = Value::object("Navigator");
        navigator.put("language", Value::string("en-US"));
        navigator.put(
            "languages",
            Value::array(vec![Value::string("en-US"), Value::string("en")]),
        );
        global.put("navigator", navigator);
        global.put("location", Value::object("Location"));
        for name in [
            "setTimeout",
            "setInterval",
            "clearTimeout",
            "clearInterval",
            "requestAnimationFrame",
            "cancelAnimationFrame",
        ] {
            global.put(name, Value::native(format!("Window.{name}")));
        }
        let global_ref = global.as_object().ok_or_else(|| {
            blitz_script::native::Error::type_error("global object is unavailable")
        })?;
        global_ref.borrow_mut().props.insert(
            "localStorage".into(),
            Property::data(self.storage_value(false)),
        );
        global_ref.borrow_mut().props.insert(
            "sessionStorage".into(),
            Property::data(self.storage_value(true)),
        );
        self.install_gpu(global)?;
        self.install_media(global)?;
        Ok(())
    }

    fn get(
        &mut self,
        doc: &mut BaseDocument,
        receiver: &Value,
        key: &str,
    ) -> blitz_script::native::Result<Option<Value>> {
        if receiver.kind() == "Navigator" && key == "sendBeacon" {
            return Ok(Some(Value::native("Browser.sendBeacon")));
        }
        if receiver.kind() == "LocalStorage" && key == "length" {
            return Ok(Some(
                Value::Number(self.local_storage.borrow().len() as f64),
            ));
        }
        if receiver.kind() == "SessionStorage" && key == "length" {
            return self.invoke("__chuzzSessionLength", Vec::new()).map(Some);
        }
        if let Some(value) = self.environment_get(doc, receiver, key) {
            return Ok(Some(value));
        }
        if let Some(value) = self.gpu_get(doc, receiver, key)? {
            return Ok(Some(value));
        }
        if let Some(value) = self.media_get(doc, receiver, key)? {
            return Ok(Some(value));
        }
        if receiver.kind() == "Document" && key == "cookie" {
            let store = self.host.net_provider().map_err(host_error)?.cookie_store();
            return Ok(Some(Value::string(store.script_cookies(doc.url()))));
        }
        Ok(None)
    }

    fn set(
        &mut self,
        doc: &mut BaseDocument,
        receiver: &Value,
        key: &str,
        value: Value,
    ) -> blitz_script::native::Result<bool> {
        if receiver.kind() == "Document" && key == "cookie" {
            let cookie = value.text();
            let _accepted = self
                .host
                .net_provider()
                .map_err(host_error)?
                .cookie_store()
                .set_script_cookie(doc.url(), &cookie)
                .map_err(|error| blitz_script::native::Error::type_error(error.to_string()))?;
            return Ok(true);
        }
        if self.gpu_set(doc, receiver, key, value.clone())? {
            return Ok(true);
        }
        if self.media_set(doc, receiver, key, value)? {
            return Ok(true);
        }
        Ok(false)
    }

    fn call(
        &mut self,
        doc: &mut BaseDocument,
        name: &str,
        this: Value,
        args: Vec<Value>,
        construct: bool,
    ) -> blitz_script::native::Result<Option<Value>> {
        if name == "Window.requestAnimationFrame" {
            let callback = args.first().cloned().unwrap_or(Value::Undefined);
            if !matches!(callback.kind().as_str(), "Function" | "Native") {
                return Err(blitz_script::native::Error::type_error(
                    "animation callback must be callable",
                ));
            }
            let receiver = Value::object("AnimationCallback");
            receiver.put("callback", callback);
            return self
                .schedule_guest_callback(
                    Value::native("Web.AnimationCallback"),
                    receiver,
                    16.0,
                    false,
                    Vec::new(),
                )
                .map(Some);
        }
        if name == "Browser.sendBeacon" {
            let options = Value::object("Object");
            options.put("method", Value::string("POST"));
            options.put("body", args.get(1).cloned().unwrap_or(Value::Undefined));
            let result = self.media_call(
                doc,
                "Media.fetch",
                Value::Undefined,
                vec![args.first().cloned().unwrap_or(Value::Undefined), options],
                false,
            );
            return Ok(Some(Value::Bool(matches!(result, Ok(Some(_))))));
        }
        if let Some(value) = self.local_storage_call(name, args.clone())? {
            return Ok(Some(value));
        }
        if matches!(
            name,
            "Window.setTimeout" | "Window.setInterval" | "setTimeout" | "setInterval"
        ) {
            let callback = args.first().cloned().unwrap_or(Value::Undefined);
            let delay = args.get(1).map(Value::number_value).unwrap_or(0.0);
            let global = self
                .state
                .get("realm.global")
                .cloned()
                .unwrap_or(Value::Undefined);
            return self
                .schedule_guest_callback(
                    callback,
                    global,
                    delay,
                    name.contains("Interval"),
                    args.into_iter().skip(2).collect(),
                )
                .map(Some);
        }
        if matches!(
            name,
            "Window.clearTimeout"
                | "Window.clearInterval"
                | "clearTimeout"
                | "clearInterval"
                | "Window.cancelAnimationFrame"
        ) {
            return self
                .cancel_guest_timer(args.first().unwrap_or(&Value::Undefined))
                .map(Some);
        }
        if self.host.intrinsic_arity(name).is_some() {
            return self.invoke(name, args).map(Some);
        }
        if let Some(value) = self.gpu_call(doc, name, this.clone(), args.clone(), construct)? {
            return Ok(Some(value));
        }
        if let Some(value) = self.media_call(doc, name, this, args, construct)? {
            return Ok(Some(value));
        }
        let _ = doc;
        Ok(None)
    }

    fn poll(
        &mut self,
        doc: &mut BaseDocument,
        waker: Option<&Waker>,
        run_timers: bool,
    ) -> blitz_script::native::Result<Vec<HostJob>> {
        if self.torn_down {
            return Ok(Vec::new());
        }
        self.host
            .poll_with_timers(waker, run_timers)
            .map_err(host_error)?;
        for frame in self.host.drain_frames() {
            self.apply_native_frame(doc, frame)?;
        }
        let completed = std::mem::take(&mut *self.completed_timers.borrow_mut());
        for id in completed {
            if self
                .timer_callbacks
                .get(&id)
                .is_some_and(|(_, repeat)| !repeat)
                && let Some((callback, _)) = self.timer_callbacks.remove(&id)
            {
                let _ = self.host.release_callback(callback);
            }
        }
        // Host polling applies frames to a configured renderer sink. Keep any
        // frames without a sink rooted on the host for the owning document.
        let _ = self.drain_media_completions()?;
        let mut jobs = std::mem::take(&mut *self.pending.borrow_mut());
        jobs.extend(self.gpu_take_jobs()?);
        Ok(jobs)
    }

    fn has_pending_jobs(&self) -> bool {
        !self.pending.borrow().is_empty()
            || self
                .host
                .next_timer_deadline_ns()
                .is_some_and(|deadline| deadline <= nagoya::now_ns())
            || self.gpu_pending()
            || self.media_pending()
    }

    fn pending_task_count(&self) -> u64 {
        self.pending.borrow().len() as u64
            + self.host.timer_count() as u64
            + self.gpu_task_count()
            + self.media_task_count()
    }

    fn teardown(&mut self) {
        if self.torn_down {
            return;
        }
        self.torn_down = true;
        self.pending.borrow_mut().clear();
        self.completed_timers.borrow_mut().clear();
        self.host.teardown_page_script_host();
        self.timer_callbacks.clear();
        self.host_objects.clear();
        self.local_storage.borrow_mut().clear();
        self.state.clear();
    }
}

fn host_error(error: HostError) -> blitz_script::native::Error {
    blitz_script::native::Error::new(error.name, error.message)
}

pub(super) fn host_callback_value(value: &HostValue) -> Value {
    match value {
        HostValue::Undefined => Value::Undefined,
        HostValue::Null => Value::Null,
        HostValue::Boolean(value) => Value::Bool(*value),
        HostValue::Number(value) => Value::Number(*value),
        HostValue::Utf16(value) => Value::String(Rc::clone(value)),
        HostValue::String(value) => Value::string(value.as_ref()),
        HostValue::Object(value) => {
            let object = Value::object("HostObject");
            if let Some(payload) = object.as_object() {
                payload.borrow_mut().payload = Some(Rc::new(value.clone()) as Rc<dyn Any>);
            }
            object
        }
        HostValue::Buffer(value) => {
            let bytes = match value.as_bytes() {
                Ok(bytes) => bytes.to_vec(),
                Err(_) => return Value::error("TypeError", "host supplied a detached buffer"),
            };
            let object = Value::object("Uint8Array");
            if let Some(payload) = object.as_object() {
                let mut payload = payload.borrow_mut();
                payload.indexed = bytes
                    .iter()
                    .map(|byte| Value::Number(f64::from(*byte)))
                    .collect();
                payload.payload =
                    Some(Rc::new(RefCell::new(HostBuffer::new(bytes))) as Rc<dyn Any>);
            }
            object
        }
        HostValue::Array(values) => Value::array(values.iter().map(host_callback_value).collect()),
        HostValue::Record(values) => {
            let object = Value::object("Object");
            if let Some(payload) = object.as_object() {
                for (key, value) in values {
                    payload
                        .borrow_mut()
                        .props
                        .insert(key.clone(), Property::data(host_callback_value(value)));
                }
            }
            object
        }
    }
}

impl blitz_script::native::NativeStorageBridge for ChuzzBridge {
    fn storage_get(&mut self, key: &str) -> blitz_script::native::Result<Option<Value>> {
        Ok(self.state.get(&format!("storage:{key}")).cloned())
    }
    fn storage_set(&mut self, key: &str, value: Value) -> blitz_script::native::Result<()> {
        let key = format!("storage:{key}");
        if self.state.len() >= MAX_STORAGE_ENTRIES && !self.state.contains_key(&key) {
            return Err(blitz_script::native::Error::new(
                "QuotaExceededError",
                "typed storage entry limit exceeded",
            ));
        }
        self.state.insert(key, value);
        Ok(())
    }
    fn storage_remove(&mut self, key: &str) -> blitz_script::native::Result<()> {
        self.state.remove(&format!("storage:{key}"));
        Ok(())
    }
    fn storage_clear(&mut self) -> blitz_script::native::Result<()> {
        self.state.retain(|key, _| !key.starts_with("storage:"));
        Ok(())
    }
}
