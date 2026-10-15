//! Process-local web API state. Only owned Rust data crosses realm boundaries.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use blitz_traits::net::Url;
use serde::Deserialize;
use serde_json::Value as Json;

#[path = "webapi_script.rs"]
mod script;

#[path = "webapi_host.rs"]
pub mod native_host;

const LIMIT: usize = 64 * 1024 * 1024;
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static PROCESS: LazyLock<Mutex<Process>> = LazyLock::new(Mutex::default);

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|poison| poison.into_inner())
}

#[derive(Default)]
struct Process {
    channels: HashMap<u64, Channel>,
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
}

impl Process {
    fn grant(&mut self) {
        for index in 0..self.leases.len() {
            if self.leases[index].held {
                continue;
            }
            let candidate = &self.leases[index];
            let blocked = self.leases.iter().enumerate().any(|(other_index, other)| {
                other_index != index
                    && other.origin == candidate.origin
                    && other.name == candidate.name
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
        process.leases.retain(|lease| lease.owner != self.id);
        process.grant();
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
    LockRequest {
        name: String,
        shared: bool,
        available: bool,
    },
    LockStatus {
        id: u64,
    },
    LockRelease {
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
            let mut process = lock(&PROCESS);
            let name = process
                .channels
                .get(&id)
                .filter(|channel| channel.owner == realm.id)
                .map(|channel| channel.name.clone())
                .ok_or_else(|| "broadcast channel is closed".to_owned())?;
            let wire: Arc<str> = Arc::from(wire);
            for (other_id, channel) in &mut process.channels {
                if *other_id != id && channel.origin == realm.origin && channel.name == name {
                    channel.incoming.push_back(Arc::clone(&wire));
                }
            }
            Ok(Reply::Null)
        }
        Operation::BroadcastRead => {
            let mut process = lock(&PROCESS);
            let mut deliveries = Vec::new();
            for (id, channel) in &mut process.channels {
                if channel.owner == realm.id {
                    for _ in 0..64 {
                        let Some(wire) = channel.incoming.pop_front() else {
                            break;
                        };
                        deliveries.push(BroadcastDelivery { id: *id, wire });
                    }
                }
            }
            Ok(Reply::BroadcastDeliveries(deliveries))
        }
        Operation::LockRequest {
            name,
            shared,
            available,
        } => {
            let mut process = lock(&PROCESS);
            let blocked = process.leases.iter().any(|lease| {
                lease.origin == realm.origin
                    && lease.name == name
                    && (!lease.held || !shared || !lease.shared)
            });
            if available && blocked {
                Ok(Reply::Null)
            } else {
                let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
                process.leases.push(Lease {
                    id,
                    owner: realm.id,
                    origin: Arc::clone(&realm.origin),
                    name,
                    shared,
                    held: false,
                });
                process.grant();
                Ok(Reply::Id(id))
            }
        }
        Operation::LockStatus { id } => {
            let process = lock(&PROCESS);
            Ok(Reply::Boolean(process.leases.iter().any(|lease| {
                lease.id == id && lease.owner == realm.id && lease.held
            })))
        }
        Operation::LockRelease { id } => {
            let mut process = lock(&PROCESS);
            process
                .leases
                .retain(|lease| lease.id != id || lease.owner != realm.id);
            process.grant();
            Ok(Reply::Null)
        }
        Operation::LockQuery => {
            let process = lock(&PROCESS);
            let entries = |held| {
                process
                    .leases
                    .iter()
                    .filter(|lease| lease.origin == realm.origin && lease.held == held)
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

pub(crate) use script::{drain_notifications, install, install_context, notifications};
