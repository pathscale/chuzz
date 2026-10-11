use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use nagoya::net::{TcpListener, TcpStream};
use nagoya::reactor::{Addr, Reactor};

mod echo;
mod udp;

const INDEX_HTML: &[u8] = include_bytes!("../../../apps/chuzz/tests/fixture-site/index.html");
const FIRST_JS: &[u8] = include_bytes!("../../../apps/chuzz/tests/fixture-site/first.js");
const SECOND_HTML: &[u8] = include_bytes!("../../../apps/chuzz/tests/fixture-site/second.html");
const SECOND_JS: &[u8] = include_bytes!("../../../apps/chuzz/tests/fixture-site/second.js");
const SITES_HTML: &[u8] = include_bytes!("../../../apps/chuzz/src/video/qa/sites.html");
const SITES_SCRIPT: &[u8] = include_bytes!("../../../apps/chuzz/src/video/qa/src/sites.ts");
const FONT: &[u8] =
    include_bytes!("../../../apps/chuzz/src/video/qa/fonts/FiraSans-Regular.woff2");
const UDP_HTML: &[u8] =
    include_bytes!("../../../apps/chuzz/src/webrtc/qa/udp/index.html");
const UDP_SCRIPT: &[u8] =
    include_bytes!("../../../apps/chuzz/src/webrtc/qa/udp/src/udp.ts");
const ECHO_HTML: &[u8] =
    include_bytes!("../../../apps/chuzz/src/webrtc/qa/echo/index.html");
const ECHO_HELPER_HTML: &[u8] =
    include_bytes!("../../../apps/chuzz/src/webrtc/qa/echo/helper.html");
const ECHO_COMMON: &[u8] =
    include_bytes!("../../../apps/chuzz/src/webrtc/qa/echo/src/common.ts");
const ECHO_SCRIPT: &[u8] =
    include_bytes!("../../../apps/chuzz/src/webrtc/qa/echo/src/echo.ts");
const ECHO_HELPER_SCRIPT: &[u8] =
    include_bytes!("../../../apps/chuzz/src/webrtc/qa/echo/src/helper.ts");
const WEBAUTHN_HTML: &[u8] =
    include_bytes!("../../../apps/chuzz/src/platform/qa/webauthn.html");
const WEBAUTHN_FONT: &[u8] =
    include_bytes!("../../../apps/chuzz/src/platform/qa/fonts/FiraSans-Regular.woff2");
/// The Referer the last `/referer.png` request carried, for the domb fixture.
static IMAGE_REFERER: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

const PIXEL: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 4, 0,
    0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 100, 248, 15, 0, 1, 5, 1, 1,
    39, 24, 227, 102, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

fn io_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

#[global_allocator]
static ALLOC: rusty_alloc::RustyAlloc = rusty_alloc::RustyAlloc;

fn main() -> io::Result<()> {
    let mut port = 49_123;
    let mut echo_host = None;
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        let value = args.next().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "usage: chuzz-qa-fixture [--port PORT] [--echo-host ABSOLUTE_PATH]",
            )
        })?;
        match argument.as_str() {
            "--port" => {
                port = value.parse().map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("invalid port: {error}"),
                    )
                })?;
            }
            "--echo-host" if echo_host.is_none() => {
                let path = PathBuf::from(value);
                if !path.is_absolute() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "--echo-host must name an absolute chuzz-headless path",
                    ));
                }
                echo_host = Some(path);
            }
            _ => {
                eprintln!(
                    "usage: chuzz-qa-fixture [--port PORT] [--echo-host ABSOLUTE_PATH]"
                );
                std::process::exit(2);
            }
        }
    }

    let reactor = Reactor::start().map_err(io_error)?;
    let listener =
        TcpListener::bind(Addr::localhost(port), &reactor.handle()).map_err(io_error)?;
    let port = listener.local_addr().map_err(io_error)?.port();
    let echoes = echo::Registry::new(echo_host, port);

    nagoya::block_on(async {
        loop {
            let (stream, _) = listener.accept().await.map_err(io_error)?;
            let echoes = Arc::clone(&echoes);
            let _ = nagoya::spawn(async move {
                match nagoya::timeout(
                    Duration::from_secs(15),
                    serve(stream, echoes, port),
                )
                .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => eprintln!("fixture request failed: {error}"),
                    Err(_) => eprintln!("fixture request deadline expired"),
                }
            });
        }
    })
}

async fn delay(duration: Duration) {
    nagoya::sleep(duration).await;
}

async fn serve(
    mut stream: TcpStream,
    echoes: Arc<echo::Registry>,
    port: u16,
) -> io::Result<()> {
    let mut head = Vec::new();
    let mut byte = [0; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() >= 16_384 {
            return respond(
                &mut stream,
                "431 Request Header Fields Too Large",
                "text/plain",
                b"headers",
            )
            .await;
        }
        if stream.read(&mut byte).await.map_err(io_error)? == 0 {
            return respond(
                &mut stream,
                "400 Bad Request",
                "text/plain",
                b"incomplete request",
            )
            .await;
        }
        head.push(byte[0]);
    }
    let head = String::from_utf8(head).map_err(io_error)?;
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_ascii_whitespace();
    let method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or("/");
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            return respond(
                &mut stream,
                "400 Bad Request",
                "text/plain",
                b"invalid header",
            )
            .await;
        };
        headers.push((name.to_ascii_lowercase(), value.trim().to_owned()));
    }
    let header = |name: &str| {
        headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
            .unwrap_or_default()
    };
    if !header("transfer-encoding").is_empty()
        || headers
            .iter()
            .filter(|(name, _)| name == "content-length")
            .count()
            > 1
    {
        return respond(
            &mut stream,
            "400 Bad Request",
            "text/plain",
            b"ambiguous request framing",
        )
        .await;
    }
    let length = if header("content-length").is_empty() {
        0
    } else {
        header("content-length").parse::<usize>().map_err(|error| {
            io::Error::new(io::ErrorKind::InvalidData, error.to_string())
        })?
    };
    let is_echo_api = path == "/api/echo" || path.starts_with("/api/echo/");
    let body_limit = if is_echo_api { 192 * 1024 } else { 65_536 };
    if length > body_limit {
        return respond(&mut stream, "413 Content Too Large", "text/plain", b"body").await;
    }
    let mut request_body = vec![0; length];
    let mut read = 0;
    while read < length {
        let count = stream
            .read(&mut request_body[read..])
            .await
            .map_err(io_error)?;
        if count == 0 {
            return respond(
                &mut stream,
                "400 Bad Request",
                "text/plain",
                b"incomplete body",
            )
            .await;
        }
        read += count;
    }

    if is_echo_api {
        let local_host = format!("127.0.0.1:{port}");
        let localhost_host = format!("localhost:{port}");
        let local_origin = format!("http://{local_host}");
        let localhost_origin = format!("http://{localhost_host}");
        if header("x-chuzz-echo") != "w09"
            || ![local_host.as_str(), localhost_host.as_str()].contains(&header("host"))
            || (!header("origin").is_empty()
                && ![local_origin.as_str(), localhost_origin.as_str()]
                    .contains(&header("origin")))
        {
            return respond(
                &mut stream,
                "403 Forbidden",
                "text/plain",
                b"same-origin echo fixture request required",
            )
            .await;
        }
        let (status, body) = echoes.route(method, path, &request_body).await;
        return respond(
            &mut stream,
            status,
            "application/json",
            body.to_string().as_bytes(),
        )
        .await;
    }

    let (content_type, body) = match path.split('?').next().unwrap_or(path) {
        "/" | "/index.html" => ("text/html; charset=utf-8", INDEX_HTML.to_vec()),
        "/first.js" => ("text/javascript; charset=utf-8", FIRST_JS.to_vec()),
        "/second.html" => ("text/html; charset=utf-8", SECOND_HTML.to_vec()),
        "/second.js" => ("text/javascript; charset=utf-8", SECOND_JS.to_vec()),
        "/sites.html" => ("text/html; charset=utf-8", SITES_HTML.to_vec()),
        "/src/sites.ts" => ("text/javascript; charset=utf-8", SITES_SCRIPT.to_vec()),
        "/fonts/FiraSans-Regular.woff2" => ("font/woff2", FONT.to_vec()),
        "/udp/" | "/udp/index.html" => ("text/html; charset=utf-8", UDP_HTML.to_vec()),
        "/udp/src/udp.ts" => ("text/javascript; charset=utf-8", UDP_SCRIPT.to_vec()),
        "/udp/fonts/FiraSans-Regular.woff2" => ("font/woff2", FONT.to_vec()),
        "/echo/" | "/echo/index.html" => ("text/html; charset=utf-8", ECHO_HTML.to_vec()),
        "/echo/helper.html" => ("text/html; charset=utf-8", ECHO_HELPER_HTML.to_vec()),
        "/echo/src/common.ts" => ("text/javascript; charset=utf-8", ECHO_COMMON.to_vec()),
        "/echo/src/echo.ts" => ("text/javascript; charset=utf-8", ECHO_SCRIPT.to_vec()),
        "/echo/src/helper.ts" => {
            ("text/javascript; charset=utf-8", ECHO_HELPER_SCRIPT.to_vec())
        }
        "/echo/fonts/FiraSans-Regular.woff2" => ("font/woff2", FONT.to_vec()),
        "/webauthn/" | "/webauthn/index.html" => {
            ("text/html; charset=utf-8", WEBAUTHN_HTML.to_vec())
        }
        "/webauthn/fonts/FiraSans-Regular.woff2" => ("font/woff2", WEBAUTHN_FONT.to_vec()),
        "/api/udp" => {
            if method != "POST" {
                return respond(
                    &mut stream,
                    "405 Method Not Allowed",
                    "text/plain",
                    b"POST required",
                )
                .await;
            }
            match udp::run() {
                Ok(report) => {
                    return respond(
                        &mut stream,
                        "200 OK",
                        "application/json",
                        report.as_bytes(),
                    )
                    .await;
                }
                Err(error) => {
                    return respond(
                        &mut stream,
                        "500 Internal Server Error",
                        "text/plain",
                        format!("UDP fixture failed: {error}").as_bytes(),
                    )
                    .await;
                }
            }
        }
        "/api/sites" => {
            let accepted = method == "POST"
                && request_body == b"age=18"
                && header("content-type").starts_with("application/x-www-form-urlencoded")
                && header("x-fixture") == "one, two"
                && header("cookie")
                    .split(';')
                    .any(|cookie| cookie.trim() == "qa_age=18");
            if !accepted {
                return respond(
                    &mut stream,
                    "422 Unprocessable Content",
                    "application/json",
                    br#"{"accepted":false}"#,
                )
                .await;
            }
            return respond(
                &mut stream,
                "201 Created",
                "application/json",
                br#"{"accepted":true}"#,
            )
            .await;
        }
        "/api/invalid-json" => ("application/json", b"invalid JSON".to_vec()),
        "/api/slow" => {
            delay(Duration::from_millis(150)).await;
            ("application/json", br#"{"value":21}"#.to_vec())
        }
        "/stress.html" => ("text/html; charset=utf-8", stress_html()),
        "/stress-entry.js" => (
            "text/javascript; charset=utf-8",
            br#"import("/late.js").then(() => { document.title = "Stress ready"; });"#.to_vec(),
        ),
        "/late.js" => {
            delay(Duration::from_millis(150)).await;
            (
                "text/javascript; charset=utf-8",
                b"export const ready = true;".to_vec(),
            )
        }
        "/referer.png" => {
            if let Ok(mut recorded) = IMAGE_REFERER.lock() {
                *recorded = Some(header("referer").to_owned());
            }
            ("image/png", PIXEL.to_vec())
        }
        "/api/image-referer" => {
            let referer = IMAGE_REFERER
                .lock()
                .map(|recorded| recorded.clone().unwrap_or_default())
                .unwrap_or_default();
            return respond(&mut stream, "200 OK", "text/plain; charset=utf-8", referer.as_bytes()).await;
        }
        path if path.starts_with("/slow-image/") => {
            delay(Duration::from_millis(50)).await;
            ("image/png", PIXEL.to_vec())
        }
        _ => {
            return respond(&mut stream, "404 Not Found", "text/plain", b"not found").await;
        }
    };

    respond(&mut stream, "200 OK", content_type, &body).await
}

fn stress_html() -> Vec<u8> {
    let mut html = String::from(
        "<!doctype html><html><head><meta charset=utf-8><title>Stress loading</title></head><body>",
    );
    for index in 0..96 {
        html.push_str(&format!(
            "<img alt=\"pixel {index}\" src=\"/slow-image/{index}.png\">"
        ));
    }
    html.push_str("<script type=module src=/stress-entry.js></script></body></html>");
    html.into_bytes()
}

async fn respond(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nAccess-Control-Allow-Origin: *\r\nX-Fixture: received\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await.map_err(io_error)?;
    stream.write_all(body).await.map_err(io_error)
}

