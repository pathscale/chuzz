//! The page and its local control socket share one process and nagoya tasks.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::{ffi::OsStrExt, fs::{OpenOptionsExt, PermissionsExt}};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Waker};
use std::time::Duration;

use blitz_control_protocol::latest::{Flag, Latest, Once};
use blitz_control_protocol::server::ControlBridgeRequest;
use blitz_control_protocol::{
    self as protocol, DebugDescriptor, DebugEvent, DebugResponse, DebugStream,
    DiagnosticsRequest, IncomingRequest, NagoyaStream, framed_json_neutral,
};
use blitz_traits::net::Url;
use endpoint_libs::libs::ws::{MessageStream, StreamError, WireMessage};
use endpoint_libs::libs::ws::mcp_wire::{INVALID_REQUEST, JsonRpcError};
use endpoint_libs::libs::ws::transport::TransportStream;
use futures::future::{Either, select};
use nagoya::net::{TcpListener, TcpStream};
use nagoya::reactor::{Addr, Reactor};

use super::page_control;

struct ControlState {
    sender: Mutex<page_control::Sender>,
    events: Arc<Latest<DebugEvent>>,
    descriptor: PathBuf,
    ready: OnceLock<Url>,
    listener: Mutex<Option<TcpListener>>,
    gate: nagoya::sync::Semaphore,
}

static CONTROL: OnceLock<Arc<ControlState>> = OnceLock::new();
static WAKER: OnceLock<Waker> = OnceLock::new();

pub(super) fn events() -> Arc<Latest<DebugEvent>> {
    Arc::clone(&CONTROL.get().expect("control socket initialized").events)
}

pub(super) fn task_context() -> Context<'static> {
    Context::from_waker(WAKER.get().expect("document wake initialized"))
}

pub(super) fn network_wake() -> Waker {
    WAKER.get().expect("document wake initialized").clone()
}

pub(super) fn ready(url: &Url) {
    let state = CONTROL.get().expect("control socket initialized");
    if state.ready.set(url.clone()).is_ok() {
        println!("{}", state.descriptor.display());
        if std::io::stdout().flush().is_err() { std::process::exit(1); }
    }
}

pub(super) fn start_worker(sender: page_control::Sender) -> Result<(), String> {
    crate::sandbox::ensure()?;
    let _runtime = nagoya::runtime::background();
    static REACTOR: OnceLock<Result<Reactor, String>> = OnceLock::new();
    let reactor = REACTOR.get_or_init(|| Reactor::start().map_err(|error| error.to_string()))
        .as_ref().map_err(Clone::clone)?;
    let dirs = crate::sandbox::directories().ok_or("sandbox directories unavailable")?;
    let directory = dirs.cache.join("control");
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).map_err(|error| error.to_string())?;
    let instance = format!("{}-{:x}", std::process::id(), std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).map_err(|error| error.to_string())?.as_nanos());
    let descriptor = directory.join(format!("{instance}.json"));
    let socket = descriptor.with_extension("sock");
    let address = Addr::path(socket.as_os_str().as_bytes()).map_err(|error| format!("control address: {error:?}"))?;
    let listener = TcpListener::bind(address, &reactor.handle()).map_err(|error| error.to_string())?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).map_err(|error| error.to_string())?;
    let published = DebugDescriptor {
        protocol_version: protocol::DEBUG_PROTOCOL_VERSION,
        pid: std::process::id(), instance_id: instance,
        address: format!("unix://{}", socket.display()),
        renderer: "blitz".to_owned(), renderer_revision: env!("CARGO_PKG_VERSION").to_owned(),
    };
    let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600)
        .open(&descriptor).map_err(|error| error.to_string())?;
    serde_json::to_writer(&mut file, &published).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    WAKER.set(sender.document_waker()).map_err(|_| "document wake initialized twice")?;
    let state = Arc::new(ControlState {
        sender: Mutex::new(sender), events: Latest::new(), descriptor,
        ready: OnceLock::new(), listener: Mutex::new(Some(listener)),
        gate: nagoya::sync::Semaphore::new(16),
    });
    CONTROL.set(Arc::clone(&state)).map_err(|_| "control socket initialized twice")?;
    drop(nagoya::spawn(listen(state)));
    Ok(())
}

async fn listen(state: Arc<ControlState>) -> Arc<ControlState> {
    let listener = state.listener.lock().unwrap_or_else(|poison| poison.into_inner())
        .take().expect("one listener task");
    while let Ok((stream, _)) = listener.accept().await {
        let work = Arc::new(ConnectionWork { state: Arc::clone(&state), stream: Mutex::new(Some(stream)) });
        drop(nagoya::spawn(connection(work)));
    }
    eprintln!("chuzz: control-listener-closed");
    std::process::exit(1);
}

struct ConnectionWork { state: Arc<ControlState>, stream: Mutex<Option<TcpStream>> }

enum Input {
    Message(Option<Result<WireMessage, StreamError>>),
    Event(Option<DebugEvent>, u64),
}

fn selected(streams: &[DebugStream], event: &DebugEvent) -> bool {
    match event {
        DebugEvent::DiagnosticLoss { stream, .. } => streams.contains(stream),
        DebugEvent::Snapshot(_) => streams.contains(&DebugStream::Snapshots),
        DebugEvent::Metrics(_) => streams.contains(&DebugStream::Metrics),
        DebugEvent::Console(_) => streams.contains(&DebugStream::Console),
        DebugEvent::RuntimeError(_) => streams.contains(&DebugStream::RuntimeErrors),
        DebugEvent::PaintCommitted { .. } => streams.contains(&DebugStream::Paint),
        _ => false,
    }
}

async fn response(state: &ControlState, request: ControlBridgeRequest) -> DebugResponse {
    let answer = page_control::Reply::new();
    let sent = state.sender.lock().unwrap_or_else(|poison| poison.into_inner())
        .try_send(Arc::new((request, Arc::clone(&answer))));
    if sent.is_err() {
        return DebugResponse::Error(protocol::DebugError { code: "documentBusy".into(), message: "document request queue unavailable".into() });
    }
    match nagoya::timeout(Duration::from_secs(60), answer.recv()).await {
        Ok(Some(response)) => Arc::try_unwrap(response).unwrap_or_else(|response| response.as_ref().clone()),
        _ => DebugResponse::Error(protocol::DebugError { code: "pageUnresponsive".into(), message: "the page exceeded its response deadline; do not automatically retry an action".into() }),
    }
}

async fn connection(work: Arc<ConnectionWork>) -> Arc<ConnectionWork> {
    let _lease = work.state.gate.acquire().await;
    let socket = work.stream.lock().unwrap_or_else(|poison| poison.into_inner()).take().expect("one connection task");
    let mut wire = TransportStream::new(framed_json_neutral(NagoyaStream::new(socket)));
    let mut observed = Vec::new();
    let mut seen = work.state.events.revision();
    loop {
        let input = if observed.is_empty() { Input::Message(wire.recv().await) } else {
            let message = wire.recv();
            let changed = work.state.events.changed_since(seen);
            futures::pin_mut!(message, changed);
            match select(message, changed).await {
                Either::Left((message, _)) => Input::Message(message),
                Either::Right(((event, revision), _)) => Input::Event(event, revision),
            }
        };
        let message = match input {
            Input::Event(event, revision) => {
                seen = revision;
                if let Some(event) = event && selected(&observed, &event)
                    && let Ok(frame) = protocol::encode_diagnostics_event(&event)
                    && wire.send(frame).await.is_err() { break; }
                continue;
            }
            Input::Message(Some(Ok(message))) => message,
            Input::Message(_) => break,
        };
        let frame = match protocol::decode_wire_value(message) {
            Ok(value) => {
                let id = protocol::peek_value_request_id(&value);
                match protocol::decode_incoming_value(value) {
                    Ok(IncomingRequest::Initialize { id }) => protocol::encode_initialize_response(id, "chuzz-headless", env!("CARGO_PKG_VERSION")),
                    Ok(IncomingRequest::Initialized) => continue,
                    Ok(IncomingRequest::ToolsList { id }) => protocol::encode_tools_list_response(id, true),
                    Ok(IncomingRequest::Agent { id, request }) => protocol::encode_response(id, &response(&work.state, ControlBridgeRequest::Agent(request)).await),
                    Ok(IncomingRequest::Diagnostics { id, request: DiagnosticsRequest::Observe { streams } }) => {
                        observed = streams; seen = work.state.events.revision();
                        protocol::encode_response(id, &DebugResponse::Ack)
                    }
                    Ok(IncomingRequest::Diagnostics { id, request }) => protocol::encode_response(id, &response(&work.state, ControlBridgeRequest::Diagnostics(request)).await),
                    Ok(IncomingRequest::Targeted { id, request }) => {
                        let streams = match &request.command { protocol::TargetCommand::Diagnostics(DiagnosticsRequest::Observe { streams }) => Some(streams.clone()), _ => None };
                        let after = request.options.after_sequence;
                        let result = if after.is_some_and(|after| after > work.state.events.revision()) {
                            DebugResponse::Error(protocol::target::error("invalidSequence", "afterSequence exceeds the current revision"))
                        } else { response(&work.state, ControlBridgeRequest::Targeted(request)).await };
                        if matches!(result.unscoped(), DebugResponse::Ack) && let Some(streams) = streams {
                            observed = streams; seen = after.unwrap_or_else(|| work.state.events.revision());
                        }
                        protocol::encode_response(id, &result)
                    }
                    Ok(_) => protocol::encode_rpc_error(id, JsonRpcError::new(INVALID_REQUEST, "unsupported control request")),
                    Err(error) => protocol::encode_rpc_error(id, JsonRpcError::new(INVALID_REQUEST, error.to_string())),
                }
            }
            Err(error) => protocol::encode_rpc_error(None, JsonRpcError::new(INVALID_REQUEST, error.to_string())),
        };
        let Ok(frame) = frame else { break; };
        if wire.send(frame).await.is_err() { break; }
    }
    drop(_lease);
    work
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Operation { Load, Poll, Resolve, Action, Storage, Request }

struct Deadline { done: Arc<Flag>, operation: Operation, duration: Duration }
pub(super) struct OperationGuard { work: Arc<Deadline> }

impl Drop for OperationGuard { fn drop(&mut self) { self.work.done.raise(); } }

pub(super) fn operation(operation: Operation) -> OperationGuard {
    let duration = match operation { Operation::Load => Duration::from_secs(60), Operation::Poll | Operation::Resolve | Operation::Action => Duration::from_secs(15), Operation::Storage => Duration::from_secs(2), Operation::Request => Duration::from_secs(10) };
    let work = Arc::new(Deadline { done: Flag::new(), operation, duration });
    drop(nagoya::spawn(deadline(Arc::clone(&work))));
    OperationGuard { work }
}

async fn deadline(work: Arc<Deadline>) -> Arc<Deadline> {
    if nagoya::timeout(work.duration, work.done.raised()).await.is_err() {
        eprintln!("chuzz: page-execution-deadline: {:?}", work.operation);
        std::process::exit(124);
    }
    work
}
