//! Private CONNECT forwarding over Nagoya readiness.

use std::future::poll_fn;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::task::{Context, Poll};
use std::time::Duration;

use nagoya::net::TcpStream;
use nagoya::reactor::socket::{TcpListener, TcpSocket};
use nagoya::reactor::{Addr, Handle, Interest, Reactor, Registration};

use super::{
    HeadlessPolicy, MAX_PROXY_CONNECTIONS, MAX_PROXY_HEADER_BYTES, PROXY_ENVIRONMENT,
    proxy_connect_url, reserve_proxy_bytes,
};

struct Socket {
    stream: TcpStream,
    raw: i32,
}

impl Socket {
    fn adopt(socket: TcpSocket, handle: &Handle) -> nagoya::reactor::Result<Self> {
        let raw = socket.raw();
        Ok(Self {
            stream: TcpStream::from_socket(socket, handle)?,
            raw,
        })
    }

    async fn connect(address: SocketAddr, handle: &Handle) -> nagoya::reactor::Result<Self> {
        let address = match address {
            SocketAddr::V4(address) => Addr::V4(address.ip().octets(), address.port()),
            SocketAddr::V6(address) => {
                Addr::V6Scoped(address.ip().octets(), address.port(), address.scope_id())
            }
        };
        let socket = TcpSocket::connect(address)?;
        let registration = handle.register(socket.raw(), Interest::WRITABLE)?;
        poll_fn(|cx| {
            match socket.connect_error() {
                Err(error) => return Poll::Ready(Err(error)),
                Ok(()) if socket.peer_addr().is_ok() => return Poll::Ready(Ok(())),
                Ok(()) => {}
            }
            registration.poll_writable(cx.waker());
            Poll::Pending
        })
        .await?;
        drop(registration);
        Self::adopt(socket, handle)
    }

    fn close_write(&self) {
        // SAFETY: stream owns this descriptor throughout the call.
        unsafe {
            libc::shutdown(self.raw, libc::SHUT_WR);
        }
    }
}

struct Listener {
    registration: Registration,
    socket: TcpListener,
    handle: Handle,
}

impl Listener {
    async fn accept(&self) -> nagoya::reactor::Result<Socket> {
        poll_fn(|cx| {
            loop {
                if !self.registration.take_readable_or_park(cx.waker()) {
                    return Poll::Pending;
                }
                match self.socket.accept() {
                    Ok((socket, _)) => {
                        self.registration.mark_readable();
                        return Poll::Ready(Socket::adopt(socket, &self.handle));
                    }
                    Err(error) if error.would_block() => {}
                    Err(error)
                        if error.interrupted()
                            || nagoya::reactor::error::transient_accept(error) =>
                    {
                        self.registration.mark_readable();
                    }
                    Err(error) => return Poll::Ready(Err(error)),
                }
            }
        })
        .await
    }
}

struct ProxyWork {
    client: std::sync::Mutex<Option<Socket>>,
    policy: Arc<HeadlessPolicy>,
    transferred: Arc<AtomicUsize>,
    handle: Handle,
}

fn enqueue(work: Arc<ProxyWork>) -> nagoya::JoinHandle<Arc<ProxyWork>> {
    nagoya::spawn(async move {
        let client = work
            .client
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(client) = client {
            handle_connection(client, &work).await;
        }
        work
    })
}

pub(super) struct EgressProxy {
    task: Option<nagoya::JoinHandle<Arc<()>>>,
    _reactor: Arc<Reactor>,
    original_environment: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl EgressProxy {
    pub(super) fn start(policy: HeadlessPolicy) -> Result<Self, String> {
        let socket = TcpListener::bind(Addr::localhost(0), 128)
            .map_err(|error| format!("could not bind the private egress filter: {error}"))?;
        let address = format!(
            "http://127.0.0.1:{}",
            socket
                .local_addr()
                .map_err(|error| error.to_string())?
                .port()
        );
        let original_environment: Vec<(&'static str, Option<std::ffi::OsString>)> =
            PROXY_ENVIRONMENT
                .iter()
                .map(|name| (*name, std::env::var_os(name)))
                .collect();
        unsafe {
            for name in &PROXY_ENVIRONMENT[..6] {
                std::env::set_var(name, &address);
            }
            for name in &PROXY_ENVIRONMENT[6..] {
                std::env::set_var(name, "");
            }
        }
        let reactor = match Reactor::start() {
            Ok(reactor) => Arc::new(reactor),
            Err(error) => {
                restore_environment(&original_environment);
                return Err(format!(
                    "could not start the private egress filter: {error}"
                ));
            }
        };
        let handle = reactor.handle();
        let registration = match handle.register(socket.raw(), Interest::READABLE) {
            Ok(registration) => registration,
            Err(error) => {
                restore_environment(&original_environment);
                return Err(format!(
                    "could not register the private egress filter: {error}"
                ));
            }
        };
        let listener = Listener {
            registration,
            socket,
            handle: handle.clone(),
        };
        let policy = Arc::new(policy);
        let transferred = Arc::new(AtomicUsize::new(0));
        let task_reactor = Arc::clone(&reactor);
        let task = nagoya::spawn(async move {
            let _reactor = task_reactor;
            let mut connections: Vec<nagoya::JoinHandle<Arc<ProxyWork>>> = Vec::new();
            loop {
                connections.retain(|task| !task.is_finished());
                let Ok(mut client) = listener.accept().await else {
                    break;
                };
                connections.retain(|task| !task.is_finished());
                if connections.len() >= MAX_PROXY_CONNECTIONS {
                    // A saturated peer cannot hold the accept task in its send.
                    let refusal = refuse(&mut client, "503 Service Unavailable");
                    let _ = nagoya::timeout(Duration::from_secs(2), refusal).await;
                    continue;
                }
                connections.push(enqueue(Arc::new(ProxyWork {
                    client: std::sync::Mutex::new(Some(client)),
                    policy: Arc::clone(&policy),
                    transferred: Arc::clone(&transferred),
                    handle: handle.clone(),
                })));
            }
            for task in connections {
                task.cancel();
            }
            Arc::new(())
        });
        Ok(Self {
            task: Some(task),
            _reactor: reactor,
            original_environment,
        })
    }
}

impl Drop for EgressProxy {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.cancel();
        }
        restore_environment(&self.original_environment);
    }
}

fn restore_environment(originals: &[(&'static str, Option<std::ffi::OsString>)]) {
    unsafe {
        for (name, value) in originals {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

async fn refuse(client: &mut Socket, status: &str) {
    let response = format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    let _ = client.stream.write_all(response.as_bytes()).await;
}

async fn handle_connection(mut client: Socket, work: &ProxyWork) {
    let Some(head) = nagoya::timeout(Duration::from_secs(2), read_head(&mut client))
        .await
        .ok()
        .flatten()
    else {
        return;
    };
    let Some(url) = proxy_connect_url(&head) else {
        let _ = nagoya::timeout(Duration::from_secs(2), refuse(&mut client, "403 Forbidden")).await;
        return;
    };
    // DNS runs on the task pool, never on the reactor driver.
    let Ok(addresses) = work.policy.resolve_public_addresses(&url) else {
        let _ = nagoya::timeout(Duration::from_secs(2), refuse(&mut client, "403 Forbidden")).await;
        return;
    };
    let mut upstream = None;
    for address in addresses {
        if let Ok(Ok(socket)) = nagoya::timeout(
            Duration::from_secs(3),
            Socket::connect(address, &work.handle),
        )
        .await
        {
            upstream = Some(socket);
            break;
        }
    }
    let Some(upstream) = upstream else {
        let _ = nagoya::timeout(
            Duration::from_secs(2),
            refuse(&mut client, "502 Bad Gateway"),
        )
        .await;
        return;
    };
    if !matches!(
        nagoya::timeout(
            Duration::from_secs(2),
            client
                .stream
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        )
        .await,
        Ok(Ok(()))
    ) {
        return;
    }
    tunnel(client, upstream, &work.transferred).await;
}

async fn read_head(client: &mut Socket) -> Option<String> {
    let mut head = Vec::with_capacity(1024);
    let mut byte = [0_u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() >= MAX_PROXY_HEADER_BYTES || client.stream.read(&mut byte).await.ok()? == 0 {
            return None;
        }
        head.push(byte[0]);
    }
    String::from_utf8(head).ok()
}

struct Relay {
    buffer: Arc<[u8; 8192]>,
    length: usize,
    written: usize,
    ended: bool,
    exhausted: bool,
}

impl Relay {
    fn new() -> Self {
        Self {
            buffer: Arc::new([0; 8192]),
            length: 0,
            written: 0,
            ended: false,
            exhausted: false,
        }
    }

    fn step(
        &mut self,
        reader: &mut Socket,
        writer: &mut Socket,
        bytes: &AtomicUsize,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), ()>> {
        if self.ended {
            return Poll::Ready(Ok(()));
        }
        for _ in 0..64 {
            if self.written < self.length {
                match writer
                    .stream
                    .poll_write(cx, &self.buffer[self.written..self.length])
                {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Ok(0) | Err(_)) => return Poll::Ready(Err(())),
                    Poll::Ready(Ok(written)) => self.written += written,
                }
                if self.written < self.length {
                    continue;
                }
            }
            if self.exhausted {
                return Poll::Ready(Err(()));
            }
            let buffer = Arc::get_mut(&mut self.buffer)
                .expect("the completed write returns its Arc buffer to the reader");
            match reader.stream.poll_read(cx, buffer) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(_)) => return Poll::Ready(Err(())),
                Poll::Ready(Ok(0)) => {
                    writer.close_write();
                    self.ended = true;
                    return Poll::Ready(Ok(()));
                }
                Poll::Ready(Ok(read)) => {
                    self.length = reserve_proxy_bytes(bytes, read);
                    self.written = 0;
                    self.exhausted = self.length < read;
                }
            }
        }
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

async fn tunnel(mut client: Socket, mut upstream: Socket, bytes: &AtomicUsize) {
    let mut sending = Relay::new();
    let mut receiving = Relay::new();
    poll_fn(|cx| {
        let sent = sending.step(&mut client, &mut upstream, bytes, cx);
        let received = receiving.step(&mut upstream, &mut client, bytes, cx);
        if matches!(sent, Poll::Ready(Err(())))
            || matches!(received, Poll::Ready(Err(())))
            || (sending.ended && receiving.ended)
        {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await;
}
