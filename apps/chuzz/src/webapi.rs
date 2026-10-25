//! Process-local web API state. Only owned Rust data crosses realm boundaries.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use blitz_traits::net::Url;
use serde::Deserialize;
use serde_json::Value as Json;

#[path = "webapi_host.rs"]
pub mod native_host;
#[path = "storage.rs"]
mod storage;

const LIMIT: usize = 64 * 1024 * 1024;
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static PROCESS: LazyLock<Mutex<Process>> = LazyLock::new(Mutex::default);

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|poison| poison.into_inner())
}

#[derive(Default)]
struct Process {
    channels: HashMap<u64, Channel>,
    ports: storage::PortState,
    leases: Vec<Lease>,
}

struct Channel {
    owner: u64,
    origin: Arc<str>,
    name: String,
    incoming: VecDeque<Arc<str>>,
}

struct Lease {
    id: u64,
    owner: u64,
    origin: Arc<str>,
    name: String,
    shared: bool,
    held: bool,
    stolen: bool,
    released: bool,
}

impl Process {
    fn grant(&mut self) {
        for index in 0..self.leases.len() {
            if self.leases[index].held || self.leases[index].stolen || self.leases[index].released {
                continue;
            }
            let candidate = &self.leases[index];
            let blocked = self.leases.iter().enumerate().any(|(other_index, other)| {
                other_index != index
                    && other.origin == candidate.origin
                    && other.name == candidate.name
                    && !other.stolen
                    && !other.released
                    && ((!other.held && other_index < index)
                        || (other.held && (!other.shared || !candidate.shared)))
            });
            if !blocked {
                self.leases[index].held = true;
            }
        }
    }
}

struct Realm {
    id: u64,
    origin: Arc<str>,
    public_origin: String,
    secure: bool,
    notifications: Arc<crate::webauthn::Notifications>,
}

impl Drop for Realm {
    fn drop(&mut self) {
        self.notifications.close();
        crate::webauthn::release_realm(self.id);
        let mut process = lock(&PROCESS);
        process
            .channels
            .retain(|_, channel| channel.owner != self.id);
        process.ports.close_realm(self.id);
        unbind_platform(self.id);
        process.leases.retain(|lease| lease.owner != self.id);
        process.grant();
        let ready = process
            .leases
            .iter()
            .filter(|lease| lease.held)
            .map(|lease| lease.owner)
            .collect::<std::collections::HashSet<_>>();
        drop(process);
        for owner in ready {
            storage::mark_platform_ready(owner);
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
enum Operation {
    Info,
    Random,
    BroadcastOpen {
        name: String,
    },
    BroadcastClose {
        id: u64,
    },
    BroadcastPost {
        id: u64,
        wire: String,
    },
    BroadcastRead,
    PlatformBind,
    PortPair,
    PortPost {
        id: u64,
        wire: String,
    },
    PortStart {
        id: u64,
    },
    PortClose {
        id: u64,
    },
    PortRead,
    LockRequest {
        name: String,
        shared: bool,
        available: bool,
        #[serde(default)]
        steal: bool,
    },
    LockState {
        id: u64,
    },
    LockStatus {
        id: u64,
    },
    LockRelease {
        id: u64,
    },
    LockCancel {
        id: u64,
    },
    LockQuery,
    WebauthnInfo,
    WebauthnCreate {
        options: crate::webauthn::CreationOptions,
    },
    WebauthnGet {
        options: crate::webauthn::RequestOptions,
    },
    WebauthnStatus {
        id: u64,
    },
    WebauthnAbort {
        id: u64,
    },
}

struct InfoReply {
    origin: String,
    secure: bool,
    platform: &'static str,
    ua_platform: &'static str,
    architecture: String,
    bitness: String,
    concurrency: usize,
    version: &'static str,
    languages: Vec<String>,
}

struct BroadcastDelivery {
    id: u64,
    wire: Arc<str>,
}

struct LockEntry {
    name: String,
    mode: &'static str,
    client_id: String,
}

enum Reply {
    Info(InfoReply),
    Random([u8; 16]),
    Id(u64),
    Null,
    BroadcastDeliveries(Vec<BroadcastDelivery>),
    LockQuery {
        held: Vec<LockEntry>,
        pending: Vec<LockEntry>,
    },
    Boolean(bool),
    Webauthn(Json),
}

fn new_realm(url: Option<&Url>, wake: crate::net_bridge::PageWake) -> Arc<Realm> {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let public_origin = url
        .map(|url| url.origin().ascii_serialization())
        .unwrap_or_else(|| "null".to_owned());
    let origin = if public_origin == "null" {
        format!("opaque:{id}")
    } else {
        public_origin.clone()
    };
    let secure = url.is_some_and(|url| {
        matches!(url.scheme(), "https" | "file")
            || url.host_str().is_some_and(|host| {
                host == "localhost"
                    || host.ends_with(".localhost")
                    || host
                        .trim_matches(['[', ']'])
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|address| address.is_loopback())
            })
    });
    Arc::new(Realm {
        id,
        origin: Arc::from(origin),
        public_origin,
        secure,
        notifications: crate::webauthn::Notifications::new(wake),
    })
}

pub(super) fn bind_platform(token: u64, waker: Option<&std::task::Waker>) -> Result<(), String> {
    storage::bind_platform(token, waker)
}

pub(super) fn take_platform_ready(token: u64) -> Result<bool, String> {
    storage::take_platform_ready(token)
}

pub(super) fn unbind_platform(token: u64) {
    storage::unbind_platform(token);
}

fn dispatch(realm: &Realm, operation: Operation) -> Result<Reply, String> {
    match operation {
        Operation::Info => Ok(Reply::Info(InfoReply {
            origin: realm.public_origin.clone(),
            secure: realm.secure,
            platform: if cfg!(target_os = "macos") {
                "MacIntel"
            } else if cfg!(target_os = "windows") {
                "Win32"
            } else {
                "Linux"
            },
            ua_platform: if cfg!(target_os = "macos") {
                "macOS"
            } else if cfg!(target_os = "windows") {
                "Windows"
            } else {
                "Linux"
            },
            architecture: if cfg!(target_arch = "aarch64") {
                "arm".to_owned()
            } else if cfg!(any(target_arch = "x86", target_arch = "x86_64")) {
                "x86".to_owned()
            } else {
                std::env::consts::ARCH.to_owned()
            },
            bitness: usize::BITS.to_string(),
            concurrency: std::thread::available_parallelism().map_or(1, usize::from),
            version: env!("CARGO_PKG_VERSION"),
            languages: crate::languages::preferred_languages().to_vec(),
        })),
        Operation::Random => {
            let provider = rustls::crypto::CryptoProvider::get_default()
                .ok_or_else(|| "secure random provider is unavailable".to_owned())?;
            let mut bytes = [0_u8; 16];
            provider
                .secure_random
                .fill(&mut bytes)
                .map_err(|_| "secure random generation failed".to_owned())?;
            Ok(Reply::Random(bytes))
        }
        Operation::BroadcastOpen { name } => {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            lock(&PROCESS).channels.insert(
                id,
                Channel {
                    owner: realm.id,
                    origin: Arc::clone(&realm.origin),
                    name,
                    incoming: VecDeque::new(),
                },
            );
            Ok(Reply::Id(id))
        }
        Operation::BroadcastClose { id } => {
            let mut process = lock(&PROCESS);
            if process
                .channels
                .get(&id)
                .is_some_and(|channel| channel.owner == realm.id)
            {
                process.channels.remove(&id);
            }
            Ok(Reply::Null)
        }
        Operation::BroadcastPost { id, wire } => {
            if wire.len() > LIMIT {
                return Err("structured clone exceeds the allocation limit".to_owned());
            }
            let (result, ready) = {
                let mut process = lock(&PROCESS);
                let name = process
                    .channels
                    .get(&id)
                    .filter(|channel| channel.owner == realm.id)
                    .map(|channel| channel.name.clone())
                    .ok_or_else(|| "broadcast channel is closed".to_owned())?;
                for (other_id, channel) in &process.channels {
                    if *other_id != id
                        && channel.origin == realm.origin
                        && channel.name == name
                        && (channel.incoming.len() >= 4096
                            || channel
                                .incoming
                                .iter()
                                .map(|wire| wire.len())
                                .sum::<usize>()
                                .saturating_add(wire.len())
                                > LIMIT)
                    {
                        return Err("broadcast queue limit reached".to_owned());
                    }
                }
                let wire: Arc<str> = Arc::from(wire);
                let mut ready = Vec::new();
                for (other_id, channel) in &mut process.channels {
                    if *other_id != id && channel.origin == realm.origin && channel.name == name {
                        channel.incoming.push_back(Arc::clone(&wire));
                        ready.push(channel.owner);
                    }
                }
                (Reply::Null, ready)
            };
            for token in ready {
                storage::mark_platform_ready(token);
            }
            Ok(result)
        }
        Operation::BroadcastRead => {
            let mut process = lock(&PROCESS);
            let mut deliveries = Vec::new();
            for (id, channel) in &mut process.channels {
                if channel.owner == realm.id {
                    while deliveries.len() < 64 {
                        let Some(wire) = channel.incoming.pop_front() else {
                            break;
                        };
                        deliveries.push(BroadcastDelivery { id: *id, wire });
                    }
                    if deliveries.len() == 64 {
                        break;
                    }
                }
            }
            let ready = process
                .channels
                .values()
                .any(|channel| channel.owner == realm.id && !channel.incoming.is_empty());
            drop(process);
            if ready {
                storage::mark_platform_ready(realm.id);
            }
            Ok(Reply::BroadcastDeliveries(deliveries))
        }
        Operation::PlatformBind => {
            storage::bind_platform(realm.id, None)?;
            Ok(Reply::Id(realm.id))
        }
        Operation::PortPair => {
            if lock(&PROCESS).ports.ports.len() >= 4096 {
                return Err("message port limit reached".to_owned());
            }
            let id = NEXT_ID.fetch_add(2, Ordering::Relaxed);
            let mut process = lock(&PROCESS);
            process.ports.ports.insert(
                id,
                storage::Port {
                    owner: realm.id,
                    peer: id + 1,
                    started: false,
                    incoming: VecDeque::new(),
                },
            );
            process.ports.ports.insert(
                id + 1,
                storage::Port {
                    owner: realm.id,
                    peer: id,
                    started: false,
                    incoming: VecDeque::new(),
                },
            );
            Ok(Reply::Id(id))
        }
        Operation::PortPost { id, wire } => {
            if wire.len() > LIMIT {
                return Err("structured clone exceeds the allocation limit".to_owned());
            }
            let owner = realm.id;
            let token = {
                let mut process = lock(&PROCESS);
                let port = process
                    .ports
                    .ports
                    .get(&id)
                    .filter(|port| port.owner == owner)
                    .ok_or_else(|| "message port is closed".to_owned())?;
                let peer_id = port.peer;
                let peer = process
                    .ports
                    .ports
                    .get_mut(&peer_id)
                    .filter(|peer| peer.peer == id)
                    .ok_or_else(|| "message port is disentangled".to_owned())?;
                if peer.incoming.len() >= 4096
                    || peer
                        .incoming
                        .iter()
                        .map(String::len)
                        .sum::<usize>()
                        .saturating_add(wire.len())
                        > LIMIT
                {
                    return Err("message port queue limit reached".to_owned());
                }
                peer.incoming.push_back(wire);
                peer.owner
            };
            storage::mark_platform_ready(token);
            Ok(Reply::Null)
        }
        Operation::PortStart { id } => {
            let ready = {
                let mut process = lock(&PROCESS);
                let port = process
                    .ports
                    .ports
                    .get_mut(&id)
                    .filter(|port| port.owner == realm.id)
                    .ok_or_else(|| "message port is closed".to_owned())?;
                port.started = true;
                !port.incoming.is_empty()
            };
            if ready {
                storage::mark_platform_ready(realm.id);
            }
            Ok(Reply::Null)
        }
        Operation::PortClose { id } => {
            let mut process = lock(&PROCESS);
            if process
                .ports
                .ports
                .get(&id)
                .is_some_and(|port| port.owner == realm.id)
                && let Some(port) = process.ports.ports.remove(&id)
                && let Some(peer) = process.ports.ports.get_mut(&port.peer)
            {
                peer.peer = 0;
            }
            Ok(Reply::Null)
        }
        Operation::PortRead => {
            let mut process = lock(&PROCESS);
            let mut deliveries = Vec::new();
            for (id, port) in &mut process.ports.ports {
                if port.owner == realm.id && port.started {
                    while deliveries.len() < 64 {
                        let Some(wire) = port.incoming.pop_front() else {
                            break;
                        };
                        deliveries.push(BroadcastDelivery {
                            id: *id,
                            wire: Arc::from(wire),
                        });
                    }
                    if deliveries.len() == 64 {
                        break;
                    }
                }
            }
            let ready =
                process.ports.ports.values().any(|port| {
                    port.owner == realm.id && port.started && !port.incoming.is_empty()
                });
            drop(process);
            if ready {
                storage::mark_platform_ready(realm.id);
            }
            Ok(Reply::BroadcastDeliveries(deliveries))
        }
        Operation::LockRequest {
            name,
            shared,
            available,
            steal,
        } => {
            if steal && (shared || available) {
                return Err(
                    "steal requires an exclusive lock and cannot be combined with ifAvailable"
                        .to_owned(),
                );
            }
            let mut process = lock(&PROCESS);
            let blocked = process.leases.iter().any(|lease| {
                !lease.stolen
                    && !lease.released
                    && lease.origin == realm.origin
                    && lease.name == name
                    && ((!lease.held) || (!shared || !lease.shared))
            });
            if available && blocked {
                return Ok(Reply::Null);
            }
            if steal {
                for lease in &mut process.leases {
                    if lease.origin == realm.origin && lease.name == name && lease.held {
                        lease.held = false;
                        lease.stolen = true;
                    }
                }
            }
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            if process.leases.len() >= 4096 {
                return Err("lock request limit reached".to_owned());
            }
            let lease = Lease {
                id,
                owner: realm.id,
                origin: Arc::clone(&realm.origin),
                name,
                shared,
                held: false,
                stolen: false,
                released: false,
            };
            if steal {
                process.leases.insert(0, lease);
            } else {
                process.leases.push(lease);
            }
            process.grant();
            let ready = process
                .leases
                .iter()
                .filter(|lease| lease.held || lease.stolen)
                .map(|lease| lease.owner)
                .collect::<std::collections::HashSet<_>>();
            drop(process);
            for owner in ready {
                storage::mark_platform_ready(owner);
            }
            Ok(Reply::Id(id))
        }
        Operation::LockState { id } => {
            let process = lock(&PROCESS);
            let state = process
                .leases
                .iter()
                .find(|lease| lease.id == id && lease.owner == realm.id)
                .map_or(3, |lease| {
                    if lease.stolen {
                        2
                    } else if lease.released {
                        3
                    } else if lease.held {
                        1
                    } else {
                        0
                    }
                });
            Ok(Reply::Id(state))
        }
        Operation::LockStatus { id } => {
            let process = lock(&PROCESS);
            Ok(Reply::Boolean(process.leases.iter().any(|lease| {
                lease.id == id && lease.owner == realm.id && lease.held
            })))
        }
        Operation::LockRelease { id } => {
            let mut process = lock(&PROCESS);
            if let Some(lease) = process
                .leases
                .iter_mut()
                .find(|lease| lease.id == id && lease.owner == realm.id)
            {
                lease.held = false;
                lease.released = true;
            }
            process.leases.retain(|lease| !lease.released);
            process.grant();
            let ready = process
                .leases
                .iter()
                .filter(|lease| lease.held)
                .map(|lease| lease.owner)
                .collect::<std::collections::HashSet<_>>();
            drop(process);
            for owner in ready {
                storage::mark_platform_ready(owner);
            }
            Ok(Reply::Null)
        }
        Operation::LockCancel { id } => {
            let mut process = lock(&PROCESS);
            if let Some(index) = process
                .leases
                .iter()
                .position(|lease| lease.id == id && lease.owner == realm.id && !lease.held)
            {
                process.leases.remove(index);
                process.grant();
            }
            let ready = process
                .leases
                .iter()
                .filter(|lease| lease.held)
                .map(|lease| lease.owner)
                .collect::<std::collections::HashSet<_>>();
            drop(process);
            for owner in ready {
                storage::mark_platform_ready(owner);
            }
            Ok(Reply::Null)
        }
        Operation::LockQuery => {
            let process = lock(&PROCESS);
            let entries = |held| {
                process
                    .leases
                    .iter()
                    .filter(|lease| {
                        !lease.stolen
                            && !lease.released
                            && lease.origin == realm.origin
                            && lease.held == held
                    })
                    .map(|lease| LockEntry {
                        name: lease.name.clone(),
                        mode: if lease.shared { "shared" } else { "exclusive" },
                        client_id: lease.owner.to_string(),
                    })
                    .collect::<Vec<_>>()
            };
            Ok(Reply::LockQuery {
                held: entries(true),
                pending: entries(false),
            })
        }
        Operation::WebauthnInfo => Ok(Reply::Boolean(
            realm.secure && crate::webauthn::platform_available(),
        )),
        Operation::WebauthnCreate { options } => Ok(Reply::Webauthn(crate::webauthn::create(
            realm.id,
            &realm.public_origin,
            realm.secure,
            options,
            &realm.notifications,
        ))),
        Operation::WebauthnGet { options } => Ok(Reply::Webauthn(crate::webauthn::get(
            realm.id,
            &realm.public_origin,
            realm.secure,
            options,
            &realm.notifications,
        ))),
        Operation::WebauthnStatus { id } => {
            Ok(Reply::Webauthn(crate::webauthn::status(realm.id, id)))
        }
        Operation::WebauthnAbort { id } => {
            crate::webauthn::abort(realm.id, id);
            Ok(Reply::Null)
        }
    }
}

fn inflate(format: &str, input: &[u8]) -> std::io::Result<Vec<u8>> {
    if format == "gzip" {
        let mut decoder = flate2::bufread::GzDecoder::new(input);
        let mut output = Vec::new();
        (&mut decoder)
            .take((LIMIT + 1) as u64)
            .read_to_end(&mut output)?;
        if output.len() > LIMIT || !decoder.into_inner().is_empty() {
            return Err(std::io::Error::other("invalid or oversized gzip stream"));
        }
        return Ok(output);
    }
    let mut decoder = flate2::Decompress::new(format == "deflate");
    let mut output = Vec::new();
    let mut chunk = [0_u8; 16384];
    loop {
        let before_in = decoder.total_in();
        let before_out = decoder.total_out();
        let status = decoder
            .decompress(
                &input[before_in as usize..],
                &mut chunk,
                flate2::FlushDecompress::Finish,
            )
            .map_err(std::io::Error::other)?;
        let written = (decoder.total_out() - before_out) as usize;
        if output.len().saturating_add(written) > LIMIT {
            return Err(std::io::Error::other(
                "decompressed allocation limit exceeded",
            ));
        }
        output.extend_from_slice(&chunk[..written]);
        if status == flate2::Status::StreamEnd {
            if decoder.total_in() as usize != input.len() {
                return Err(std::io::Error::other("trailing compressed data"));
            }
            return Ok(output);
        }
        if decoder.total_in() == before_in && written == 0 {
            return Err(std::io::Error::other("truncated compressed stream"));
        }
    }
}

fn compress(format: &str, input: &[u8], decode: bool) -> Result<Vec<u8>, String> {
    if !matches!(format, "gzip" | "deflate" | "deflate-raw") {
        return Err("unsupported compression format".to_owned());
    }
    if input.len() > LIMIT {
        return Err("compression allocation limit exceeded".to_owned());
    }
    let output = if decode {
        inflate(format, input)
    } else {
        match format {
            "gzip" => {
                let mut encoder =
                    flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                encoder.write_all(input).and_then(|()| encoder.finish())
            }
            "deflate" => {
                let mut encoder =
                    flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
                encoder.write_all(input).and_then(|()| encoder.finish())
            }
            _ => {
                let mut encoder =
                    flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                encoder.write_all(input).and_then(|()| encoder.finish())
            }
        }
    }
    .map_err(|error| error.to_string())?;
    if output.len() > LIMIT {
        return Err("compression output allocation limit exceeded".to_owned());
    }
    Ok(output)
}

#[cfg(test)]
mod wave35_tests {
    use super::*;
    fn realm(origin: &str) -> Arc<Realm> {
        new_realm(
            Some(&Url::parse(origin).unwrap()),
            crate::net_bridge::PageWake::default(),
        )
    }
    fn id(reply: Reply) -> u64 {
        if let Reply::Id(id) = reply {
            id
        } else {
            panic!("expected id")
        }
    }
    #[test]
    fn wave35_webapi_ports_queue_start_and_owner_close() {
        let (a, b) = (
            realm("https://ports.invalid"),
            realm("https://ports.invalid"),
        );
        let port = id(dispatch(&a, Operation::PortPair).unwrap());
        dispatch(
            &a,
            Operation::PortPost {
                id: port,
                wire: "one".into(),
            },
        )
        .unwrap();
        dispatch(
            &a,
            Operation::PortPost {
                id: port,
                wire: "two".into(),
            },
        )
        .unwrap();
        assert!(
            matches!(dispatch(&a,Operation::PortRead).unwrap(),Reply::BroadcastDeliveries(v) if v.is_empty())
        );
        dispatch(&b, Operation::PortClose { id: port + 1 }).unwrap();
        dispatch(&a, Operation::PortStart { id: port + 1 }).unwrap();
        assert!(
            matches!(dispatch(&a,Operation::PortRead).unwrap(),Reply::BroadcastDeliveries(v) if v.len()==2 && v[0].wire.as_ref()=="one" && v[1].wire.as_ref()=="two")
        );
        dispatch(&a, Operation::PortClose { id: port + 1 }).unwrap();
        assert!(
            dispatch(
                &a,
                Operation::PortPost {
                    id: port,
                    wire: "three".into()
                }
            )
            .is_err()
        );
    }
    #[test]
    fn wave35_webapi_broadcast_origin_sender_and_fifo() {
        let (a, b, c) = (
            realm("https://broadcast.invalid"),
            realm("https://broadcast.invalid"),
            realm("https://other.invalid"),
        );
        let x = id(dispatch(
            &a,
            Operation::BroadcastOpen {
                name: "test".into(),
            },
        )
        .unwrap());
        let y = id(dispatch(
            &a,
            Operation::BroadcastOpen {
                name: "test".into(),
            },
        )
        .unwrap());
        let z = id(dispatch(
            &b,
            Operation::BroadcastOpen {
                name: "test".into(),
            },
        )
        .unwrap());
        dispatch(
            &c,
            Operation::BroadcastOpen {
                name: "test".into(),
            },
        )
        .unwrap();
        for wire in ["one", "two"] {
            dispatch(
                &a,
                Operation::BroadcastPost {
                    id: x,
                    wire: wire.into(),
                },
            )
            .unwrap();
        }
        assert!(
            matches!(dispatch(&a,Operation::BroadcastRead).unwrap(),Reply::BroadcastDeliveries(v) if v.len()==2 && v.iter().all(|v|v.id==y) && v[0].wire.as_ref()=="one")
        );
        assert!(
            matches!(dispatch(&b,Operation::BroadcastRead).unwrap(),Reply::BroadcastDeliveries(v) if v.len()==2 && v.iter().all(|v|v.id==z))
        );
        assert!(
            matches!(dispatch(&c,Operation::BroadcastRead).unwrap(),Reply::BroadcastDeliveries(v) if v.is_empty())
        );
    }
    #[test]
    fn wave35_webapi_lock_fairness_and_steal_identity() {
        let (a, b, c) = (
            realm("https://locks.invalid"),
            realm("https://locks.invalid"),
            realm("https://locks.invalid"),
        );
        let request = |shared, available, steal| Operation::LockRequest {
            name: "test".into(),
            shared,
            available,
            steal,
        };
        let x = id(dispatch(&a, request(true, false, false)).unwrap());
        let y = id(dispatch(&b, request(true, false, false)).unwrap());
        let z = id(dispatch(&c, request(false, false, false)).unwrap());
        assert!(matches!(
            dispatch(&a, Operation::LockStatus { id: x }).unwrap(),
            Reply::Boolean(true)
        ));
        assert!(matches!(
            dispatch(&b, Operation::LockStatus { id: y }).unwrap(),
            Reply::Boolean(true)
        ));
        assert!(matches!(
            dispatch(&c, Operation::LockStatus { id: z }).unwrap(),
            Reply::Boolean(false)
        ));
        assert!(matches!(
            dispatch(&a, request(true, true, false)).unwrap(),
            Reply::Null
        ));
        let stolen = id(dispatch(&b, request(false, false, true)).unwrap());
        assert!(matches!(
            dispatch(&a, Operation::LockState { id: x }).unwrap(),
            Reply::Id(2)
        ));
        assert!(matches!(
            dispatch(&b, Operation::LockStatus { id: stolen }).unwrap(),
            Reply::Boolean(true)
        ));
        dispatch(&a, Operation::LockRelease { id: x }).unwrap();
        assert!(matches!(
            dispatch(&b, Operation::LockStatus { id: stolen }).unwrap(),
            Reply::Boolean(true)
        ));
        drop(b);
        assert!(matches!(
            dispatch(&c, Operation::LockStatus { id: z }).unwrap(),
            Reply::Boolean(true)
        ));
    }
}
