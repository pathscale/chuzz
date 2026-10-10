use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

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
const PIXEL: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 4, 0,
    0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 100, 248, 15, 0, 1, 5, 1, 1,
    39, 24, 227, 102, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

fn main() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);
    let port = match (args.next().as_deref(), args.next()) {
        (None, None) => 49_123,
        (Some("--port"), Some(value)) if args.next().is_none() => {
            value.parse().map_err(|error| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("invalid port: {error}"),
                )
            })?
        }
        _ => {
            eprintln!("usage: chuzz-qa-fixture [--port PORT]");
            std::process::exit(2);
        }
    };

    let runtime = nagoya::runtime::Runtime::new(4);
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    for connection in listener.incoming() {
        match connection {
            Ok(stream) => {
                runtime.executor().submit(move || {
                    if let Err(error) = serve(stream) {
                        eprintln!("fixture request failed: {error}");
                    }
                });
            }
            Err(error) => eprintln!("fixture accept failed: {error}"),
        }
    }
    Ok(())
}

fn delay(duration: Duration) {
    nagoya::block_on(nagoya::sleep(duration));
}

fn serve(mut stream: TcpStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut parts = request_line.split_ascii_whitespace();
    let method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or("/");
    let mut headers = Vec::new();
    let mut size = request_line.len();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return respond(&mut stream, "400 Bad Request", "text/plain", b"incomplete request");
        }
        size += line.len();
        if size > 16_384 {
            return respond(&mut stream, "431 Request Header Fields Too Large", "text/plain", b"headers");
        }
        if line == "\r\n" {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            return respond(&mut stream, "400 Bad Request", "text/plain", b"invalid header");
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
    let length = if header("content-length").is_empty() {
        0
    } else {
        header("content-length").parse::<usize>().map_err(|error| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())
        })?
    };
    if length > 65_536 {
        return respond(&mut stream, "413 Content Too Large", "text/plain", b"body");
    }
    let mut request_body = vec![0; length];
    reader.read_exact(&mut request_body)?;

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
        "/api/udp" => {
            if method != "POST" {
                return respond(
                    &mut stream,
                    "405 Method Not Allowed",
                    "text/plain",
                    b"POST required",
                );
            }
            match udp::run() {
                Ok(report) => {
                    return respond(
                        &mut stream,
                        "200 OK",
                        "application/json",
                        report.as_bytes(),
                    );
                }
                Err(error) => {
                    return respond(
                        &mut stream,
                        "500 Internal Server Error",
                        "text/plain",
                        format!("UDP fixture failed: {error}").as_bytes(),
                    );
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
                );
            }
            return respond(
                &mut stream,
                "201 Created",
                "application/json",
                br#"{"accepted":true}"#,
            );
        }
        "/api/invalid-json" => ("application/json", b"invalid JSON".to_vec()),
        "/api/slow" => {
            delay(Duration::from_millis(150));
            ("application/json", br#"{"value":21}"#.to_vec())
        }
        "/stress.html" => ("text/html; charset=utf-8", stress_html()),
        "/stress-entry.js" => (
            "text/javascript; charset=utf-8",
            br#"import("/late.js").then(() => { document.title = "Stress ready"; });"#.to_vec(),
        ),
        "/late.js" => {
            delay(Duration::from_millis(150));
            (
                "text/javascript; charset=utf-8",
                b"export const ready = true;".to_vec(),
            )
        }
        path if path.starts_with("/slow-image/") => {
            delay(Duration::from_millis(50));
            ("image/png", PIXEL.to_vec())
        }
        _ => return respond(&mut stream, "404 Not Found", "text/plain", b"not found"),
    };

    respond(&mut stream, "200 OK", content_type, &body)
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

fn respond(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nX-Fixture: received\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)
}

