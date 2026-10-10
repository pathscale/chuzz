//! Process-local web API state. Only owned Rust data crosses realm boundaries.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use blitz_script::boa_engine::object::builtins::{AlignedVec, JsArrayBuffer};
use blitz_script::boa_engine::{
    Context, JsNativeError, JsResult, JsString, JsValue, NativeFunction, Source,
};
use blitz_traits::net::Url;

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
}

impl Drop for Realm {
    fn drop(&mut self) {
        let mut process = lock(&PROCESS);
        process.channels.retain(|_, channel| channel.owner != self.id);
        process.leases.retain(|lease| lease.owner != self.id);
        process.grant();
    }
}

#[derive(Clone)]
struct Host(Arc<Realm>);

#[derive(serde::Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
enum Operation {
    Info,
    Random,
    BroadcastOpen { name: String },
    BroadcastClose { id: u64 },
    BroadcastPost { id: u64, wire: String },
    BroadcastRead,
    LockRequest {
        name: String,
        shared: bool,
        available: bool,
    },
    LockStatus { id: u64 },
    LockRelease { id: u64 },
    LockQuery,
}

fn text(args: &[JsValue], index: usize, context: &mut Context) -> JsResult<String> {
    Ok(args
        .get(index)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped())
}

fn native_error(message: impl std::fmt::Display) -> blitz_script::boa_engine::JsError {
    JsNativeError::typ().with_message(message.to_string()).into()
}

fn platform(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let host = context
        .get_data::<Host>()
        .cloned()
        .ok_or_else(|| native_error("web API realm is unavailable"))?;
    let description = text(args, 0, context)?;
    if description.len() > LIMIT {
        return Err(native_error("web API message exceeds the allocation limit"));
    }
    let operation: Operation =
        serde_json::from_str(&description).map_err(native_error)?;
    let realm = &host.0;
    let result = match operation {
        Operation::Info => serde_json::json!({
            "origin": realm.public_origin,
            "secure": realm.secure,
            "platform": if cfg!(target_os = "macos") { "MacIntel" }
                else if cfg!(target_os = "windows") { "Win32" } else { "Linux" },
            "uaPlatform": if cfg!(target_os = "macos") { "macOS" }
                else if cfg!(target_os = "windows") { "Windows" } else { "Linux" },
            "architecture": if cfg!(target_arch = "aarch64") { "arm" }
                else if cfg!(any(target_arch = "x86", target_arch = "x86_64")) { "x86" }
                else { std::env::consts::ARCH },
            "bitness": usize::BITS.to_string(),
            "concurrency": std::thread::available_parallelism().map_or(1, usize::from),
            "version": env!("CARGO_PKG_VERSION"),
        }),
        Operation::Random => {
            let provider = rustls::crypto::CryptoProvider::get_default()
                .ok_or_else(|| native_error("secure random provider is unavailable"))?;
            let mut bytes = [0_u8; 16];
            provider
                .secure_random
                .fill(&mut bytes)
                .map_err(|_| native_error("secure random generation failed"))?;
            serde_json::json!(bytes)
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
            serde_json::json!(id)
        }
        Operation::BroadcastClose { id } => {
            let mut process = lock(&PROCESS);
            if process.channels.get(&id).is_some_and(|channel| channel.owner == realm.id) {
                process.channels.remove(&id);
            }
            serde_json::Value::Null
        }
        Operation::BroadcastPost { id, wire } => {
            let mut process = lock(&PROCESS);
            let name = process
                .channels
                .get(&id)
                .filter(|channel| channel.owner == realm.id)
                .map(|channel| channel.name.clone())
                .ok_or_else(|| native_error("broadcast channel is closed"))?;
            let wire: Arc<str> = Arc::from(wire);
            for (other_id, channel) in &mut process.channels {
                if *other_id != id && channel.origin == realm.origin && channel.name == name {
                    channel.incoming.push_back(Arc::clone(&wire));
                }
            }
            serde_json::Value::Null
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
                        deliveries.push(serde_json::json!({
                            "id": id,
                            "wire": wire.as_ref(),
                        }));
                    }
                }
            }
            serde_json::json!(deliveries)
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
                serde_json::Value::Null
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
                serde_json::json!(id)
            }
        }
        Operation::LockStatus { id } => {
            let process = lock(&PROCESS);
            serde_json::json!(process.leases.iter().any(|lease| {
                lease.id == id && lease.owner == realm.id && lease.held
            }))
        }
        Operation::LockRelease { id } => {
            let mut process = lock(&PROCESS);
            process
                .leases
                .retain(|lease| lease.id != id || lease.owner != realm.id);
            process.grant();
            serde_json::Value::Null
        }
        Operation::LockQuery => {
            let process = lock(&PROCESS);
            let entries = |held| {
                process
                    .leases
                    .iter()
                    .filter(|lease| lease.origin == realm.origin && lease.held == held)
                    .map(|lease| {
                        serde_json::json!({
                            "name": lease.name,
                            "mode": if lease.shared { "shared" } else { "exclusive" },
                            "clientId": lease.owner.to_string(),
                        })
                    })
                    .collect::<Vec<_>>()
            };
            serde_json::json!({"held": entries(true), "pending": entries(false)})
        }
    };
    JsValue::from_json(&result, context)
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
            return Err(std::io::Error::other("decompressed allocation limit exceeded"));
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

fn codec(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let format = text(args, 0, context)?;
    if !matches!(format.as_str(), "gzip" | "deflate" | "deflate-raw") {
        return Err(native_error("unsupported compression format"));
    }
    let object = args
        .get(1)
        .and_then(JsValue::as_object)
        .ok_or_else(|| native_error("compression requires an ArrayBuffer"))?;
    let input = JsArrayBuffer::from_object(object)?
        .to_vec()
        .ok_or_else(|| native_error("detached compression input"))?;
    if input.len() > LIMIT {
        return Err(native_error("compression allocation limit exceeded"));
    }
    let decode = args.get(2).is_some_and(JsValue::to_boolean);
    let output = if decode {
        inflate(&format, &input)
    } else {
        match format.as_str() {
            "gzip" => {
                let mut encoder =
                    flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                encoder.write_all(&input).and_then(|()| encoder.finish())
            }
            "deflate" => {
                let mut encoder =
                    flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
                encoder.write_all(&input).and_then(|()| encoder.finish())
            }
            _ => {
                let mut encoder =
                    flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                encoder.write_all(&input).and_then(|()| encoder.finish())
            }
        }
    }
    .map_err(native_error)?;
    if output.len() > LIMIT {
        return Err(native_error("compression output allocation limit exceeded"));
    }
    Ok(JsArrayBuffer::from_byte_block(AlignedVec::from_iter(0, output), context)?.into())
}

pub(crate) fn install_context(context: &mut Context, url: Option<&Url>) -> JsResult<()> {
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
    context.insert_data(Host(Arc::new(Realm {
        id,
        origin: Arc::from(origin),
        public_origin,
        secure,
    })));
    for (name, length, function) in [
        (
            "__chuzzPlatformCall",
            1,
            platform as fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>,
        ),
        ("__chuzzCodec", 3, codec),
    ] {
        context.register_global_builtin_callable(
            JsString::from(name),
            length,
            NativeFunction::from_fn_ptr(function),
        )?;
    }
    context.eval(Source::from_bytes(include_str!("platform/common.ts")))?;
    Ok(())
}

pub(crate) fn install(document: &mut blitz_script::ScriptDocument) {
    let url = document.current_url().cloned();
    document.with_js_context(|context| {
        install_context(context, url.as_ref()).expect("web API native bootstrap");
        for source in [
            include_str!("platform/streams.ts"),
            include_str!("platform/body.ts"),
            include_str!("platform/window.ts"),
        ] {
            context
                .eval(Source::from_bytes(source))
                .expect("web API page bootstrap");
        }
    });
}

