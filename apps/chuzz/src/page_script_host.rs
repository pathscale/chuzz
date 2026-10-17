//! Persistent page-side adapter for the native subsystem installers.
//!
//! Create one host per document, retain guest callback roots in closures passed
//! to `retain_callback`, and invoke registered intrinsics on the realm thread.
//! The renderer drives `poll` with its current waker and attaches a frame sink
//! that validates target identity and generation before applying raster updates.
//! Dropping the host or calling `teardown_page_script_host` retires the realm.
//! The published script backend still needs to bind its guest values and calls
//! to this boundary; this module does not attach itself to a renderer.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Context, Waker};
use std::time::Duration;

use crate::script_host::{
    self as script_host, ChuzzScriptHost, HostCallbackId, HostError, HostGeneration,
    HostInstallContext, HostIntrinsic, HostObject, HostRealm, HostValue,
};
use crate::tab_session::{LocalStorageError, LocalStorageStore};
use crate::tab_stash::Text;

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

struct TimerEntry {
    callback: HostCallbackId,
    repeat: bool,
    interval: Duration,
    deadline_ns: u64,
    wake: Pin<Box<dyn Future<Output = ()>>>,
    wake_ready: bool,
    firing: bool,
}

fn duration_ns(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
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

/// One registered native intrinsic that an engine adapter can expose to a guest.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostIntrinsicBinding {
    pub name: &'static str,
    pub arity: u8,
}

/// A lease for an engine adapter attached to one page host.
///
/// The lease becomes invalid when the page host is torn down or dropped. It
/// does not retain the host or an engine guest.
#[derive(Clone)]
pub struct PageHostAttachment {
    live: Rc<Cell<bool>>,
}

impl PageHostAttachment {
    pub fn is_live(&self) -> bool {
        self.live.get()
    }

    pub fn ensure_live(&self) -> Result<(), HostError> {
        if self.is_live() {
            Ok(())
        } else {
            Err(HostError::new(
                "InvalidStateError",
                "page script host attachment has been retired",
            ))
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
    attachment_live: Rc<Cell<bool>>,
    timers: BTreeMap<u64, TimerEntry>,
    next_timer_id: u64,
    local_storage: Rc<std::cell::RefCell<LocalStorageStore>>,
}

impl ChuzzPageRuntimeHost {
    pub fn new_page_script_host(config: HostInstallContext) -> Result<Self, HostError> {
        let local_storage_origin = config
            .base
            .as_ref()
            .map(url::Url::origin)
            .unwrap_or_else(url::Origin::new_opaque);
        let mut host = Self {
            state: ChuzzRealmState::new(),
            config: Some(config),
            torn_down: false,
            frames: BTreeMap::new(),
            frame_sink: None,
            attachment_live: Rc::new(Cell::new(true)),
            timers: BTreeMap::new(),
            next_timer_id: 1,
            local_storage: Rc::new(std::cell::RefCell::new(LocalStorageStore::new(
                local_storage_origin,
            ))),
        };
        let local_storage = Rc::clone(&host.local_storage);
        host.set_state("chuzz.localStorage", HostObject::new(local_storage))?;
        let install_config = host.config.take().expect("config installed");
        let result = script_host::install_all_chuzz_hosts(&mut host, &install_config);
        host.config = Some(install_config);
        if let Err(error) = result.and_then(|()| install_local_storage_intrinsics(&mut host)) {
            host.teardown();
            return Err(error);
        }
        Ok(host)
    }

    /// Acquire a lease that the engine adapter can check before dispatching
    /// calls associated with this page.
    pub fn attachment(&self) -> Result<PageHostAttachment, HostError> {
        self.ensure_live()?;
        Ok(PageHostAttachment {
            live: Rc::clone(&self.attachment_live),
        })
    }

    /// The exact provider installed for this page. Script-visible navigation
    /// and cookie properties must use this provider rather than the renderer's
    /// trait-object copy on `BaseDocument`.
    pub fn net_provider(&self) -> Result<&Arc<crate::document_loader::NetProvider>, HostError> {
        self.config
            .as_ref()
            .map(|config| &config.net)
            .ok_or_else(|| {
                HostError::new("InvalidStateError", "page network provider is unavailable")
            })
    }

    pub(crate) fn script_network(&self) -> Result<crate::net_bridge::NetworkHost, HostError> {
        self.ensure_live()?;
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| HostError::new("InvalidStateError", "page host has no configuration"))?;
        Ok(crate::net_bridge::NetworkHost::new(
            Arc::clone(&config.net),
            config.base.clone(),
            config.deadline,
            config.signal.clone(),
            Arc::clone(&config.lifetime),
        ))
    }

    /// The exact base URL supplied to the native hosts during installation.
    pub fn base_url(&self) -> Result<Option<&url::Url>, HostError> {
        self.config
            .as_ref()
            .map(|config| config.base.as_ref())
            .ok_or_else(|| {
                HostError::new(
                    "InvalidStateError",
                    "page host configuration is unavailable",
                )
            })
    }

    /// Return the registered intrinsic surface in deterministic name order.
    /// The returned metadata can be used to install callable guest bindings;
    /// dispatch remains fallible through `invoke_intrinsic`.
    pub fn intrinsic_bindings(&self) -> Vec<HostIntrinsicBinding> {
        self.state
            .intrinsics
            .iter()
            .map(|(name, (arity, _))| HostIntrinsicBinding {
                name,
                arity: *arity,
            })
            .collect()
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

    /// Schedule a retained callback. The caller keeps ownership of its callback
    /// registration and may share it across timers until explicitly released.
    pub fn schedule_timer(
        &mut self,
        callback: HostCallbackId,
        delay: Duration,
        repeat: bool,
    ) -> Result<u64, HostError> {
        self.ensure_live()?;
        if !callback.is_scoped_to(&self.state.realm, &self.state.generation)
            || !self
                .state
                .callbacks
                .iter()
                .any(|entry| entry.id.same_callback(&callback))
        {
            return Err(HostError::new(
                "InvalidStateError",
                "timer callback is unavailable in this realm",
            ));
        }

        if self.timers.len() >= 4096 {
            return Err(HostError::new(
                "QuotaExceededError",
                "page timer limit reached",
            ));
        }
        let id = self.next_timer_id;
        if id == 0 || self.timers.contains_key(&id) {
            return Err(HostError::new(
                "QuotaExceededError",
                "timer identifier space exhausted",
            ));
        }
        self.next_timer_id = id.checked_add(1).unwrap_or(0);

        let interval = if repeat {
            delay.max(Duration::from_millis(1))
        } else {
            delay
        };
        let deadline_ns = nagoya::now_ns().saturating_add(duration_ns(interval));
        self.timers.insert(
            id,
            TimerEntry {
                callback,
                repeat,
                interval,
                deadline_ns,
                wake: Box::pin(nagoya::sleep_until(deadline_ns)),
                wake_ready: false,
                firing: false,
            },
        );
        if let Some(config) = &self.config {
            config.wake.wake();
        }
        Ok(id)
    }

    pub fn cancel_timer(&mut self, id: u64) -> Result<bool, HostError> {
        self.ensure_live()?;
        Ok(self.timers.remove(&id).is_some())
    }

    /// Hosts that poll without a task waker can sleep until this deadline.
    pub fn next_timer_deadline_ns(&self) -> Option<u64> {
        self.timers.values().map(|timer| timer.deadline_ns).min()
    }

    pub fn timer_count(&self) -> usize {
        self.timers.len()
    }

    pub fn seed_session(&mut self, entries: &[(String, String)]) -> Result<(), HostError> {
        self.ensure_live()?;
        script_host::session::seed(self, entries)
    }

    pub fn capture_session(
        &self,
        budget: &Arc<crate::tab_stash::SleepingBudget>,
    ) -> Result<Option<Arc<crate::tab_stash::SessionBucket>>, HostError> {
        self.ensure_live()?;
        script_host::session::snapshot(self, budget)
    }

    /// Read the current origin's browser-owned localStorage data. The native
    /// document adapter should expose these calls directly to Ekojit; guest
    /// keys are never used as properties of a guest object.
    pub fn local_storage_length(&mut self) -> Result<usize, HostError> {
        self.ensure_live()?;
        self.local_storage
            .borrow_mut()
            .length()
            .map_err(local_storage_error)
    }

    pub fn local_storage_key(&mut self, index: usize) -> Result<Option<Rc<[u16]>>, HostError> {
        self.ensure_live()?;
        Ok(self
            .local_storage
            .borrow_mut()
            .key(index)
            .map_err(local_storage_error)?
            .map(|value| Rc::from(value.to_vec())))
    }

    pub fn local_storage_get(&mut self, key: &[u16]) -> Result<Option<Rc<[u16]>>, HostError> {
        self.ensure_live()?;
        Ok(self
            .local_storage
            .borrow_mut()
            .get(key)
            .map_err(local_storage_error)?
            .map(|value| Rc::from(value.to_vec())))
    }

    pub fn local_storage_set(&mut self, key: &[u16], value: &[u16]) -> Result<(), HostError> {
        self.ensure_live()?;
        let key = Text::capture(key.iter().copied(), 2048).ok_or_else(local_quota_error)?;
        let value = Text::capture(value.iter().copied(), 8192).ok_or_else(local_quota_error)?;
        self.local_storage
            .borrow_mut()
            .set(key, value)
            .map_err(local_storage_error)
    }

    pub fn local_storage_remove(&mut self, key: &[u16]) -> Result<(), HostError> {
        self.ensure_live()?;
        self.local_storage
            .borrow_mut()
            .remove(key)
            .map_err(local_storage_error)
    }

    pub fn local_storage_clear(&mut self) -> Result<(), HostError> {
        self.ensure_live()?;
        self.local_storage
            .borrow_mut()
            .clear()
            .map_err(local_storage_error)
    }

    pub fn seed_local_storage(&mut self, entries: &[(String, String)]) -> Result<(), HostError> {
        self.ensure_live()?;
        self.local_storage
            .borrow_mut()
            .seed(entries)
            .map_err(local_storage_error)
    }

    pub fn poll(&mut self, waker: Option<&Waker>) -> Result<bool, HostError> {
        self.poll_with_timers(waker, true)
    }

    pub fn poll_with_timers(
        &mut self,
        waker: Option<&Waker>,
        run_timers: bool,
    ) -> Result<bool, HostError> {
        self.ensure_live()?;

        let mut first_error = None;
        let mut changed = false;
        match script_host::poll_all_chuzz_hosts(self, waker) {
            Ok(did_change) => changed |= did_change,
            Err(error) => first_error = Some(error),
        }

        let now = nagoya::now_ns();
        if let Some(waker) = waker.filter(|_| run_timers) {
            let mut context = Context::from_waker(waker);
            for timer in self.timers.values_mut() {
                if timer.firing || timer.wake_ready {
                    continue;
                }
                if timer.wake.as_mut().poll(&mut context).is_ready() {
                    timer.wake_ready = true;
                }
            }
        }

        let due: Vec<u64> = self
            .timers
            .iter()
            .filter_map(|(id, timer)| {
                (run_timers && !timer.firing && (timer.wake_ready || timer.deadline_ns <= now))
                    .then_some(*id)
            })
            .collect();

        for id in due {
            let Some(timer) = self.timers.get_mut(&id) else {
                continue;
            };
            timer.firing = true;
            let callback = timer.callback.clone();

            match self.invoke_callback(callback, &[]) {
                Ok(_) => changed = true,
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }

            if let Some(timer) = self.timers.get_mut(&id) {
                if timer.repeat {
                    timer.firing = false;
                    timer.deadline_ns =
                        nagoya::now_ns().saturating_add(duration_ns(timer.interval));
                    timer.wake = Box::pin(nagoya::sleep_until(timer.deadline_ns));
                    timer.wake_ready = false;
                    if let Some(waker) = waker {
                        let mut context = Context::from_waker(waker);
                        timer.wake_ready = timer.wake.as_mut().poll(&mut context).is_ready();
                    }
                } else {
                    self.timers.remove(&id);
                }
            }
        }

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
                        sink_error.get_or_insert(error);
                    }
                }
            }
        }

        if let Some(error) = first_error.or(sink_error) {
            Err(error)
        } else {
            Ok(changed || had_frames)
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

    pub(crate) fn callback_is_live(&self, callback: &HostCallbackId) -> bool {
        self.state
            .callbacks
            .iter()
            .any(|entry| entry.id.same_callback(callback))
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
        self.attachment_live.set(false);
        if let Some(config) = self.config.take() {
            blitz_traits::net::AbortController {
                signal: config.signal.clone(),
            }
            .abort();
            config.wake.retire();
            config.lifetime.retire(Vec::new());
            drop(config);
        }
        self.timers.clear();
        self.state.callbacks.clear();
        self.state.intrinsics.clear();
        self.state.states.clear();
        self.frames.clear();
        self.frame_sink.take();
    }
}

fn local_quota_error() -> HostError {
    HostError::new("QuotaExceededError", "localStorage quota exceeded")
}

fn local_storage_error(error: LocalStorageError) -> HostError {
    match error {
        LocalStorageError::QuotaExceeded => local_quota_error(),
        LocalStorageError::Persistence => {
            HostError::new("UnknownError", "localStorage persistence failed")
        }
    }
}

fn install_local_storage_intrinsics(host: &mut ChuzzPageRuntimeHost) -> Result<(), HostError> {
    host.register_intrinsic("__chuzzLocal", 3, local_dispatch)?;
    host.register_intrinsic("__chuzzLocalLength", 0, local_length)?;
    host.register_intrinsic("__chuzzLocalKey", 1, local_key)?;
    host.register_intrinsic("__chuzzLocalGetItem", 1, local_get_item)?;
    host.register_intrinsic("__chuzzLocalSetItem", 2, local_set_item)?;
    host.register_intrinsic("__chuzzLocalRemoveItem", 1, local_remove_item)?;
    host.register_intrinsic("__chuzzLocalClear", 0, local_clear)?;
    Ok(())
}

fn local_storage_owner(
    host: &dyn ChuzzScriptHost,
) -> Result<Rc<std::cell::RefCell<LocalStorageStore>>, HostError> {
    host.state("chuzz.localStorage")
        .and_then(|state| {
            state
                .downcast_ref::<Rc<std::cell::RefCell<LocalStorageStore>>>()
                .cloned()
        })
        .ok_or_else(|| HostError::new("TypeError", "localStorage owner unavailable"))
}

fn local_dispatch(
    host: &mut dyn ChuzzScriptHost,
    args: &[HostValue],
) -> Result<HostValue, HostError> {
    let operation = match args.first() {
        Some(HostValue::Number(value))
            if value.is_finite() && value.fract() == 0.0 && (0.0..=5.0).contains(value) =>
        {
            *value as u8
        }
        _ => {
            return Err(HostError::new(
                "TypeError",
                "invalid localStorage operation",
            ));
        }
    };
    local_operation(host, operation, &args[1..])
}

fn local_operation(
    host: &dyn ChuzzScriptHost,
    operation: u8,
    args: &[HostValue],
) -> Result<HostValue, HostError> {
    let owner = local_storage_owner(host)?;
    let mut store = owner.borrow_mut();
    match operation {
        0 => Ok(HostValue::Number(
            store.length().map_err(local_storage_error)? as f64,
        )),
        1 => {
            let index = match args.first() {
                Some(HostValue::Number(value)) if value.is_finite() => {
                    value.trunc().rem_euclid(4_294_967_296.0) as usize
                }
                _ => {
                    return Err(HostError::new(
                        "TypeError",
                        "localStorage index must be a number",
                    ));
                }
            };
            Ok(store
                .key(index)
                .map_err(local_storage_error)?
                .map_or(HostValue::Null, |value| {
                    HostValue::Utf16(Rc::from(value.to_vec()))
                }))
        }
        2 => {
            let key = local_text_arg(args, 0, 2048)?;
            Ok(store
                .get(key.units())
                .map_err(local_storage_error)?
                .map_or(HostValue::Null, |value| {
                    HostValue::Utf16(Rc::from(value.to_vec()))
                }))
        }
        3 => {
            let key = local_text_arg(args, 0, 2048)?;
            let value = local_text_arg(args, 1, 8192)?;
            store.set(key, value).map_err(local_storage_error)?;
            Ok(HostValue::Undefined)
        }
        4 => {
            let key = local_text_arg(args, 0, 2048)?;
            store.remove(key.units()).map_err(local_storage_error)?;
            Ok(HostValue::Undefined)
        }
        5 => {
            store.clear().map_err(local_storage_error)?;
            Ok(HostValue::Undefined)
        }
        _ => Err(HostError::new(
            "TypeError",
            "unknown localStorage operation",
        )),
    }
}

fn local_length(
    host: &mut dyn ChuzzScriptHost,
    args: &[HostValue],
) -> Result<HostValue, HostError> {
    local_operation(host, 0, args)
}

fn local_key(host: &mut dyn ChuzzScriptHost, args: &[HostValue]) -> Result<HostValue, HostError> {
    local_operation(host, 1, args)
}

fn local_get_item(
    host: &mut dyn ChuzzScriptHost,
    args: &[HostValue],
) -> Result<HostValue, HostError> {
    local_operation(host, 2, args)
}

fn local_set_item(
    host: &mut dyn ChuzzScriptHost,
    args: &[HostValue],
) -> Result<HostValue, HostError> {
    local_operation(host, 3, args)
}

fn local_remove_item(
    host: &mut dyn ChuzzScriptHost,
    args: &[HostValue],
) -> Result<HostValue, HostError> {
    local_operation(host, 4, args)
}

fn local_clear(host: &mut dyn ChuzzScriptHost, args: &[HostValue]) -> Result<HostValue, HostError> {
    local_operation(host, 5, args)
}

fn local_text_arg(args: &[HostValue], index: usize, limit: usize) -> Result<Text, HostError> {
    let value = args
        .get(index)
        .ok_or_else(|| HostError::new("TypeError", "localStorage key or value must be a string"))?;
    let text = match value {
        HostValue::Utf16(value) => Text::capture(value.iter().copied(), limit),
        HostValue::String(value) => Text::capture(value.encode_utf16(), limit),
        _ => {
            return Err(HostError::new(
                "TypeError",
                "localStorage key or value must be a string",
            ));
        }
    };
    text.ok_or_else(local_quota_error)
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
        self.timers
            .retain(|_, timer| !timer.callback.same_callback(&id));
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
