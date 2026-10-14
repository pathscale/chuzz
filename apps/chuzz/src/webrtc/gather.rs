//! Host-owned UDP gathering. No wildcard address is advertised.
//!
//! Loopback is always available to same-worker fixtures. Additional concrete
//! interface addresses are host policy, supplied through CHUZZ_WEBRTC_HOSTS.
//! The initial STUN surface accepts numeric addresses and uses bounded binding
//! transactions. DNS and TURN are explicitly unavailable in this slice.

use std::future::{Future, poll_fn};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::OnceLock;
use std::task::Poll;
use std::time::{Duration, Instant};

use nagoya::net::UdpSocket;
use nagoya::reactor::{Addr, Reactor};
use serde_json::Value;
use str0m::Candidate;

use super::host::Peer;

static REACTOR: OnceLock<Result<Reactor, String>> = OnceLock::new();

pub(super) struct Config {
    hosts: Vec<IpAddr>,
    servers: Vec<SocketAddr>,
}

impl Config {
    pub(super) fn parse(value: &Value) -> Result<Self, String> {
        if value
            .get("iceTransportPolicy")
            .and_then(Value::as_str)
            .is_some_and(|value| value != "all")
        {
            return Err("NotSupportedError: relay-only ICE is unavailable".into());
        }
        if value
            .get("iceCandidatePoolSize")
            .and_then(Value::as_u64)
            .is_some_and(|value| value != 0)
        {
            return Err("NotSupportedError: candidate pooling is unavailable".into());
        }
        let mut hosts: Vec<IpAddr> = vec![Ipv4Addr::LOCALHOST.into(), Ipv6Addr::LOCALHOST.into()];
        if let Ok(configured) = std::env::var("CHUZZ_WEBRTC_HOSTS") {
            for value in configured
                .split(',')
                .filter(|value| !value.trim().is_empty())
            {
                let ip: IpAddr = value
                    .trim()
                    .parse()
                    .map_err(|_| "OperationError: invalid CHUZZ_WEBRTC_HOSTS address")?;
                if ip.is_unspecified() || ip.is_multicast() {
                    return Err(
                        "OperationError: WebRTC host addresses must be concrete unicast IPs".into(),
                    );
                }
                if !hosts.contains(&ip) {
                    hosts.push(ip);
                }
            }
        }
        if hosts.len() > 8 {
            return Err("OperationError: at most eight WebRTC host addresses are supported".into());
        }

        let mut servers = Vec::new();
        if let Some(entries) = value.get("iceServers") {
            let entries = entries
                .as_array()
                .ok_or("TypeError: iceServers must be a sequence")?;
            for entry in entries {
                let urls = entry
                    .get("urls")
                    .ok_or("TypeError: an ICE server requires urls")?;
                let urls: Vec<&str> = if let Some(url) = urls.as_str() {
                    vec![url]
                } else {
                    urls.as_array()
                        .ok_or("TypeError: ICE server urls must be strings")?
                        .iter()
                        .map(|url| {
                            url.as_str()
                                .ok_or("TypeError: ICE server urls must be strings")
                        })
                        .collect::<Result<_, _>>()?
                };
                for url in urls {
                    let address = url
                        .strip_prefix("stun:")
                        .ok_or("NotSupportedError: only numeric stun: UDP servers are supported")?;
                    let address = address.strip_suffix("?transport=udp").unwrap_or(address);
                    let address = address
                        .parse::<SocketAddr>()
                        .or_else(|_| {
                            address
                                .parse::<IpAddr>()
                                .map(|ip| SocketAddr::new(ip, 3478))
                        })
                        .map_err(
                            |_| "NotSupportedError: STUN DNS names require a later resolver lane",
                        )?;
                    if address.ip().is_unspecified()
                        || address.ip().is_multicast()
                        || address.port() == 0
                    {
                        return Err("TypeError: invalid STUN server address".into());
                    }
                    if !servers.contains(&address) {
                        servers.push(address);
                    }
                }
            }
        }
        if servers.len() > 4 {
            return Err("OperationError: at most four STUN endpoints are supported".into());
        }
        Ok(Self { hosts, servers })
    }
}

pub(super) fn address(value: SocketAddr) -> Addr {
    match value {
        SocketAddr::V4(value) => Addr::V4(value.ip().octets(), value.port()),
        SocketAddr::V6(value) => Addr::V6(value.ip().octets(), value.port()),
    }
}

pub(super) fn socket_address(value: Addr) -> Result<SocketAddr, String> {
    match value {
        Addr::V4(ip, port) => Ok(SocketAddr::new(Ipv4Addr::from(ip).into(), port)),
        Addr::V6(ip, port) => Ok(SocketAddr::new(Ipv6Addr::from(ip).into(), port)),
        Addr::V6Scoped(ip, port, scope_id) => Ok(SocketAddr::V6(std::net::SocketAddrV6::new(
            Ipv6Addr::from(ip),
            port,
            0,
            scope_id,
        ))),
        Addr::Path(_) => Err("OperationError: expected an IP datagram address".into()),
    }
}

pub(super) struct Socket {
    pub socket: UdpSocket,
    pub local: SocketAddr,
}

fn mapped(bytes: &[u8], transaction: &[u8; 12]) -> Option<SocketAddr> {
    if bytes.len() < 20
        || bytes[..2] != [1, 1]
        || bytes[4..8] != [0x21, 0x12, 0xa4, 0x42]
        || bytes[8..20] != transaction[..]
    {
        return None;
    }
    let length = usize::from(u16::from_be_bytes([bytes[2], bytes[3]]));
    if length % 4 != 0 || length + 20 != bytes.len() {
        return None;
    }
    let mut cursor = 20;
    while cursor + 4 <= bytes.len() {
        let kind = u16::from_be_bytes([bytes[cursor], bytes[cursor + 1]]);
        let length = usize::from(u16::from_be_bytes([bytes[cursor + 2], bytes[cursor + 3]]));
        cursor += 4;
        let value = bytes.get(cursor..cursor.checked_add(length)?)?;
        if kind == 0x0020 && value.len() >= 4 && value[0] == 0 {
            let port = u16::from_be_bytes([value[2], value[3]]) ^ 0x2112;
            let cookie = [0x21, 0x12, 0xa4, 0x42];
            let ip = match (value[1], value.len()) {
                (1, 8) => {
                    let mut ip = [0; 4];
                    for index in 0..4 {
                        ip[index] = value[index + 4] ^ cookie[index];
                    }
                    IpAddr::V4(ip.into())
                }
                (2, 20) => {
                    let mut ip = [0; 16];
                    for index in 0..16 {
                        let mask = if index < 4 {
                            cookie[index]
                        } else {
                            transaction[index - 4]
                        };
                        ip[index] = value[index + 4] ^ mask;
                    }
                    IpAddr::V6(ip.into())
                }
                _ => return None,
            };
            if port != 0 && !ip.is_unspecified() && !ip.is_multicast() {
                return Some(SocketAddr::new(ip, port));
            }
        }
        cursor = cursor.checked_add((length + 3) & !3)?;
    }
    None
}

async fn binding(
    socket: &Socket,
    server: SocketAddr,
    transaction: [u8; 12],
    peer: &Peer,
    limit: Instant,
) -> Option<SocketAddr> {
    let mut request = [0; 20];
    request[1] = 1;
    request[4..8].copy_from_slice(&[0x21, 0x12, 0xa4, 0x42]);
    request[8..].copy_from_slice(&transaction);
    let mut buffer = [0; 2048];

    for attempt in 0..3 {
        if Instant::now() >= limit {
            return None;
        }
        let sent = poll_fn(|context| {
            if !peer.register(context.waker()) {
                return Poll::Ready(None);
            }
            socket
                .socket
                .poll_send_to(context, &request, address(server))
                .map(Some)
        })
        .await?;
        if sent.ok()? != request.len() {
            return None;
        }
        let deadline = (Instant::now() + Duration::from_millis(100 << attempt)).min(limit);
        let mut sleep = Box::pin(nagoya::sleep(
            deadline.saturating_duration_since(Instant::now()),
        ));
        loop {
            let received = poll_fn(|context| {
                if !peer.register(context.waker()) {
                    return Poll::Ready(None);
                }
                if sleep.as_mut().poll(context).is_ready() {
                    return Poll::Ready(None);
                }
                socket.socket.poll_recv_from(context, &mut buffer).map(Some)
            })
            .await;
            let Some(Ok((length, source))) = received else {
                break;
            };
            if socket_address(source).ok() == Some(server)
                && let Some(mapped) = mapped(&buffer[..length], &transaction)
            {
                return Some(mapped);
            }
        }
    }
    None
}

pub(super) async fn gather(
    config: Config,
    peer: &Peer,
    certificate: &[u8],
) -> Result<(Vec<Socket>, Vec<Candidate>), String> {
    let reactor = REACTOR
        .get_or_init(|| Reactor::start().map_err(|error| error.to_string()))
        .as_ref()
        .map_err(Clone::clone)?;
    let handle = reactor.handle();
    let mut sockets = Vec::new();
    let mut candidates = Vec::new();

    for ip in config.hosts {
        if peer.closed() {
            return Err("AbortError: peer closed while gathering".into());
        }
        let socket = match UdpSocket::bind(address(SocketAddr::new(ip, 0)), &handle) {
            Ok(socket) => socket,
            Err(_) if ip == IpAddr::V6(Ipv6Addr::LOCALHOST) => continue,
            Err(error) => return Err(format!("OperationError: UDP bind failed: {error}")),
        };
        let local = socket_address(socket.local_addr().map_err(|error| error.to_string())?)?;
        candidates.push(Candidate::host(local, "udp").map_err(|error| error.to_string())?);
        sockets.push(Socket { socket, local });
        peer.metrics(|metrics| metrics.sockets = sockets.len());
    }

    let limit = Instant::now() + Duration::from_secs(2);
    let provider = super::crypto::provider();
    for socket in &sockets {
        for server in &config.servers {
            if socket.local.is_ipv4() != server.is_ipv4() || Instant::now() >= limit {
                continue;
            }
            let mut seed = certificate.to_vec();
            seed.extend_from_slice(format!("{}:{}:{server}", peer.id, socket.local).as_bytes());
            let digest = provider.sha256_provider.sha256(&seed);
            let mut transaction = [0; 12];
            transaction.copy_from_slice(&digest[..12]);
            if let Some(mapped) = binding(socket, *server, transaction, peer, limit).await {
                let value = format!(
                    "candidate:w07{} 1 udp 1694498815 {} {} typ srflx raddr {} rport {}",
                    candidates.len(),
                    mapped.ip(),
                    mapped.port(),
                    socket.local.ip(),
                    socket.local.port()
                );
                candidates
                    .push(Candidate::from_sdp_string(&value).map_err(|error| error.to_string())?);
            }
        }
    }
    if peer.closed() {
        return Err("AbortError: peer closed while gathering".into());
    }
    Ok((sockets, candidates))
}
