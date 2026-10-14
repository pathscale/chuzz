use std::sync::Arc;
use std::time::{Duration, Instant};

use nago_wss::proto::message::{Limits, Message};
use nago_wss::upgrade::UpgradeOutcome;
use nagoya::net::{TcpListener, TcpStream};
use nagoya::reactor::{Addr, Reactor};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ServerConfig, ServerConnection};
use serde::Deserialize;

const HTML: &[u8] = include_bytes!("../../../apps/chuzz/src/webrtc/qa/tls/index.html");
const SCRIPT: &[u8] = include_bytes!("../../../apps/chuzz/src/webrtc/qa/tls/src/tls.ts");
const FONT: &[u8] =
    include_bytes!("../../../apps/chuzz/src/webrtc/qa/tls/fonts/FiraSans-Regular.woff2");
const SUBPROTOCOL: &str = "chuzz.w03.v1";

type TlsStream = nago_wss::tls::TlsStream<TcpStream>;

struct Options {
    port: u16,
    version: &'static rustls::SupportedProtocolVersion,
    version_name: &'static str,
    certs: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
}

fn options() -> Result<Options, String> {
    let mut port = 49_443;
    let mut version = &rustls::version::TLS13;
    let mut version_name = "TLS1.3";
    let mut certs = Vec::new();
    let mut key = None;
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        let value = args.next().ok_or_else(|| {
            "usage: chuzz-tls-fixture --port PORT --version 12|13 --cert DER [--cert DER] --key DER"
                .to_owned()
        })?;
        match argument.as_str() {
            "--port" => {
                port = value
                    .parse()
                    .map_err(|_| "invalid fixture port".to_owned())?;
                if port == 0 {
                    return Err("fixture port must be nonzero".to_owned());
                }
            }
            "--version" => match value.as_str() {
                "12" => {
                    version = &rustls::version::TLS12;
                    version_name = "TLS1.2";
                }
                "13" => {
                    version = &rustls::version::TLS13;
                    version_name = "TLS1.3";
                }
                _ => return Err("TLS version must be 12 or 13".to_owned()),
            },
            "--cert" => {
                certs.push(CertificateDer::from(
                    std::fs::read(value).map_err(|error| error.to_string())?,
                ));
            }
            "--key" if key.is_none() => {
                let bytes = std::fs::read(value).map_err(|error| error.to_string())?;
                key = Some(
                    PrivateKeyDer::try_from(bytes)
                        .map_err(|error| format!("invalid DER private key: {error}"))?,
                );
            }
            _ => return Err("unknown or repeated fixture argument".to_owned()),
        }
    }
    if certs.is_empty() {
        return Err("provide the leaf certificate and any intermediates with --cert".to_owned());
    }
    Ok(Options {
        port,
        version,
        version_name,
        certs,
        key: key.ok_or_else(|| "--key is required".to_owned())?,
    })
}

#[global_allocator]
static ALLOC: rusty_alloc::RustyAlloc = rusty_alloc::RustyAlloc;

fn main() {
    if let Err(error) = run() {
        eprintln!("TLS fixture failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let options = options()?;
    let provider = Arc::new(ps_rustcrypto::provider());
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[options.version])
        .map_err(|error| error.to_string())?
        .with_no_client_auth()
        .with_single_cert(options.certs, options.key)
        .map_err(|error| error.to_string())?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let config = Arc::new(config);
    let reactor = Reactor::start().map_err(|error| error.to_string())?;
    let listener = TcpListener::bind(Addr::localhost(options.port), &reactor.handle())
        .map_err(|error| error.to_string())?;
    eprintln!(
        "TLS fixture: https://localhost:{}/, {} only",
        options.port, options.version_name,
    );
    nagoya::block_on(async {
        loop {
            let (stream, _) = listener.accept().await.map_err(|error| error.to_string())?;
            let config = Arc::clone(&config);
            let version = options.version_name;
            let _detached = nagoya::spawn(async move {
                match nagoya::timeout(Duration::from_secs(60), serve(stream, config, version)).await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => eprintln!("TLS fixture connection failed: {error}"),
                    Err(_) => eprintln!("TLS fixture connection deadline expired"),
                }
            });
        }
    })
}

fn local_origin(headers: &[(String, String)]) -> Option<&str> {
    let origin = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("origin"))?
        .1
        .as_str();
    let url = url::Url::parse(origin).ok()?;
    (url.scheme() == "https"
        && url.host_str() == Some("localhost")
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none())
    .then_some(origin)
}

async fn serve(
    stream: TcpStream,
    config: Arc<ServerConfig>,
    version: &'static str,
) -> Result<(), String> {
    let session = ServerConnection::new(config).map_err(|error| error.to_string())?;
    let mut stream = TlsStream::server(stream, session);
    stream
        .handshake()
        .await
        .map_err(|error| format!("TLS handshake: {error}"))?;
    let limits = Limits {
        max_frame: 128 * 1024,
        max_message: 128 * 1024,
    };
    let outcome = nago_wss::upgrade::accept(
        stream,
        limits,
        &[("Cache-Control", "no-store")],
        |request| {
            let protocol = (request.path == "/session"
                && local_origin(&request.headers).is_some()
                && request.protocols.iter().any(|value| value == SUBPROTOCOL))
            .then(|| SUBPROTOCOL.to_owned());
            (protocol, Vec::new())
        },
    )
    .await
    .map_err(|error| error.to_string())?;

    match outcome {
        UpgradeOutcome::Upgraded(mut connection, request) => {
            if request.path != "/session"
                || local_origin(&request.headers).is_none()
                || !request.protocols.iter().any(|value| value == SUBPROTOCOL)
            {
                connection
                    .close(None)
                    .await
                    .map_err(|error| error.to_string())?;
                let origin = request
                    .headers
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case("origin"))
                    .map(|(_, value)| value.as_str());
                return Err(format!(
                    "fixture upgrade policy rejected request: path {:?}, origin {:?}, protocols {:?}",
                    request.path, origin, request.protocols
                ));
            }
            let provider = ps_rustcrypto::provider();
            let mut entropy = [0u8; 16];
            provider
                .secure_random
                .fill(&mut entropy)
                .map_err(|_| "session entropy unavailable".to_owned())?;
            let session: String = entropy.iter().map(|byte| format!("{byte:02x}")).collect();
            let hello = serde_json::json!({
                "type": "hello",
                "session": session,
                "tls": version,
            });
            connection
                .write(Message::Text(hello.to_string().into_bytes().into()))
                .await
                .map_err(|error| error.to_string())?;
            let start = Instant::now();
            let mut expected = 1u32;
            loop {
                let message = nagoya::timeout(Duration::from_secs(8), connection.read())
                    .await
                    .map_err(|_| "heartbeat deadline expired".to_owned())?
                    .map_err(|error| error.to_string())?;
                match message {
                    Some(Message::Text(bytes)) => {
                        let heartbeat: Heartbeat = serde_json::from_slice(&bytes)
                            .map_err(|_| "invalid heartbeat".to_owned())?;
                        if heartbeat.kind != "heartbeat"
                            || heartbeat.session != session
                            || heartbeat.sequence != expected
                            || heartbeat.payload.len() != 32_768
                            || !heartbeat.payload.bytes().all(|byte| byte == b'q')
                            || expected > 32
                        {
                            return Err(
                                "heartbeat session, sequence or payload mismatch".to_owned()
                            );
                        }
                        let reply = serde_json::json!({
                            "type": "ack",
                            "session": session,
                            "sequence": expected,
                            "tls": version,
                            "elapsedMs": start.elapsed().as_millis(),
                            "payload": heartbeat.payload,
                        });
                        connection
                            .write(Message::Text(reply.to_string().into_bytes().into()))
                            .await
                            .map_err(|error| error.to_string())?;
                        expected += 1;
                    }
                    Some(Message::Ping(payload)) => {
                        connection
                            .pong(payload)
                            .await
                            .map_err(|error| error.to_string())?;
                    }
                    Some(Message::Pong(_)) => {}
                    Some(Message::Close(frame)) => {
                        connection
                            .close(frame)
                            .await
                            .map_err(|error| error.to_string())?;
                        eprintln!(
                            "TLS fixture session complete: {version}, {} heartbeats, {} ms",
                            expected - 1,
                            start.elapsed().as_millis(),
                        );
                        return Ok(());
                    }
                    Some(Message::Binary(_)) => {
                        return Err("fixture requires text heartbeats".to_owned());
                    }
                    None => return Err("session ended without a close frame".to_owned()),
                }
            }
        }
        UpgradeOutcome::Plain {
            mut stream,
            request,
            ..
        } => {
            let origin = local_origin(&request.headers);
            let path = request.path.split('?').next().unwrap_or_default();
            let asset;
            let (status, content_type, body) = match path {
                "/" | "/index.html" => ("200 OK", "text/html; charset=utf-8", HTML),
                "/src/tls.ts" => ("200 OK", "text/javascript; charset=utf-8", SCRIPT),
                "/fonts/FiraSans-Regular.woff2" => ("200 OK", "font/woff2", FONT),
                "/asset.json" => {
                    asset = serde_json::json!({
                        "tls": version,
                        "marker": "chuzz-w03-https",
                        "payload": "h".repeat(65_536),
                    })
                    .to_string()
                    .into_bytes();
                    ("200 OK", "application/json", asset.as_slice())
                }
                _ => ("404 Not Found", "text/plain", b"not found".as_slice()),
            };
            let cors = origin
                .map(|origin| format!("Access-Control-Allow-Origin: {origin}\r\nVary: Origin\r\n"))
                .unwrap_or_default();
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\n\
                 Content-Length: {}\r\nCache-Control: no-store\r\n\
                 {cors}Connection: close\r\n\r\n",
                body.len(),
            );
            stream
                .write_all(head.as_bytes())
                .await
                .map_err(|error| error.to_string())?;
            stream
                .write_all(body)
                .await
                .map_err(|error| error.to_string())?;
            stream.close().await.map_err(|error| error.to_string())
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Heartbeat {
    #[serde(rename = "type")]
    kind: String,
    session: String,
    sequence: u32,
    payload: String,
}
