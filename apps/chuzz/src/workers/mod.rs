//! Engine-neutral dedicated-worker protocol.
//!
//! The selected script engine owns worker realms, guest values, structured
//! clone validation, and event dispatch. Chuzz transports only owned clone
//! payloads and worker metadata across this boundary. No worker engine is
//! currently registered, so requests fail instead of pretending to execute.
#![allow(dead_code)]

use std::sync::{Arc, OnceLock};
use std::task::Waker;

use blitz_traits::net::Url;

use crate::document_loader::NetProvider;

/// Stable identifier assigned by the worker runtime.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct WorkerId(pub u64);

/// Script kind selected by the page's Worker options.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkerKind {
    Classic,
    Module,
}

/// Owned request metadata. The runtime is responsible for fetching and
/// executing the script with the creator's security context.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerRequest {
    pub url: Url,
    pub kind: WorkerKind,
    pub name: String,
}

/// Engine-produced structured-clone wire representation. Transferable
/// buffers are separated so the driver can preserve their ownership and
/// detachment semantics without exposing engine guest objects to Chuzz.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerClonePayload {
    pub graph: Vec<u8>,
    pub buffers: Vec<Vec<u8>>,
}

/// Events produced by an actual worker realm.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkerEvent {
    Message(WorkerClonePayload),
    MessageError,
    Error {
        message: String,
        filename: String,
        line: u32,
        column: u32,
        name: String,
        stack: String,
    },
    Closed,
}

/// Error returned by the worker runtime boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerError {
    pub name: String,
    pub message: String,
}

impl WorkerError {
    fn unavailable() -> Self {
        Self {
            name: String::from("NotSupportedError"),
            message: String::from("no dedicated worker runtime is registered"),
        }
    }
}

/// Operations an engine adapter must implement for real worker execution.
///
/// Implementations own worker guest realms and perform structured clone in
/// the source or destination guest. Polling must return only events that were
/// actually produced by those realms.
pub trait WorkerRuntimeDriver {
    fn create(&mut self, request: WorkerRequest) -> Result<WorkerId, WorkerError>;
    fn post_message(
        &mut self,
        worker: WorkerId,
        payload: WorkerClonePayload,
    ) -> Result<(), WorkerError>;
    fn terminate(&mut self, worker: WorkerId) -> Result<(), WorkerError>;
    fn poll(&mut self, waker: Option<&Waker>) -> Result<Vec<(WorkerId, WorkerEvent)>, WorkerError>;
}

/// Construct an engine-backed worker driver for one page's network policy.
pub type WorkerRuntimeFactory =
    fn(Arc<NetProvider>) -> Result<Box<dyn WorkerRuntimeDriver>, WorkerError>;

static WORKER_RUNTIME_FACTORY: OnceLock<WorkerRuntimeFactory> = OnceLock::new();

/// Register the real worker backend selected by the page script runtime.
pub fn register_worker_runtime_factory(factory: WorkerRuntimeFactory) -> Result<(), WorkerError> {
    WORKER_RUNTIME_FACTORY
        .set(factory)
        .map_err(|_| WorkerError {
            name: String::from("InvalidStateError"),
            message: String::from("a worker runtime factory is already registered"),
        })
}

/// Create the page's worker driver, failing explicitly when no engine adapter
/// has been installed.
pub fn new_worker_runtime(
    net: Arc<NetProvider>,
) -> Result<Box<dyn WorkerRuntimeDriver>, WorkerError> {
    let factory = WORKER_RUNTIME_FACTORY
        .get()
        .copied()
        .ok_or_else(WorkerError::unavailable)?;
    factory(net)
}
