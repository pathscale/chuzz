//! Persistent page-side adapter for the native subsystem installers.
//!
//! Create one host per document, retain guest callback roots in closures passed
//! to `retain_callback`, and invoke registered intrinsics on the realm thread.
//! The renderer drives `poll` with its current waker and attaches a frame sink
//! that validates target identity and generation before applying raster updates.
//! Dropping the host or calling `teardown_page_script_host` retires the realm.
//! The published script backend still needs to bind its guest values and calls
//! to this boundary; this module does not attach itself to a renderer.

use std::collections::BTreeMap;
use std::rc::Rc;
use std::task::Waker;

use crate::script_host::{
    self as script_host, ChuzzScriptHost, HostCallbackId, HostError, HostGeneration,
    HostInstallContext, HostIntrinsic, HostObject, HostRealm, HostValue,
};

/// An owned, engine-neutral script value. Conversions preserve UTF-16 code
/// units, object identity, detached buffers, and nested structure.
pub enum NativeValue {
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    Utf16(Rc<[u16]>),
    String(Rc<str>),
    Buffer(crate::script_host::HostBuffer),
    Object(HostObject),
    Array(Vec<NativeValue>),
    Record(BTreeMap<String, NativeValue>),
}

impl From<HostValue> for NativeValue {
    fn from(value: HostValue) -> Self {
        match value {
            HostValue::Undefined => Self::Undefined,
            HostValue::Null => Self::Null,
            HostValue::Boolean(value) => Self::Boolean(value),
            HostValue::Number(value) => Self::Number(value),
            HostValue::Utf16(value) => Self::Utf16(value),
            HostValue::String(value) => Self::String(value),
            HostValue::Buffer(value) => Self::Buffer(value),
            HostValue::Object(value) => Self::Object(value),
            HostValue::Array(values) => Self::Array(values.into_iter().map(Into::into).collect()),
            HostValue::Record(values) => Self::Record(
                values
                    .into_iter()
                    .map(|(key, value)| (key, value.into()))
                    .collect(),
            ),
        }
    }
}

impl From<NativeValue> for HostValue {
    fn from(value: NativeValue) -> Self {
        match value {
            NativeValue::Undefined => Self::Undefined,
            NativeValue::Null => Self::Null,
            NativeValue::Boolean(value) => Self::Boolean(value),
            NativeValue::Number(value) => Self::Number(value),
            NativeValue::Utf16(value) => Self::Utf16(value),
            NativeValue::String(value) => Self::String(value),
            NativeValue::Buffer(value) => Self::Buffer(value),
            NativeValue::Object(value) => Self::Object(value),
            NativeValue::Array(values) => Self::Array(values.into_iter().map(Into::into).collect()),
            NativeValue::Record(values) => Self::Record(
                values
                    .into_iter()
                    .map(|(key, value)| (key, value.into()))
                    .collect(),
            ),
        }
    }
}

type RetainedCallback =
    Rc<dyn Fn(&mut dyn ChuzzScriptHost, &[HostValue]) -> Result<HostValue, HostError> + 'static>;
type FrameSink = Box<dyn FnMut(crate::script_host::HostFrame) -> Result<(), HostError>>;

struct CallbackEntry {
    id: HostCallbackId,
    callback: RetainedCallback,
}

/// Persistent native state for one page realm.
pub struct ChuzzRealmState {
    realm: HostRealm,
    generation: HostGeneration,
    states: BTreeMap<&'static str, HostObject>,
    intrinsics: BTreeMap<&'static str, (u8, HostIntrinsic)>,
    callbacks: Vec<CallbackEntry>,
}

impl ChuzzRealmState {
    fn new() -> Self {
        let realm = HostRealm::new();
        let generation = HostGeneration::new(&realm);
        Self {
            realm,
            generation,
            states: BTreeMap::new(),
            intrinsics: BTreeMap::new(),
            callbacks: Vec::new(),
        }
    }
}

/// Concrete lifecycle owner for a page's native script hosts.
pub struct ChuzzPageRuntimeHost {
    state: ChuzzRealmState,
    config: Option<HostInstallContext>,
    torn_down: bool,
    frames: BTreeMap<(crate::script_host::HostFrameSource, u64), crate::script_host::HostFrame>,
    frame_sink: Option<FrameSink>,
}

impl ChuzzPageRuntimeHost {
    pub fn new_page_script_host(config: HostInstallContext) -> Result<Self, HostError> {
        let mut host = Self {
            state: ChuzzRealmState::new(),
            config: Some(config),
            torn_down: false,
            frames: BTreeMap::new(),
            frame_sink: None,
        };
        let install_config = host.config.take().expect("config installed");
        let result = script_host::install_all_chuzz_hosts(&mut host, &install_config);
        host.config = Some(install_config);
        if let Err(error) = result {
            host.teardown();
            return Err(error);
        }
        Ok(host)
    }

    pub fn invoke_intrinsic(
        &mut self,
        name: &str,
        args: &[HostValue],
    ) -> Result<HostValue, HostError> {
        self.ensure_live()?;
        let (_, callback) = self
            .state
            .intrinsics
            .get(name)
            .copied()
            .ok_or_else(|| HostError::new("TypeError", "unknown native intrinsic"))?;
        callback(self, args)
    }

    pub fn retain_callback(
        &mut self,
        callback: impl Fn(&mut dyn ChuzzScriptHost, &[HostValue]) -> Result<HostValue, HostError>
        + 'static,
    ) -> Result<HostCallbackId, HostError> {
        self.ensure_live()?;
        if self.state.callbacks.len() >= 4096 {
            return Err(HostError::new(
                "QuotaExceededError",
                "retained callback limit reached",
            ));
        }
        let token = Rc::new(());
        let id = HostCallbackId::new(&self.state.realm, &self.state.generation, token)
            .map_err(|_| HostError::new("InvalidStateError", "callback realm is invalid"))?;
        self.state.callbacks.push(CallbackEntry {
            id: id.clone(),
            callback: Rc::new(callback),
        });
        Ok(id)
    }

    pub fn invoke_callback(
        &mut self,
        id: HostCallbackId,
        args: &[HostValue],
    ) -> Result<HostValue, HostError> {
        self.ensure_live()?;
        if !id.is_scoped_to(&self.state.realm, &self.state.generation) {
            return Err(HostError::new(
                "InvalidStateError",
                "callback belongs to another realm",
            ));
        }
        let callback = self
            .state
            .callbacks
            .iter()
            .find(|entry| entry.id.same_callback(&id))
            .map(|entry| Rc::clone(&entry.callback))
            .ok_or_else(|| HostError::new("InvalidStateError", "callback has been released"))?;
        callback(self, args)
    }

    pub fn poll(&mut self, waker: Option<&Waker>) -> Result<bool, HostError> {
        self.ensure_live()?;
        let poll_result = script_host::poll_all_chuzz_hosts(self, waker);
        let had_frames = !self.frames.is_empty();
        let mut sink_error = None;
        if let Some(sink) = self.frame_sink.as_mut() {
            let keys: Vec<_> = self.frames.keys().copied().collect();
            for key in keys {
                let Some(frame) = self.frames.get(&key) else {
                    continue;
                };
                match sink(frame.clone()) {
                    Ok(()) => {
                        self.frames.remove(&key);
                    }
                    Err(error) => {
                        sink_error = Some(error);
                        break;
                    }
                }
            }
        }
        match poll_result {
            Err(error) => Err(error),
            Ok(_) if sink_error.is_some() => Err(sink_error.expect("sink error recorded")),
            Ok(changed) => Ok(changed || had_frames),
        }
    }

    pub fn set_frame_sink(
        &mut self,
        sink: impl FnMut(crate::script_host::HostFrame) -> Result<(), HostError> + 'static,
    ) -> Result<(), HostError> {
        self.ensure_live()?;
        self.frame_sink = Some(Box::new(sink));
        Ok(())
    }

    pub fn drain_frames(&mut self) -> Vec<crate::script_host::HostFrame> {
        std::mem::take(&mut self.frames).into_values().collect()
    }

    pub fn is_torn_down(&self) -> bool {
        self.torn_down
    }
    pub fn callback_count(&self) -> usize {
        self.state.callbacks.len()
    }
    pub fn intrinsic_arity(&self, name: &str) -> Option<u8> {
        self.state.intrinsics.get(name).map(|entry| entry.0)
    }

    pub fn teardown_page_script_host(&mut self) {
        self.teardown();
    }

    fn ensure_live(&self) -> Result<(), HostError> {
        if self.torn_down {
            Err(HostError::new(
                "InvalidStateError",
                "page script host has been torn down",
            ))
        } else {
            Ok(())
        }
    }

    fn teardown(&mut self) {
        if self.torn_down {
            return;
        }
        self.torn_down = true;
        if let Some(config) = self.config.take() {
            blitz_traits::net::AbortController {
                signal: config.signal.clone(),
            }
            .abort();
            config.wake.retire();
            config.lifetime.retire(Vec::new());
            drop(config);
        }
        self.state.callbacks.clear();
        self.state.intrinsics.clear();
        self.state.states.clear();
        self.frames.clear();
        self.frame_sink.take();
    }
}

impl Drop for ChuzzPageRuntimeHost {
    fn drop(&mut self) {
        self.teardown();
    }
}

impl ChuzzScriptHost for ChuzzPageRuntimeHost {
    fn set_state(&mut self, key: &'static str, value: HostObject) -> Result<(), HostError> {
        self.ensure_live()?;
        self.state.states.insert(key, value);
        Ok(())
    }
    fn state(&self, key: &'static str) -> Option<HostObject> {
        self.state.states.get(key).cloned()
    }
    fn register_intrinsic(
        &mut self,
        name: &'static str,
        arity: u8,
        callback: HostIntrinsic,
    ) -> Result<(), HostError> {
        self.ensure_live()?;
        self.state.intrinsics.insert(name, (arity, callback));
        Ok(())
    }
    fn dispatch_callback(
        &mut self,
        id: HostCallbackId,
        args: &[HostValue],
    ) -> Result<HostValue, HostError> {
        self.invoke_callback(id, args)
    }
    fn release_callback(&mut self, id: HostCallbackId) -> Result<(), HostError> {
        self.ensure_live()?;
        if !id.is_scoped_to(&self.state.realm, &self.state.generation) {
            return Err(HostError::new(
                "InvalidStateError",
                "callback belongs to another realm",
            ));
        }
        let before = self.state.callbacks.len();
        self.state
            .callbacks
            .retain(|entry| !entry.id.same_callback(&id));
        if before == self.state.callbacks.len() {
            return Err(HostError::new(
                "InvalidStateError",
                "callback has been released",
            ));
        }
        Ok(())
    }
    fn deliver_frame(&mut self, frame: crate::script_host::HostFrame) -> Result<(), HostError> {
        self.ensure_live()?;
        let key = (frame.source, frame.target);
        // A late completion from an older target generation must not displace
        // the current frame while it waits for the renderer sink.
        if self
            .frames
            .get(&key)
            .is_some_and(|pending| pending.generation > frame.generation)
        {
            return Ok(());
        }
        if !self.frames.contains_key(&key) && self.frames.len() >= 4096 {
            return Err(HostError::new(
                "QuotaExceededError",
                "pending frame limit reached",
            ));
        }
        self.frames.insert(key, frame);
        Ok(())
    }
}

pub fn new_page_script_host(config: HostInstallContext) -> Result<ChuzzPageRuntimeHost, HostError> {
    ChuzzPageRuntimeHost::new_page_script_host(config)
}

pub fn teardown_page_script_host(host: &mut ChuzzPageRuntimeHost) {
    host.teardown_page_script_host();
}
