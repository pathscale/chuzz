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

use endpoint_libs::libs::ws::transport::{TransportStream, framed_json};
use endpoint_libs::libs::ws::{MessageStream, WireMessage};
use serde::{Deserialize, Serialize};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::oneshot;

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
    Arc<dyn Fn(AgentControlRequest) -> oneshot::Receiver<ControlResponse> + Send + Sync + 'static>;

pub struct ControlServer {
    descriptor_path: PathBuf,
    socket_path: PathBuf,
    shutdown: Option<oneshot::Sender<()>>,
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
        }
        // A stale socket from a killed process would refuse the bind.
        let _ = remove_file(&socket_path);

        let listener = std::os::unix::net::UnixListener::bind(&socket_path)?;
        listener.set_nonblocking(true)?;
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

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
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
            let _ = shutdown.send(());
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

fn run(
    listener: std::os::unix::net::UnixListener,
    bridge: ControlBridge,
    shutdown: oneshot::Receiver<()>,
) {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()
    else {
        return;
    };
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, async move {
        let Ok(listener) = UnixListener::from_std(listener) else {
            return;
        };
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                _ = &mut shutdown => break,
                accepted = listener.accept() => match accepted {
                    Ok((stream, _)) => {
                        let bridge = Arc::clone(&bridge);
                        // Several clients may watch at once; one slow reader
                        // must not stall the others.
                        tokio::task::spawn_local(handle_connection(stream, bridge));
                    }
                    Err(_) => break,
                }
            }
        }
    });
}

async fn handle_connection(stream: UnixStream, bridge: ControlBridge) {
    let mut stream = TransportStream::new(framed_json(stream));
    while let Some(message) = stream.recv().await {
        let response = match message {
            // Ping, pong and close frames are transport bookkeeping, not
            // requests: answering them with a protocol error would be wrong.
            Ok(WireMessage::Text(text)) => match serde_json::from_str::<AgentControlRequest>(&text)
            {
                Ok(request) => bridge(request).await.unwrap_or_else(|_| {
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
        if stream.send(WireMessage::Text(encoded)).await.is_err() {
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

    #[tokio::test]
    async fn a_client_gets_an_answer_and_teardown_removes_both_files() {
        let dir = std::env::temp_dir().join(format!("chuzz-control-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: single-threaded test, set before the server reads it.
        unsafe { std::env::set_var("CHUZZ_CONTROL_DIR", &dir) };

        let bridge: ControlBridge = Arc::new(|_request| {
            let (tx, rx) = oneshot::channel();
            let _ = tx.send(ControlResponse::Ok);
            rx
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

        let stream = UnixStream::connect(&socket_path).await.unwrap();
        let mut client = TransportStream::new(framed_json(stream));
        let request = serde_json::to_string(&AgentControlRequest::Inspect {
            root: None,
            max_depth: 2,
            include_attrs: crate::AttrScope::None,
        })
        .unwrap();
        client.send(WireMessage::Text(request)).await.unwrap();
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
    }
}
