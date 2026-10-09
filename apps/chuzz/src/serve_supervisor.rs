//! Keep the control socket responsive even when a page never returns from Boa.
//!
//! The page keeps the existing loader and document loop. Its private transport
//! is an inherited Unix socket, not another public control endpoint. Deadlines
//! are enforced outside the process executing scripts.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

use blitz_control_protocol::latest::{Latest, Once};
use blitz_control_protocol::server::{AgentControlServer, ControlBridgeRequest, Host};
use blitz_control_protocol::{
    AgentControlRequest, DebugError, DebugEvent, DebugResponse, DiagnosticsRequest,
};
use blitz_traits::net::Url;
use serde::{Deserialize, Serialize};

use super::{
    BOT_CHILD_MAX_PENDING_REQUESTS, HeadlessPolicy, Target, WallTimeoutGuard, classify, trace,
};

const WORKER_ENV: &str = "CHUZZ_HEADLESS_PAGE_WORKER";
const ROUTE_ENV: &str = "CHUZZ_HEADLESS_WORKER_ROUTE";
const REQUEST_DEADLINE: Duration = Duration::from_secs(10);
const IO_SLICE: Duration = Duration::from_millis(8);
const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

type Answer = Arc<Once<DebugResponse>>;
type DocumentRequest = (ControlBridgeRequest, Answer);

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(super) enum Operation {
    Load,
    Poll,
    Resolve,
    Action,
    Storage,
    Request,
}

impl Operation {
    fn budget(self) -> Duration {
        match self {
            Self::Load | Self::Request => REQUEST_DEADLINE,
            Self::Poll | Self::Resolve | Self::Action => Duration::from_secs(5),
            Self::Storage => Duration::from_secs(2),
        }
    }
}

#[derive(Serialize, Deserialize)]
enum WorkerFrame {
    Begin(Operation),
    End,
    Ready(String),
    Reply(DebugResponse),
    Paint,
    Failed(String),
}

#[derive(Serialize, Deserialize)]
enum WireRequest {
    Agent(AgentControlRequest),
    Diagnostics(DiagnosticsRequest),
}

impl From<ControlBridgeRequest> for WireRequest {
    fn from(request: ControlBridgeRequest) -> Self {
        match request {
            ControlBridgeRequest::Agent(request) => Self::Agent(request),
            ControlBridgeRequest::Diagnostics(request) => Self::Diagnostics(request),
        }
    }
}

impl From<WireRequest> for ControlBridgeRequest {
    fn from(request: WireRequest) -> Self {
        match request {
            WireRequest::Agent(request) => Self::Agent(request),
            WireRequest::Diagnostics(request) => Self::Diagnostics(request),
        }
    }
}

fn encode(value: &impl Serialize) -> Result<Vec<u8>, String> {
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err("the page worker frame exceeded its byte limit".to_owned());
    }
    let mut frame = Vec::with_capacity(4 + bytes.len());
    frame.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    frame.extend_from_slice(&bytes);
    Ok(frame)
}

fn page_error(message: &str) -> DebugResponse {
    DebugResponse::Error(DebugError {
        code: "pageUnresponsive".into(),
        message: format!(
            "{message}; an action may already have taken effect, so do not retry it automatically"
        ),
    })
}

fn not_ready(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
    )
}

static WORKER_WIRE: OnceLock<Mutex<UnixStream>> = OnceLock::new();

pub(super) fn is_worker() -> bool {
    matches!(std::env::var(WORKER_ENV), Ok(value) if value == "1")
}

pub(super) fn launch_route() -> Option<String> {
    std::env::var(ROUTE_ENV).ok()
}

fn send(frame: &WorkerFrame) {
    let result = (|| {
        let bytes = encode(frame)?;
        let wire = WORKER_WIRE
            .get()
            .ok_or("the page worker transport was not initialized")?;
        let mut wire = wire
            .lock()
            .map_err(|_| "the page worker transport lock was poisoned")?;
        wire.write_all(&bytes).map_err(|error| error.to_string())
    })();
    if let Err(error) = result {
        trace(&format!("page worker transport failed: {error}"));
        std::process::exit(1);
    }
}

pub(super) struct OperationGuard;

pub(super) fn operation(operation: Operation) -> Option<OperationGuard> {
    if !is_worker() {
        return None;
    }
    send(&WorkerFrame::Begin(operation));
    Some(OperationGuard)
}

impl Drop for OperationGuard {
    fn drop(&mut self) {
        send(&WorkerFrame::End);
    }
}

pub(super) fn ready(url: &Url) {
    send(&WorkerFrame::Ready(url.to_string()));
}

pub(super) fn painted() {
    if is_worker() {
        send(&WorkerFrame::Paint);
    }
}

pub(super) fn failed(message: &str) {
    send(&WorkerFrame::Failed(message.to_owned()));
}

pub(super) fn start_worker(sender: mpsc::SyncSender<DocumentRequest>) -> Result<(), String> {
    // Only the internally launched worker receives a socket as stdin. Keep
    // stdout separate so page console output cannot corrupt this transport.
    let descriptor = io::stdin()
        .as_fd()
        .try_clone_to_owned()
        .map_err(|error| format!("could not inherit the page worker socket: {error}"))?;
    let wire = UnixStream::from(descriptor);
    wire.set_nonblocking(false)
        .map_err(|error| error.to_string())?;
    let mut reader = wire.try_clone().map_err(|error| error.to_string())?;
    WORKER_WIRE
        .set(Mutex::new(wire))
        .map_err(|_| "the page worker transport was initialized twice".to_owned())?;

    std::thread::Builder::new()
        .name("chuzz-page-control".to_owned())
        .spawn(move || {
            loop {
                let request = (|| {
                    let mut header = [0_u8; 4];
                    reader.read_exact(&mut header)?;
                    let length = u32::from_be_bytes(header) as usize;
                    if length > MAX_FRAME_BYTES {
                        return Err(io::Error::other(
                            "page worker request exceeded its byte limit",
                        ));
                    }
                    let mut bytes = vec![0_u8; length];
                    reader.read_exact(&mut bytes)?;
                    serde_json::from_slice::<WireRequest>(&bytes)
                        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
                })();
                let Ok(request) = request else {
                    // Also handles a killed supervisor during initial loading:
                    // this thread exists before any page script executes.
                    std::process::exit(0);
                };
                let answer = Once::new();
                match sender.try_send((request.into(), Arc::clone(&answer))) {
                    Ok(()) => {}
                    Err(mpsc::TrySendError::Full(_)) => {
                        answer.fill(DebugResponse::Error(DebugError {
                            code: "documentBusy".into(),
                            message: "the page worker request queue is full".into(),
                        }));
                    }
                    Err(mpsc::TrySendError::Disconnected(_)) => {
                        failed("the page document loop stopped");
                        std::process::exit(1);
                    }
                }
                let response = nagoya::block_on(answer.recv())
                    .unwrap_or_else(|| page_error("the page document loop closed its reply"));
                send(&WorkerFrame::Reply(response));
            }
        })
        .map_err(|error| format!("could not start the page worker control reader: {error}"))?;
    Ok(())
}

struct Session {
    child: Child,
    wire: UnixStream,
    incoming: Vec<u8>,
    outgoing: Vec<u8>,
    written: usize,
    ready: bool,
    local: bool,
    startup_deadline: Instant,
    operations: Vec<(Operation, Instant)>,
}

impl Session {
    fn spawn(target: &str, route: Option<&str>, local: bool) -> Result<Self, String> {
        let (wire, worker) = UnixStream::pair().map_err(|error| error.to_string())?;
        wire.set_nonblocking(false)
            .map_err(|error| error.to_string())?;
        wire.set_read_timeout(Some(IO_SLICE))
            .map_err(|error| error.to_string())?;
        wire.set_write_timeout(Some(IO_SLICE))
            .map_err(|error| error.to_string())?;
        let input: OwnedFd = worker.into();
        let mut command = Command::new(std::env::current_exe().map_err(|error| error.to_string())?);
        command
            .arg(target)
            .env(WORKER_ENV, "1")
            .env_remove(ROUTE_ENV)
            .stdin(Stdio::from(input))
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        if let Some(route) = route {
            command.env(ROUTE_ENV, route);
        }
        let startup_deadline = Instant::now() + REQUEST_DEADLINE;
        let child = command.spawn().map_err(|error| error.to_string())?;
        Ok(Self {
            child,
            wire,
            incoming: Vec::new(),
            outgoing: Vec::new(),
            written: 0,
            ready: false,
            local,
            startup_deadline,
            operations: Vec::new(),
        })
    }

    fn queue(&mut self, request: WireRequest) -> Result<(), String> {
        self.outgoing = encode(&request)?;
        self.written = 0;
        Ok(())
    }

    fn expired(&self, now: Instant) -> Option<String> {
        if !self.ready && now >= self.startup_deadline {
            return Some("the page did not finish loading within 10s".to_owned());
        }
        self.operations
            .iter()
            .find(|(_, deadline)| now >= *deadline)
            .map(|(operation, _)| format!("the page exceeded its {operation:?} deadline"))
    }

    fn pump(&mut self) -> Result<Vec<WorkerFrame>, String> {
        if self.written < self.outgoing.len() {
            match self.wire.write(&self.outgoing[self.written..]) {
                Ok(0) => return Err("the page worker stopped reading requests".to_owned()),
                Ok(length) => self.written += length,
                Err(error) if not_ready(&error) => {}
                Err(error) => return Err(format!("could not write to the page worker: {error}")),
            }
            if self.written == self.outgoing.len() {
                self.outgoing.clear();
                self.written = 0;
            }
        }

        let mut bytes = [0_u8; 65_536];
        match self.wire.read(&mut bytes) {
            Ok(0) => return Err("the page worker closed its transport".to_owned()),
            Ok(length) => self.incoming.extend_from_slice(&bytes[..length]),
            // EAGAIN on macOS is readiness, not a dead page. Keep both partial
            // incoming frames and partial outgoing writes until the deadline.
            Err(error) if not_ready(&error) => return Ok(Vec::new()),
            Err(error) => return Err(format!("could not read the page worker: {error}")),
        }

        let mut frames = Vec::new();
        let mut consumed = 0;
        while self.incoming.len() - consumed >= 4 {
            let header = &self.incoming[consumed..consumed + 4];
            let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
            if length > MAX_FRAME_BYTES {
                return Err("the page worker reply exceeded its byte limit".to_owned());
            }
            if self.incoming.len() - consumed - 4 < length {
                break;
            }
            let end = consumed + 4 + length;
            let frame = serde_json::from_slice(&self.incoming[consumed + 4..end])
                .map_err(|error| format!("invalid page worker frame: {error}"))?;
            frames.push(frame);
            consumed = end;
        }
        if consumed > 0 {
            self.incoming.drain(..consumed);
        }
        Ok(frames)
    }
}

struct QueuedRequest {
    request: ControlBridgeRequest,
    answer: Answer,
    deadline: Instant,
}

struct PendingRequest {
    answer: Answer,
    deadline: Instant,
    acknowledge_ready: bool,
}

struct Supervisor {
    session: Option<Session>,
    retired: Vec<Child>,
    pending: Option<PendingRequest>,
    failure: Option<String>,
    url: Option<Url>,
    local_target: Option<String>,
    local_origin: Option<String>,
    policy: HeadlessPolicy,
    events: Arc<Latest<DebugEvent>>,
    render_revision: u64,
}

impl Supervisor {
    fn fail(&mut self, message: &str) {
        let message = if self.policy.bot_child {
            "the page stopped responding within its execution deadline".to_owned()
        } else {
            message.to_owned()
        };
        trace(&message);
        if let Some(pending) = self.pending.take() {
            pending.answer.fill(page_error(&message));
        }
        if let Some(session) = self.session.take() {
            let Session { mut child, .. } = session;
            let _ = child.kill();
            self.retired.push(child);
        }
        self.failure = Some(message);
    }

    fn check_deadlines(&mut self) {
        let now = Instant::now();
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| now >= pending.deadline)
        {
            self.fail("the page did not answer the control request within 10s");
            return;
        }
        if let Some(message) = self
            .session
            .as_ref()
            .and_then(|session| session.expired(now))
        {
            self.fail(&message);
        }
    }

    fn frame(&mut self, frame: WorkerFrame) {
        match frame {
            WorkerFrame::Begin(operation) => {
                if let Some(session) = &mut self.session {
                    session
                        .operations
                        .push((operation, Instant::now() + operation.budget()));
                }
            }
            WorkerFrame::End => {
                if let Some(session) = &mut self.session
                    && session.operations.pop().is_none()
                {
                    self.fail("the page worker ended an unknown operation");
                }
            }
            WorkerFrame::Ready(url) => {
                let Ok(url) = Url::parse(&url) else {
                    self.fail("the page worker reported an invalid document URL");
                    return;
                };
                if let Some(session) = &mut self.session {
                    if !session.ready && session.local {
                        self.local_origin = Some(url.origin().ascii_serialization());
                    }
                    session.ready = true;
                }
                self.url = Some(url);
                if self
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.acknowledge_ready)
                    && let Some(pending) = self.pending.take()
                {
                    pending.answer.fill(DebugResponse::Ack);
                }
            }
            WorkerFrame::Reply(response) => {
                if let DebugResponse::Error(error) = &response
                    && error.code == "pageUnresponsive"
                {
                    let message = error.message.clone();
                    if let Some(pending) = self.pending.take() {
                        pending.answer.fill(response);
                    }
                    self.fail(&message);
                    return;
                }
                match self.pending.take() {
                    Some(pending) if !pending.acknowledge_ready => {
                        pending.answer.fill(response);
                    }
                    Some(pending) => {
                        pending.answer.fill(page_error(
                            "the page worker replied before navigation was ready",
                        ));
                        self.fail("the page worker replied before navigation was ready");
                    }
                    None => self.fail("the page worker sent an unsolicited reply"),
                }
            }
            WorkerFrame::Paint => {
                self.render_revision = self.render_revision.saturating_add(1);
                self.events.set_now(DebugEvent::PaintCommitted {
                    revision: self.render_revision,
                });
            }
            WorkerFrame::Failed(message) => self.fail(&message),
        }
    }

    fn accept(&mut self, queued: QueuedRequest) {
        if let Some(message) = &self.failure {
            let ControlBridgeRequest::Agent(AgentControlRequest::Navigate { url }) = queued.request
            else {
                queued.answer.fill(page_error(message));
                return;
            };
            if self.policy.bot_child {
                queued.answer.fill(DebugResponse::Error(DebugError {
                    code: "policyDenied".into(),
                    message: "a failed bot page cannot restart its browser budgets".into(),
                }));
                return;
            }
            let destination = match &self.url {
                Some(base) => base.join(&url),
                None => Url::parse(&url),
            };
            let destination = match destination {
                Ok(destination) => destination,
                Err(error) => {
                    queued.answer.fill(DebugResponse::Error(DebugError {
                        code: "invalidArgument".into(),
                        message: format!("could not resolve the recovery navigation: {error}"),
                    }));
                    return;
                }
            };
            if let Err(message) = self.policy.validate_url(&destination) {
                queued.answer.fill(DebugResponse::Error(DebugError {
                    code: "policyDenied".into(),
                    message,
                }));
                return;
            }
            let local = self.local_target.is_some()
                && self.local_origin.as_deref()
                    == Some(destination.origin().ascii_serialization().as_str());
            let mut route = destination.path().to_owned();
            if let Some(query) = destination.query() {
                route.push('?');
                route.push_str(query);
            }
            if let Some(fragment) = destination.fragment() {
                route.push('#');
                route.push_str(fragment);
            }
            let target = if local {
                self.local_target.as_deref().unwrap_or(destination.as_str())
            } else {
                destination.as_str()
            };
            match Session::spawn(target, local.then_some(route.as_str()), local) {
                Ok(session) => {
                    self.session = Some(session);
                    self.failure = None;
                    self.pending = Some(PendingRequest {
                        answer: queued.answer,
                        deadline: queued.deadline,
                        acknowledge_ready: true,
                    });
                }
                Err(message) => {
                    queued.answer.fill(page_error(&message));
                    self.fail(&message);
                }
            }
            return;
        }
        let result = self
            .session
            .as_mut()
            .ok_or_else(|| "the page worker is unavailable".to_owned())
            .and_then(|session| session.queue(queued.request.into()));
        match result {
            Ok(()) => {
                self.pending = Some(PendingRequest {
                    answer: queued.answer,
                    deadline: queued.deadline,
                    acknowledge_ready: false,
                });
            }
            Err(message) => {
                queued.answer.fill(page_error(&message));
                self.fail(&message);
            }
        }
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        if let Some(session) = &mut self.session {
            let _ = session.child.kill();
        }
        for child in &mut self.retired {
            let _ = child.kill();
        }
    }
}

pub(super) fn serve(target: &str) -> Result<(), String> {
    let policy = HeadlessPolicy::from_env()?;
    let classified = classify(target)?;
    policy.validate_target(&classified)?;
    let local = matches!(classified, Target::Directory(_) | Target::File { .. });
    let _wall_timeout = if policy.bot_child {
        Some(WallTimeoutGuard::start(policy.wall_time)?)
    } else {
        None
    };
    let max_pending = if policy.bot_child {
        BOT_CHILD_MAX_PENDING_REQUESTS
    } else {
        64
    };
    let inbox = Arc::new(Mutex::new(VecDeque::<QueuedRequest>::new()));
    let bridge_inbox = Arc::clone(&inbox);
    let bridge: blitz_control_protocol::server::ControlBridge = Arc::new(move |request| {
        let answer = Once::new();
        let deadline = Instant::now() + REQUEST_DEADLINE;
        match bridge_inbox.lock() {
            Ok(mut inbox) if inbox.len() < max_pending => {
                inbox.push_back(QueuedRequest {
                    request,
                    answer: Arc::clone(&answer),
                    deadline,
                });
            }
            Ok(_) => {
                answer.fill(DebugResponse::Error(DebugError {
                    code: "documentBusy".into(),
                    message: "the inspection request queue is full".into(),
                }));
            }
            Err(_) => {
                answer.fill(page_error("the inspection request queue is unavailable"));
            }
        }
        answer
    });
    let events = Latest::new();
    let server = AgentControlServer::start_with_events(
        bridge,
        Host {
            name: "chuzz-headless".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            diagnostics: !policy.bot_child,
        },
        Arc::clone(&events),
    )
    .map_err(|error| format!("could not host the control socket: {error}"))?;
    trace(&format!(
        "inspection socket listening: {}",
        server.descriptor_path().display()
    ));
    println!("{}", server.descriptor_path().display());
    io::stdout()
        .flush()
        .map_err(|error| format!("could not announce the control socket: {error}"))?;

    let (session, failure) = match Session::spawn(target, None, local) {
        Ok(session) => (Some(session), None),
        Err(message) => (None, Some(message)),
    };
    let mut supervisor = Supervisor {
        session,
        retired: Vec::new(),
        pending: None,
        failure,
        url: Url::parse(target).ok(),
        local_target: local.then(|| target.to_owned()),
        local_origin: None,
        policy,
        events,
        render_revision: 0,
    };
    loop {
        supervisor.check_deadlines();
        let frames = match &mut supervisor.session {
            Some(session) => session.pump(),
            None => Ok(Vec::new()),
        };
        // Check before applying End or Reply, so late data cannot erase an
        // expired execution or request deadline.
        supervisor.check_deadlines();
        match frames {
            Ok(frames) => {
                for frame in frames {
                    if supervisor.session.is_none() {
                        break;
                    }
                    supervisor.frame(frame);
                }
            }
            Err(message) => supervisor.fail(&message),
        }
        // Never block the public control loop waiting to reap a killed page.
        supervisor
            .retired
            .retain_mut(|child| !matches!(child.try_wait(), Ok(Some(_)) | Err(_)));

        let now = Instant::now();
        let (expired, next) = {
            let mut inbox = inbox
                .lock()
                .map_err(|_| "the inspection request queue was poisoned".to_owned())?;
            let mut expired = Vec::new();
            inbox.retain(|queued| {
                if now >= queued.deadline {
                    expired.push(Arc::clone(&queued.answer));
                    false
                } else {
                    true
                }
            });
            let next = if supervisor.pending.is_none() && supervisor.retired.is_empty() {
                inbox.pop_front()
            } else {
                None
            };
            (expired, next)
        };
        for answer in expired {
            answer.fill(page_error(
                "the control request expired while waiting for the page",
            ));
        }
        if let Some(next) = next {
            supervisor.accept(next);
        }
        if supervisor.session.is_none() {
            std::thread::sleep(IO_SLICE);
        }
    }
}
