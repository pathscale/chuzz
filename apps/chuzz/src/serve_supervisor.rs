//! Keep the control socket responsive even when a page never returns from Boa.
//!
//! The page keeps the existing loader and document loop. Its private transport
//! is an inherited Unix socket, not another public control endpoint. Deadlines
//! are enforced outside the process executing scripts.

use nagoya::process::Child;
use std::collections::VecDeque;
use std::io::{self, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use blitz_control_protocol::latest::{Latest, Once};
use blitz_control_protocol::server::{AgentControlServer, ControlBridgeRequest, Host};
use blitz_control_protocol::{
    AgentControlRequest, DebugError, DebugEvent, DebugResponse, DiagnosticsRequest,
};
use blitz_traits::net::Url;
use serde::{Deserialize, Serialize};

use super::{
    BOT_CHILD_MAX_PENDING_REQUESTS, HeadlessPolicy, Target, WallTimeoutGuard, classify,
    page_control, trace,
};

const WORKER_ENV: &str = "CHUZZ_HEADLESS_PAGE_WORKER";
const ROUTE_ENV: &str = "CHUZZ_HEADLESS_WORKER_ROUTE";
const REQUEST_DEADLINE: Duration = Duration::from_secs(10);
/// A real site fetches dozens of scripts over the network before it is ready;
/// ten seconds is a budget for local fixtures, not for the web.
const REMOTE_LOAD_DEADLINE: Duration = Duration::from_secs(60);

/// The worker link carries the same answers the public socket sends, so it
/// takes the public frame limit. A larger worker frame could only be refused
/// one hop later, by the encoder that writes it to the client.
const MAX_FRAME_BYTES: usize = blitz_control_protocol::MAX_DEBUG_FRAME_BYTES;

type Answer = Arc<Once<DebugResponse>>;

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
            // A hang guard, not a speed limit: one interpreted task (three.js compiling node
            // materials for a planet) measured 4.9 s, so 5 s killed working pages.
            Self::Poll | Self::Resolve | Self::Action => Duration::from_secs(15),
            Self::Storage => Duration::from_secs(2),
        }
    }
}

#[derive(Serialize, Deserialize)]
enum WorkerFrame {
    Begin(Operation),
    End,
    Ready(String),
    Reply(Box<DebugResponse>),
    Paint,
    Failed(String),
}

#[derive(Serialize, Deserialize)]
enum WireRequest {
    Agent(AgentControlRequest),
    Diagnostics(DiagnosticsRequest),
    Targeted(blitz_control_protocol::TargetRequest),
}

impl From<ControlBridgeRequest> for WireRequest {
    fn from(request: ControlBridgeRequest) -> Self {
        match request {
            ControlBridgeRequest::Agent(request) => Self::Agent(request),
            ControlBridgeRequest::Diagnostics(request) => Self::Diagnostics(request),
            ControlBridgeRequest::Targeted(request) => Self::Targeted(request),
        }
    }
}

impl From<WireRequest> for ControlBridgeRequest {
    fn from(request: WireRequest) -> Self {
        match request {
            WireRequest::Agent(request) => Self::Agent(request),
            WireRequest::Diagnostics(request) => Self::Diagnostics(request),
            WireRequest::Targeted(request) => Self::Targeted(request),
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

fn worker_reactor() -> Result<&'static nagoya::reactor::Reactor, String> {
    static REACTOR: OnceLock<Result<nagoya::reactor::Reactor, String>> = OnceLock::new();
    REACTOR
        .get_or_init(|| nagoya::reactor::Reactor::start().map_err(|error| error.to_string()))
        .as_ref()
        .map_err(Clone::clone)
}

fn adopt_wire(wire: UnixStream) -> Result<nagoya::net::TcpStream, String> {
    use std::os::fd::IntoRawFd;
    wire.set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    let handle = worker_reactor()?.handle();
    // SAFETY: into_raw_fd transfers sole ownership of the connected socket.
    let socket = unsafe { nagoya::reactor::socket::TcpSocket::from_raw(wire.into_raw_fd()) };
    nagoya::net::TcpStream::from_socket(socket, &handle).map_err(|error| error.to_string())
}

struct WorkerWire {
    outgoing: Mutex<spsc::Producer<Arc<Vec<u8>>>>,
    ready: nagoya::sync::Notify,
}

static WORKER_WIRE: OnceLock<Arc<WorkerWire>> = OnceLock::new();

pub(super) fn is_worker() -> bool {
    matches!(std::env::var(WORKER_ENV), Ok(value) if value == "1")
}

pub(super) fn launch_route() -> Option<String> {
    std::env::var(ROUTE_ENV).ok()
}

fn send(frame: &WorkerFrame) {
    let result = (|| {
        let bytes = Arc::new(encode(frame)?);
        let wire = WORKER_WIRE
            .get()
            .ok_or("the page worker transport was not initialized")?;
        let producer = wire
            .outgoing
            .lock()
            .map_err(|_| "the page worker transport lock was poisoned")?;
        if producer.try_push(bytes).is_some() {
            return Err("the page worker output queue is full".to_owned());
        }
        drop(producer);
        wire.ready.notify_one();
        Ok(())
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

async fn read_exact(reader: &mut nagoya::net::TcpStream, bytes: &mut [u8]) -> Result<(), String> {
    let mut read = 0;
    while read < bytes.len() {
        let length = reader
            .read(&mut bytes[read..])
            .await
            .map_err(|error| error.to_string())?;
        if length == 0 {
            return Err("the page worker closed its transport".into());
        }
        read += length;
    }
    Ok(())
}

pub(super) fn start_worker(sender: page_control::Sender) -> Result<(), String> {
    let descriptor = io::stdin()
        .as_fd()
        .try_clone_to_owned()
        .map_err(|error| format!("could not inherit the page worker socket: {error}"))?;
    let wire = UnixStream::from(descriptor);
    let mut reader = adopt_wire(wire.try_clone().map_err(|error| error.to_string())?)?;
    let mut writer = adopt_wire(wire)?;
    let (producer, consumer) = spsc::make(128);
    let wire = Arc::new(WorkerWire {
        outgoing: Mutex::new(producer),
        ready: nagoya::sync::Notify::new(),
    });
    WORKER_WIRE
        .set(Arc::clone(&wire))
        .map_err(|_| "the page worker transport was initialized twice".to_owned())?;
    drop(nagoya::spawn(async move {
        loop {
            if let Some(bytes) = consumer.try_pop() {
                if writer.write_all(&bytes).await.is_err() {
                    std::process::exit(0);
                }
            } else {
                wire.ready.notified().await;
            }
        }
    }));
    drop(nagoya::spawn(async move {
        loop {
            let request = async {
                let mut header = [0_u8; 4];
                read_exact(&mut reader, &mut header).await?;
                let length = u32::from_be_bytes(header) as usize;
                if length > MAX_FRAME_BYTES {
                    return Err("page worker request exceeded its byte limit".to_owned());
                }
                let mut bytes = vec![0; length];
                read_exact(&mut reader, &mut bytes).await?;
                serde_json::from_slice::<WireRequest>(&bytes).map_err(|error| error.to_string())
            }
            .await;
            let Ok(request) = request else {
                std::process::exit(0);
            };
            let answer = Once::new();
            match sender.try_send(Arc::new((request.into(), Arc::clone(&answer)))) {
                Ok(()) => false,
                Err(page_control::TrySendError::Full) => {
                    answer.fill(DebugResponse::Error(DebugError {
                        code: "documentBusy".into(),
                        message: "the page worker request queue is full".into(),
                    }))
                }
                Err(page_control::TrySendError::Disconnected) => {
                    failed("the page document loop stopped");
                    std::process::exit(1);
                }
            }
            let response = answer
                .recv()
                .await
                .unwrap_or_else(|| page_error("the page document loop closed its reply"));
            send(&WorkerFrame::Reply(Box::new(response)));
        }
    }));
    Ok(())
}

struct Session {
    child: Child,
    wire: nagoya::net::TcpStream,
    incoming: bytes::BytesMut,
    outgoing: Arc<Vec<u8>>,
    written: usize,
    ready: bool,
    local: bool,
    startup_deadline: Instant,
    operations: Vec<(Operation, Instant)>,
}

impl Session {
    fn spawn(target: &str, route: Option<&str>, local: bool) -> Result<Self, String> {
        let (wire, worker) = UnixStream::pair().map_err(|error| error.to_string())?;
        let wire = adopt_wire(wire)?;
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
        let startup_deadline = Instant::now()
            + if local {
                REQUEST_DEADLINE
            } else {
                REMOTE_LOAD_DEADLINE
            };
        let child = nagoya::process::Command::from(command)
            .kill_on_drop(true)
            .spawn(&worker_reactor()?.handle())
            .map_err(|error| error.to_string())?;
        Ok(Self {
            child,
            wire,
            incoming: bytes::BytesMut::new(),
            outgoing: Arc::new(Vec::new()),
            written: 0,
            ready: false,
            local,
            startup_deadline,
            operations: Vec::new(),
        })
    }

    fn queue(&mut self, request: WireRequest) -> Result<(), String> {
        self.outgoing = Arc::new(encode(&request)?);
        self.written = 0;
        Ok(())
    }

    fn expired(&self, now: Instant) -> Option<String> {
        if !self.ready && now >= self.startup_deadline {
            return Some("the page did not finish loading within its startup deadline".to_owned());
        }
        self.operations
            .iter()
            .find(|(_, deadline)| now >= *deadline)
            .map(|(operation, _)| format!("the page exceeded its {operation:?} deadline"))
    }

    async fn pump(&mut self) -> Result<Vec<WorkerFrame>, String> {
        std::future::poll_fn(|cx| self.poll_pump(cx)).await
    }

    fn poll_pump(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<Vec<WorkerFrame>, String>> {
        use std::future::Future;
        use std::task::Poll;
        {
            // Child::wait registers the process exit descriptor. Cancellation
            // retains its cached exit status and is safe between wakeups.
            let mut exited = std::pin::pin!(self.child.wait());
            if let Poll::Ready(status) = exited.as_mut().poll(cx) {
                return Poll::Ready(Err(format!("the page worker exited: {status:?}")));
            }
        }
        if self.written < self.outgoing.len() {
            match self.wire.poll_write(cx, &self.outgoing[self.written..]) {
                Poll::Ready(Ok(0)) => {
                    return Poll::Ready(Err("the page worker stopped reading requests".to_owned()));
                }
                Poll::Ready(Ok(length)) => self.written += length,
                Poll::Pending => {}
                Poll::Ready(Err(error)) => {
                    return Poll::Ready(Err(format!(
                        "could not write to the page worker: {error}"
                    )));
                }
            }
            if self.written == self.outgoing.len() {
                self.outgoing = Arc::new(Vec::new());
                self.written = 0;
            }
        }

        self.incoming.reserve(65_536);
        match self.wire.poll_read_buf(cx, &mut self.incoming) {
            Poll::Ready(Ok(0)) => {
                return Poll::Ready(Err("the page worker closed its transport".to_owned()));
            }
            Poll::Ready(Ok(_)) => {}
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(error)) => {
                return Poll::Ready(Err(format!("could not read the page worker: {error}")));
            }
        }
        let mut frames = Vec::new();
        let mut consumed = 0;
        while self.incoming.len() - consumed >= 4 {
            let header = &self.incoming[consumed..consumed + 4];
            let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
            if length > MAX_FRAME_BYTES {
                return Poll::Ready(Err(
                    "the page worker reply exceeded its byte limit".to_owned()
                ));
            }
            if self.incoming.len() - consumed - 4 < length {
                break;
            }
            let end = consumed + 4 + length;
            match serde_json::from_slice(&self.incoming[consumed + 4..end]) {
                Ok(frame) => frames.push(frame),
                Err(error) => {
                    return Poll::Ready(Err(format!("invalid page worker frame: {error}")));
                }
            }
            consumed = end;
        }
        if consumed > 0 {
            let _ = self.incoming.split_to(consumed);
        }
        Poll::Ready(Ok(frames))
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
    retired: Vec<nagoya::JoinHandle<Arc<()>>>,
    wake: Arc<nagoya::sync::Notify>,
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
            let _ = child.start_kill();
            let wake = Arc::clone(&self.wake);
            self.retired.push(nagoya::spawn(async move {
                let _ = child.wait().await;
                wake.notify_one();
                Arc::new(())
            }));
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
                    let budget = match operation {
                        Operation::Load if !session.local => REMOTE_LOAD_DEADLINE,
                        _ => operation.budget(),
                    };
                    session
                        .operations
                        .push((operation, Instant::now() + budget));
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
                let response = *response;
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
            let _ = session.child.start_kill();
        }
        for task in self.retired.drain(..) {
            task.cancel();
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
    let inbox = Arc::new(Mutex::new(VecDeque::<Arc<QueuedRequest>>::new()));
    let bridge_inbox = Arc::clone(&inbox);
    let wake = Arc::new(nagoya::sync::Notify::new());
    let bridge_wake = Arc::clone(&wake);
    let bridge: blitz_control_protocol::server::ControlBridge = Arc::new(move |request| {
        let answer = Once::new();
        let deadline = Instant::now() + REQUEST_DEADLINE;
        match bridge_inbox.lock() {
            Ok(mut inbox) if inbox.len() < max_pending => {
                inbox.push_back(Arc::new(QueuedRequest {
                    request,
                    answer: Arc::clone(&answer),
                    deadline,
                }));
                drop(inbox);
                bridge_wake.notify_one();
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
        wake: Arc::clone(&wake),
        pending: None,
        failure,
        url: Url::parse(target).ok(),
        local_target: local.then(|| target.to_owned()),
        local_origin: None,
        policy,
        events,
        render_revision: 0,
    };
    nagoya::block_on(async {
        loop {
            supervisor.check_deadlines();
            supervisor.retired.retain(|task| !task.is_finished());
            let now = Instant::now();
            let (expired, next, queued_deadline) = {
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
                let deadline = inbox.iter().map(|request| request.deadline).min();
                (expired, next, deadline)
            };
            for answer in expired {
                answer.fill(page_error(
                    "the control request expired while waiting for the page",
                ));
            }
            if let Some(next) = next {
                supervisor.accept(Arc::try_unwrap(next).unwrap_or_else(|_| {
                    panic!("supervisor request ownership was retained after enqueue")
                }));
            }

            let mut deadline = queued_deadline;
            let mut include = |due: Instant| {
                deadline = Some(deadline.map_or(due, |current| current.min(due)));
            };
            if let Some(pending) = &supervisor.pending {
                include(pending.deadline);
            }
            if let Some(session) = &supervisor.session {
                if !session.ready {
                    include(session.startup_deadline);
                }
                for (_, due) in &session.operations {
                    include(*due);
                }
            }
            let frames = {
                use std::future::Future;
                use std::task::Poll;
                let mut changed = Box::pin(wake.notified());
                let mut timer = Box::pin(async {
                    match deadline {
                        Some(due) => {
                            nagoya::sleep(due.saturating_duration_since(Instant::now())).await
                        }
                        None => std::future::pending::<()>().await,
                    }
                });
                let mut progress = Box::pin(async {
                    match &mut supervisor.session {
                        Some(session) => session.pump().await,
                        None => std::future::pending::<Result<Vec<WorkerFrame>, String>>().await,
                    }
                });
                std::future::poll_fn(|cx| {
                    if timer.as_mut().poll(cx).is_ready() || changed.as_mut().poll(cx).is_ready() {
                        return Poll::Ready(None);
                    }
                    progress.as_mut().poll(cx).map(Some)
                })
                .await
            };
            // Deadlines are checked before any End or Reply may clear them.
            supervisor.check_deadlines();
            match frames {
                Some(Ok(frames)) => {
                    for frame in frames {
                        if supervisor.session.is_none() {
                            break;
                        }
                        supervisor.frame(frame);
                    }
                }
                Some(Err(message)) => supervisor.fail(&message),
                None => {}
            }
        }
    })
}
