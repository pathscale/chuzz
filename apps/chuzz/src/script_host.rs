use std::any::Any;
use std::collections::BTreeMap;
use std::rc::Rc;

pub use crate::audio::native_host as audio;
pub use crate::media::native_host as media;
pub use crate::net_bridge::native_host as network;
pub use crate::tab_session::native_host as session;
pub use crate::video::native_host as video;
pub use crate::webapi::native_host as webapi;
pub use crate::webgpu::native_host as webgpu;
pub use crate::webrtc::native_host as webrtc;
pub use crate::ws_bridge::native_host as websocket;

/// Identity for one live script realm.
pub struct HostRealm(Rc<()>);

impl HostRealm {
    pub fn new() -> Self {
        Self(Rc::new(()))
    }
}

impl Default for HostRealm {
    fn default() -> Self {
        Self::new()
    }
}

/// A unique callback generation within a realm.
pub struct HostGeneration {
    realm: Rc<()>,
    identity: Rc<()>,
}

impl HostGeneration {
    pub fn new(realm: &HostRealm) -> Self {
        Self {
            realm: Rc::clone(&realm.0),
            identity: Rc::new(()),
        }
    }
}

/// Error returned when a callback root is paired with a generation from another realm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostCallbackScopeError;

impl std::fmt::Display for HostCallbackScopeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("callback generation belongs to another realm")
    }
}

impl std::error::Error for HostCallbackScopeError {}

/// An opaque retained callback root scoped to a realm and generation.
///
/// Cloning an ID retains the same root. The root is released when its final ID
/// is dropped. IDs contain no engine pointer or numeric handle.
#[derive(Clone)]
pub struct HostCallbackId {
    realm: Rc<()>,
    generation: Rc<()>,
    root: Rc<dyn Any>,
}

impl HostCallbackId {
    pub fn new<T: Any>(
        realm: &HostRealm,
        generation: &HostGeneration,
        root: Rc<T>,
    ) -> Result<Self, HostCallbackScopeError> {
        if !Rc::ptr_eq(&realm.0, &generation.realm) {
            return Err(HostCallbackScopeError);
        }
        let root: Rc<dyn Any> = root;
        Ok(Self {
            realm: Rc::clone(&realm.0),
            generation: Rc::clone(&generation.identity),
            root,
        })
    }

    pub fn is_scoped_to(&self, realm: &HostRealm, generation: &HostGeneration) -> bool {
        Rc::ptr_eq(&self.realm, &realm.0)
            && Rc::ptr_eq(&self.realm, &generation.realm)
            && Rc::ptr_eq(&self.generation, &generation.identity)
    }

    pub fn same_callback(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.realm, &other.realm)
            && Rc::ptr_eq(&self.generation, &other.generation)
            && Rc::ptr_eq(&self.root, &other.root)
    }
}

impl PartialEq for HostCallbackId {
    fn eq(&self, other: &Self) -> bool {
        self.same_callback(other)
    }
}

impl Eq for HostCallbackId {}

/// A rooted opaque object whose identity is preserved by cloning.
#[derive(Clone)]
pub struct HostObject {
    root: Rc<dyn Any>,
}

impl HostObject {
    pub fn new<T: Any>(value: T) -> Self {
        Self::from_root(Rc::new(value))
    }

    pub fn from_root<T: Any>(root: Rc<T>) -> Self {
        let root: Rc<dyn Any> = root;
        Self { root }
    }

    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        self.root.as_ref().downcast_ref::<T>()
    }

    pub fn same_identity(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.root, &other.root)
    }
}

/// An owned buffer that can be detached once and transferred after validation.
pub struct HostBuffer {
    bytes: Option<Vec<u8>>,
}

/// A detached buffer kept alive until a destination accepts or drops it.
pub struct HostBufferTransfer {
    bytes: Vec<u8>,
}

/// Buffer access failed because the buffer has already been detached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostBufferDetached;

impl std::fmt::Display for HostBufferDetached {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("buffer is detached")
    }
}

impl std::error::Error for HostBufferDetached {}

/// A detach attempt failed before ownership was transferred.
pub enum HostBufferDetachError<E> {
    Detached,
    Rejected(E),
}

/// A destination rejected a transfer. The transfer remains owned by the error.
pub struct HostBufferAttachError<E> {
    pub transfer: HostBufferTransfer,
    pub reason: E,
}

impl HostBuffer {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self { bytes: Some(bytes) }
    }

    pub fn is_detached(&self) -> bool {
        self.bytes.is_none()
    }

    pub fn as_bytes(&self) -> Result<&[u8], HostBufferDetached> {
        self.bytes.as_deref().ok_or(HostBufferDetached)
    }

    /// Validate a transfer before detaching this buffer.
    ///
    /// A rejected transfer leaves the buffer attached and unchanged.
    pub fn detach_with<E>(
        &mut self,
        validate: impl FnOnce(usize) -> Result<(), E>,
    ) -> Result<HostBufferTransfer, HostBufferDetachError<E>> {
        let length = self
            .bytes
            .as_ref()
            .ok_or(HostBufferDetachError::Detached)?
            .len();
        validate(length).map_err(HostBufferDetachError::Rejected)?;
        match self.bytes.take() {
            Some(bytes) => Ok(HostBufferTransfer { bytes }),
            None => Err(HostBufferDetachError::Detached),
        }
    }
}

impl HostBufferTransfer {
    pub fn byte_len(&self) -> usize {
        self.bytes.len()
    }

    /// Validate the destination before attaching the transferred bytes.
    ///
    /// On rejection, the returned error retains the transfer and its bytes.
    pub fn attach_with<E>(
        self,
        validate: impl FnOnce(usize) -> Result<(), E>,
    ) -> Result<HostBuffer, HostBufferAttachError<E>> {
        if let Err(reason) = validate(self.bytes.len()) {
            return Err(HostBufferAttachError {
                transfer: self,
                reason,
            });
        }
        Ok(HostBuffer {
            bytes: Some(self.bytes),
        })
    }
}

/// A value crossing the native script host boundary.
///
/// UTF-16 code units are kept separately from Rust strings so lone surrogates
/// can be represented without loss. Objects retain identity through `HostObject`.
pub enum HostValue {
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    Utf16(Rc<[u16]>),
    String(Rc<str>),
    Buffer(HostBuffer),
    Object(HostObject),
    Array(Vec<HostValue>),
    Record(BTreeMap<String, HostValue>),
}

/// A native script exception with its name and message preserved.
#[derive(Debug)]
pub struct HostError {
    pub name: String,
    pub message: String,
}

impl std::fmt::Display for HostError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.name, self.message)
    }
}

impl std::error::Error for HostError {}

impl HostError {
    pub fn new(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            message: message.into(),
        }
    }
}

/// Native callback retained by the host and invoked without evaluating source text.
pub type HostIntrinsic = fn(&mut dyn ChuzzScriptHost, &[HostValue]) -> Result<HostValue, HostError>;

/// Shared, engine-neutral contract for native callbacks.
pub trait ChuzzScriptHost {
    /// Store native resources in this realm. Replacing state drops its old owner.
    fn set_state(&mut self, key: &'static str, value: HostObject) -> Result<(), HostError>;

    /// Retain a native resource while an intrinsic uses it.
    fn state(&self, key: &'static str) -> Option<HostObject>;

    fn register_intrinsic(
        &mut self,
        name: &'static str,
        arity: u8,
        callback: HostIntrinsic,
    ) -> Result<(), HostError>;

    fn dispatch_callback(
        &mut self,
        id: HostCallbackId,
        args: &[HostValue],
    ) -> Result<HostValue, HostError>;

    /// Drop this retention of a callback. Other cloned IDs retain their root.
    fn release_callback(&mut self, id: HostCallbackId) -> Result<(), HostError>;
}

/// Native resources shared by the subsystem installers for one page realm.
///
/// The owner aborts `signal` and retires `wake` when the realm is torn down.
pub struct HostInstallContext {
    pub net: std::sync::Arc<crate::document_loader::NetProvider>,
    pub base: Option<url::Url>,
    pub deadline: std::time::Duration,
    pub wake: crate::net_bridge::PageWake,
    pub lifetime: std::sync::Arc<blitz_dom::release::Teardown>,
    pub signal: blitz_traits::net::AbortSignal,
    pub session_backing: Option<std::sync::Arc<crate::tab_stash::SessionBucket>>,
}

/// Install the native subsystem surface in dependency order for one realm.
///
/// On failure, the caller must discard the partially installed realm. Native
/// state is owned by the host and released when that realm is discarded.
pub fn install_all_chuzz_hosts(
    host: &mut dyn ChuzzScriptHost,
    config: &HostInstallContext,
) -> Result<(), HostError> {
    session::install(host, config)?;
    network::install(host, config)?;
    websocket::install(host, config)?;
    webapi::install(host, config)?;
    media::install(host, config)?;
    audio::install(host, config)?;
    video::install(host, config)?;
    webrtc::install(host, config)?;
    webgpu::install(host, config)
}

/// Poll on the realm thread with its current waker. The renderer schedules
/// animation turns for active video/capture sources and applies raster updates.
pub fn poll_all_chuzz_hosts(
    host: &mut dyn ChuzzScriptHost,
    waker: Option<&std::task::Waker>,
) -> Result<bool, HostError> {
    let mut changed = network::poll(host, waker)?;
    changed |= websocket::poll(host, waker)?;
    changed |= webapi::poll(host, waker)?;
    changed |= audio::poll(host, waker)?;
    changed |= media::poll(host, waker)?;
    changed |= video::poll(host, waker)?;
    changed |= webrtc::poll(host, waker)?;
    changed |= webgpu::poll(host, waker)?;
    Ok(changed)
}

/// Convert structured native metadata without serializing or evaluating source.
/// Binary payloads must use `HostValue::Buffer` separately.
pub fn from_json(value: serde_json::Value) -> HostValue {
    match value {
        serde_json::Value::Null => HostValue::Null,
        serde_json::Value::Bool(value) => HostValue::Boolean(value),
        serde_json::Value::Number(value) => HostValue::Number(value.as_f64().unwrap_or(f64::NAN)),
        serde_json::Value::String(value) => HostValue::String(value.into()),
        serde_json::Value::Array(values) => {
            HostValue::Array(values.into_iter().map(from_json).collect())
        }
        serde_json::Value::Object(values) => HostValue::Record(
            values
                .into_iter()
                .map(|(key, value)| (key, from_json(value)))
                .collect(),
        ),
    }
}

pub fn to_json(value: &HostValue) -> Result<serde_json::Value, HostError> {
    match value {
        HostValue::Null => Ok(serde_json::Value::Null),
        HostValue::Boolean(value) => Ok((*value).into()),
        HostValue::Number(value) => {
            // Host numbers use the script numeric representation. Preserve
            // integral metadata as integers for native typed command decoders.
            if value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_991.0 {
                return Ok(serde_json::Value::Number(serde_json::Number::from(
                    *value as i64,
                )));
            }
            serde_json::Number::from_f64(*value)
                .map(serde_json::Value::Number)
                .ok_or_else(|| HostError::new("TypeError", "metadata number must be finite"))
        }
        HostValue::String(value) => Ok(value.to_string().into()),
        HostValue::Utf16(value) => String::from_utf16(value)
            .map(Into::into)
            .map_err(|_| HostError::new("TypeError", "metadata contains an unpaired surrogate")),
        HostValue::Array(values) => values
            .iter()
            .map(to_json)
            .collect::<Result<Vec<_>, _>>()
            .map(serde_json::Value::Array),
        HostValue::Record(values) => values
            .iter()
            .map(|(key, value)| Ok((key.clone(), to_json(value)?)))
            .collect::<Result<serde_json::Map<_, _>, _>>()
            .map(serde_json::Value::Object),
        HostValue::Undefined | HostValue::Buffer(_) | HostValue::Object(_) => Err(HostError::new(
            "TypeError",
            "value is not structured metadata",
        )),
    }
}

pub fn string_arg(args: &[HostValue], index: usize) -> Result<String, HostError> {
    match args.get(index) {
        Some(HostValue::String(value)) => Ok(value.to_string()),
        Some(HostValue::Utf16(value)) => String::from_utf16(value)
            .map_err(|_| HostError::new("TypeError", "string contains an unpaired surrogate")),
        _ => Err(HostError::new("TypeError", "expected a string argument")),
    }
}

pub fn number_arg(args: &[HostValue], index: usize) -> Result<f64, HostError> {
    match args.get(index) {
        Some(HostValue::Number(value)) => Ok(*value),
        _ => Err(HostError::new("TypeError", "expected a number argument")),
    }
}
