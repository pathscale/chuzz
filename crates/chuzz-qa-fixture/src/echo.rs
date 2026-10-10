use std::collections::{BTreeMap, VecDeque};
use std::future::poll_fn;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Poll, Waker};
use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};

const SESSIONS: usize = 4;
const MESSAGES: usize = 128;
const MESSAGE_BYTES: usize = 144 * 1024;
const LIFETIME: Duration = Duration::from_secs(120);
const POLL_WAIT: Duration = Duration::from_secs(2);
const MAX_ID: u64 = 9_007_199_254_740_991;

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|error| error.into_inner())
}

#[derive(Clone, Serialize)]
struct Envelope {
    sequence: u64,
    payload: Value,
}

#[derive(Default)]
struct Mailbox {
    last: u64,
    acknowledged: u64,
    messages: VecDeque<Envelope>,
    reader: Option<Waker>,
}

#[derive(Default)]
struct SessionState {
    closed: bool,
    mailboxes: [Mailbox; 2],
}

struct Session {
    pid: u32,
    child: Mutex<Option<Child>>,
    state: Mutex<SessionState>,
}

impl Session {
    fn send(&self, sender: usize, payload: Value) -> Result<Value, String> {
        if !payload.is_object() || payload.to_string().len() > MESSAGE_BYTES {
            return Err("invalid or oversized signaling message".into());
        }
        let (sequence, reader) = {
            let mut state = lock(&self.state);
            if state.closed {
                return Err("session closed".into());
            }
            let mailbox = &mut state.mailboxes[1 - sender];
            if mailbox.messages.len() >= MESSAGES || mailbox.last == MAX_ID {
                return Err("signaling queue limit reached".into());
            }
            mailbox.last += 1;
            let sequence = mailbox.last;
            mailbox.messages.push_back(Envelope { sequence, payload });
            (sequence, mailbox.reader.take())
        };
        if let Some(reader) = reader {
            reader.wake();
        }
        Ok(json!({"sequence": sequence}))
    }

    async fn receive(&self, side: usize, after: u64) -> Result<Value, String> {
        let receive = poll_fn(|context| {
            let mut state = lock(&self.state);
            if state.closed {
                return Poll::Ready(Err("session closed".to_owned()));
            }
            let mailbox = &mut state.mailboxes[side];
            if after < mailbox.acknowledged || after > mailbox.last {
                return Poll::Ready(Err("invalid signaling cursor".to_owned()));
            }
            mailbox.acknowledged = after;
            while mailbox
                .messages
                .front()
                .is_some_and(|message| message.sequence <= after)
            {
                mailbox.messages.pop_front();
            }
            if !mailbox.messages.is_empty() {
                let messages: Vec<_> = mailbox.messages.iter().take(16).cloned().collect();
                return Poll::Ready(Ok(json!({"messages": messages})));
            }
            mailbox.reader = Some(context.waker().clone());
            Poll::Pending
        });
        match nagoya::timeout(POLL_WAIT, receive).await {
            Ok(result) => result,
            Err(_) => Ok(json!({"messages": []})),
        }
    }

    fn close(&self) -> Result<Value, String> {
        let readers = {
            let mut state = lock(&self.state);
            state.closed = true;
            state
                .mailboxes
                .iter_mut()
                .filter_map(|mailbox| {
                    mailbox.messages.clear();
                    mailbox.reader.take()
                })
                .collect::<Vec<_>>()
        };
        for reader in readers {
            reader.wake();
        }

        let mut child = lock(&self.child);
        if let Some(process) = child.as_mut() {
            if process.try_wait().map_err(|error| error.to_string())?.is_none() {
                process.kill().map_err(|error| error.to_string())?;
                process.wait().map_err(|error| error.to_string())?;
            }
            child.take();
        }
        Ok(json!({
            "pid": self.pid,
            "sessionClosed": true,
            "reaped": true,
            "messages": 0,
        }))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let child = self
            .child
            .get_mut()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(process) = child.as_mut() {
            let _ = process.kill();
            let _ = process.wait();
        }
    }
}

#[derive(Default)]
struct State {
    next: u64,
    sessions: BTreeMap<u64, Arc<Session>>,
}

pub struct Registry {
    program: Option<PathBuf>,
    port: u16,
    state: Mutex<State>,
}

impl Registry {
    pub fn new(program: Option<PathBuf>, port: u16) -> Arc<Self> {
        Arc::new(Self {
            program,
            port,
            state: Mutex::new(State::default()),
        })
    }

    fn create(self: &Arc<Self>) -> Result<Value, String> {
        let program = self
            .program
            .as_ref()
            .ok_or("enable echo sessions with --echo-host ABSOLUTE_PATH")?;
        let mut state = lock(&self.state);
        if state.sessions.len() >= SESSIONS || state.next == MAX_ID {
            return Err("echo session limit reached".into());
        }
        state.next += 1;
        let id = state.next;
        let target = format!(
            "http://127.0.0.1:{}/echo/helper.html?session={id}",
            self.port,
        );
        let child = Command::new(program)
            .arg(target)
            .env_remove("QA_INSPECT_PAGE")
            .env_remove("CHUZZ_HEADLESS_MODE")
            .env_remove("CHUZZ_HEADLESS_PAGE_WORKER")
            .env_remove("CHUZZ_HEADLESS_WORKER_ROUTE")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| format!("could not launch echo helper: {error}"))?;
        let pid = child.id();
        state.sessions.insert(
            id,
            Arc::new(Session {
                pid,
                child: Mutex::new(Some(child)),
                state: Mutex::new(SessionState::default()),
            }),
        );
        drop(state);

        let registry = Arc::downgrade(self);
        let _ = nagoya::spawn(async move {
            nagoya::sleep(LIFETIME).await;
            if let Some(registry) = registry.upgrade()
                && let Err(error) = registry.finish(id)
            {
                eprintln!("echo session expiry failed: {error}");
            }
        });
        Ok(json!({"session": id, "pid": pid, "lifetimeMs": 120000}))
    }

    fn finish(&self, id: u64) -> Result<Value, String> {
        let session = {
            let mut state = lock(&self.state);
            state.sessions.remove(&id)
        };
        match session {
            Some(session) => session.close(),
            None => Ok(json!({"sessionClosed": true, "reaped": true, "messages": 0})),
        }
    }

    pub async fn route(
        self: &Arc<Self>,
        method: &str,
        target: &str,
        body: &[u8],
    ) -> (&'static str, Value) {
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        if path == "/api/echo" {
            if method != "POST" {
                return failure("405 Method Not Allowed", "POST required");
            }
            if self.program.is_none() {
                return failure(
                    "503 Service Unavailable",
                    "enable echo sessions with --echo-host ABSOLUTE_PATH",
                );
            }
            return match self.create() {
                Ok(value) => ("201 Created", value),
                Err(error) => failure("500 Internal Server Error", &error),
            };
        }

        let Some(relative) = path.strip_prefix("/api/echo/") else {
            return failure("404 Not Found", "unknown echo route");
        };
        let mut fields = relative.split('/');
        let id = fields.next().and_then(|value| value.parse::<u64>().ok());
        let side = fields.next();
        if fields.next().is_some() {
            return failure("404 Not Found", "unknown echo route");
        }
        let Some(id) = id else {
            return failure("400 Bad Request", "invalid session identifier");
        };

        if method == "DELETE" && side.is_none() {
            return match self.finish(id) {
                Ok(value) => ("200 OK", value),
                Err(error) => failure("500 Internal Server Error", &error),
            };
        }

        let side = match side {
            Some("page") => 0,
            Some("helper") => 1,
            _ => return failure("404 Not Found", "unknown echo mailbox"),
        };
        let session = lock(&self.state).sessions.get(&id).cloned();
        let Some(session) = session else {
            return failure("410 Gone", "session closed");
        };

        match method {
            "POST" => {
                let payload = match serde_json::from_slice(body) {
                    Ok(payload) => payload,
                    Err(_) => return failure("400 Bad Request", "invalid signaling JSON"),
                };
                match session.send(side, payload) {
                    Ok(value) => ("200 OK", value),
                    Err(error) => failure("409 Conflict", &error),
                }
            }
            "GET" => {
                let mut cursor = None;
                for (name, value) in url::form_urlencoded::parse(query.as_bytes()) {
                    if name != "after" || cursor.is_some() {
                        return failure("400 Bad Request", "one after cursor is required");
                    }
                    cursor = value.parse::<u64>().ok();
                    if cursor.is_none() {
                        return failure("400 Bad Request", "invalid signaling cursor");
                    }
                }
                let Some(cursor) = cursor else {
                    return failure("400 Bad Request", "one after cursor is required");
                };
                match session.receive(side, cursor).await {
                    Ok(value) => ("200 OK", value),
                    Err(error) if error == "session closed" => failure("410 Gone", &error),
                    Err(error) => failure("409 Conflict", &error),
                }
            }
            _ => failure("405 Method Not Allowed", "GET, POST or DELETE required"),
        }
    }
}

fn failure(status: &'static str, message: &str) -> (&'static str, Value) {
    (status, json!({"error": message}))
}

