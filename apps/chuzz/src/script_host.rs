use std::any::Any;
use std::rc::Rc;

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
}

/// A native script exception with its name and message preserved.
pub struct HostError {
    pub name: String,
    pub message: String,
}

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

    fn release_callback(&mut self, id: HostCallbackId) -> Result<(), HostError>;
}
