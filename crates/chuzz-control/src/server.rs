//! The local control socket, ported from AgencyZero's `agent_control_server`.
//!
//! A Unix socket at a `0600` path with a `0600` JSON descriptor beside it
//! carrying the pid, address and protocol version, so a client can find a
//! running browser without being told where to look. Clients speak MCP
//! JSON-RPC over endpoint-libs framed text.
//!
//! Requests arrive on the socket thread and are answered on the UI thread: the
//! DOM is not `Send`, so every command crosses a bridge channel and is executed
//! where the document lives.

use std::fs::{OpenOptions, remove_file};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use blitz_control_protocol::latest::{Flag, Once};
use blitz_control_protocol::{NagoyaStream, framed_json_neutral};
use endpoint_libs::libs::ws::transport::TransportStream;
use endpoint_libs::libs::ws::{MessageStream, WireMessage};
use nagoya::reactor::socket::TcpListener as BoundListener;
use nagoya::reactor::{Addr, Reactor, TcpListener, TcpStream, block_on_with};
use serde::{Deserialize, Serialize};
use std::os::unix::ffi::OsStrExt;

use crate::{AgentControlRequest, CONTROL_PROTOCOL_VERSION, ControlError, ControlResponse};

/// Published beside the socket so a client can discover a running browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlDescriptor {
    pub protocol_version: u32,
    pub pid: u32,
    pub address: String,
    pub renderer: String,
}

/// Runs a request on the UI thread and resolves when it has been answered.
pub type ControlBridge =
    Arc<dyn Fn(AgentControlRequest) -> Arc<Once<ControlResponse>> + Send + Sync + 'static>;

pub struct ControlServer {
    descriptor_path: PathBuf,
    socket_path: PathBuf,
    shutdown: Option<Arc<Flag>>,
    thread: Option<JoinHandle<()>>,
}

impl ControlServer {
    /// Bind the socket and publish the descriptor. The listener runs on its own
    /// thread so it never blocks the UI.
    pub fn start(bridge: ControlBridge) -> io::Result<Self> {
        let socket_path = runtime_dir().join(format!("chuzz-{}.sock", std::process::id()));
        let descriptor_path = socket_path.with_extension("json");
        if let Some(parent) = socket_path.parent() {
            std::fs::create_dir_all(parent)?;
            // The directory carries the access control, set before anything
            // inside it exists. A socket is created by bind already listening,
            // so a mode applied to it afterwards is always late.
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }
        // A stale socket from a killed process would refuse the bind.
        let _ = remove_file(&socket_path);

        // Bound before `start` returns, so a caller that connects the moment it
        // has the path finds something listening.
        let addr = Addr::path(socket_path.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::other("socket path is not a valid unix address"))?;
        let listener = BoundListener::bind(addr, 128)
            .map_err(|error| std::io::Error::other(format!("bind: {error:?}")))?;
        std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;

        write_descriptor(
            &descriptor_path,
            &ControlDescriptor {
                protocol_version: CONTROL_PROTOCOL_VERSION,
                pid: std::process::id(),
                address: format!("unix://{}", socket_path.display()),
                renderer: "blitz".to_owned(),
            },
        )?;

        let shutdown_tx = Flag::new();
        let shutdown_rx = Arc::clone(&shutdown_tx);
        let thread = thread::Builder::new()
            .name("chuzz-control".to_owned())
            .spawn(move || run(listener, bridge, shutdown_rx))?;

        Ok(Self {
            descriptor_path,
            socket_path,
            shutdown: Some(shutdown_tx),
            thread: Some(thread),
        })
    }

    pub fn socket_path(&self) -> &std::path::Path {
        &self.socket_path
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            shutdown.raise();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        // Leaving either behind would advertise a browser that is gone.
        let _ = remove_file(&self.socket_path);
        let _ = remove_file(&self.descriptor_path);
    }
}

fn runtime_dir() -> PathBuf {
    std::env::var_os("CHUZZ_CONTROL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

fn write_descriptor(path: &std::path::Path, descriptor: &ControlDescriptor) -> io::Result<()> {
    let encoded = serde_json::to_vec_pretty(descriptor)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(&encoded)
}
fn run(listener: BoundListener, bridge: ControlBridge, shutdown: Arc<Flag>) {
    // One reactor, owned by this thread, driving the listener and every
    // connection on it: a nagoya socket only makes progress while its own
    // reactor is polled.
    let Ok(reactor) = Reactor::local() else {
        return;
    };
    let handle = reactor.handle();
    let Ok(listener) = TcpListener::from_listener(listener, &handle) else {
        return;
    };

    block_on_with(&reactor, async move {
        use futures::StreamExt;
        use futures::future::{Either, select};
        use futures::stream::FuturesUnordered;

        // Held and polled in place rather than spawned. nagoya's TaskSet never
        // removes a finished task's entry, so a set fed by unbounded connection
        // churn grows one entry per connection ever accepted.
        let mut connections = FuturesUnordered::new();

        loop {
            let stopping = shutdown.raised();
            let accepted = listener.accept();
            futures::pin_mut!(stopping, accepted);

            let accepted = if connections.is_empty() {
                match select(stopping, accepted).await {
                    Either::Left(_) => break,
                    Either::Right((accepted, _)) => accepted,
                }
            } else {
                let progress = select(accepted, connections.next());
                futures::pin_mut!(progress);
                match select(stopping, progress).await {
                    Either::Left(_) => break,
                    Either::Right((Either::Left((accepted, _)), _)) => accepted,
                    Either::Right((Either::Right(_), _)) => continue,
                }
            };

            let Ok((stream, _)) = accepted else {
                break;
            };
            // Several clients may watch at once; one slow reader must not stall
            // the others.
            connections.push(handle_connection(stream, Arc::clone(&bridge)));
        }
    });
}

async fn handle_connection(stream: TcpStream, bridge: ControlBridge) {
    let mut stream = TransportStream::new(framed_json_neutral(NagoyaStream::new(stream)));
    while let Some(message) = stream.recv().await {
        let response = match message {
            // Ping, pong and close frames are transport bookkeeping, not
            // requests: answering them with a protocol error would be wrong.
            Ok(WireMessage::Text(text)) => match serde_json::from_str::<AgentControlRequest>(&text)
            {
                Ok(request) => bridge(request).recv().await.unwrap_or_else(|| {
                    ControlResponse::Error(ControlError::new(
                        "bridge_closed",
                        "the UI-thread control bridge closed",
                    ))
                }),
                Err(error) => {
                    ControlResponse::Error(ControlError::new("invalid_request", error.to_string()))
                }
            },
            Ok(WireMessage::Close(_)) => break,
            Ok(_) => continue,
            Err(error) => ControlResponse::Error(ControlError::new("transport", error.to_string())),
        };

        let encoded = serde_json::to_string(&response).unwrap_or_else(|error| {
            // The fallback is a literal so it cannot itself fail to encode.
            format!(
                r#"{{"result":"error","value":{{"code":"encode","message":"{}"}}}}"#,
                error.to_string().replace('"', "'")
            )
        });
        if stream
            .send(WireMessage::Text(encoded.into()))
            .await
            .is_err()
        {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_descriptor_round_trips() {
        let descriptor = ControlDescriptor {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            pid: 4321,
            address: "unix:///tmp/chuzz-4321.sock".to_owned(),
            renderer: "blitz".to_owned(),
        };
        let encoded = serde_json::to_value(&descriptor).unwrap();
        assert_eq!(encoded["protocolVersion"], CONTROL_PROTOCOL_VERSION);
        assert_eq!(encoded["pid"], 4321);
        assert_eq!(
            serde_json::from_value::<ControlDescriptor>(encoded).unwrap(),
            descriptor
        );
    }

    #[test]
    fn a_client_gets_an_answer_and_teardown_removes_both_files() {
        nagoya::block_on(async {
            let dir =
                std::env::temp_dir().join(format!("chuzz-control-test-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            // SAFETY: single-threaded test, set before the server reads it.
            unsafe { std::env::set_var("CHUZZ_CONTROL_DIR", &dir) };

            let bridge: ControlBridge = Arc::new(|_request| {
                let answer = Once::new();
                answer.fill(ControlResponse::Ok);
                answer
            });
            let server = ControlServer::start(bridge).unwrap();
            let socket_path = server.socket_path().to_path_buf();
            let descriptor_path = socket_path.with_extension("json");

            assert!(socket_path.exists(), "socket was not bound");
            assert!(descriptor_path.exists(), "descriptor was not published");

            let mode = std::fs::metadata(&descriptor_path)
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "descriptor must not be world readable");

            let addr = Addr::path(socket_path.as_os_str().as_bytes()).unwrap();
            // Held for the whole test. As a temporary inside the `connect` call
            // it was dropped at the end of that statement, and dropping a
            // `Reactor` stops the thread that drives its sockets: the request
            // went out, the answer was never read, and the test hung for good.
            let client_reactor = Reactor::start().unwrap();
            let stream = TcpStream::connect(addr, &client_reactor.handle())
                .await
                .unwrap();
            let mut client = TransportStream::new(framed_json_neutral(NagoyaStream::new(stream)));
            let request = serde_json::to_string(&AgentControlRequest::Inspect {
                root: None,
                max_depth: 2,
                include_attrs: crate::AttrScope::None,
            })
            .unwrap();
            client
                .send(WireMessage::Text(request.into()))
                .await
                .unwrap();
            let WireMessage::Text(reply) = client.recv().await.unwrap().unwrap() else {
                panic!("expected a text reply");
            };
            assert_eq!(
                serde_json::from_str::<ControlResponse>(&reply).unwrap(),
                ControlResponse::Ok
            );

            drop(client);
            drop(server);
            assert!(!socket_path.exists(), "socket outlived the server");
            assert!(!descriptor_path.exists(), "descriptor outlived the server");
            let _ = std::fs::remove_dir_all(&dir);
        });
    }
}
