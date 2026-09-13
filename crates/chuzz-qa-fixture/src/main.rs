use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

const INDEX_HTML: &[u8] = include_bytes!("../../../apps/chuzz/tests/fixture-site/index.html");
const FIRST_JS: &[u8] = include_bytes!("../../../apps/chuzz/tests/fixture-site/first.js");
const SECOND_HTML: &[u8] = include_bytes!("../../../apps/chuzz/tests/fixture-site/second.html");
const SECOND_JS: &[u8] = include_bytes!("../../../apps/chuzz/tests/fixture-site/second.js");
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

    let listener = TcpListener::bind(("127.0.0.1", port))?;
    for connection in listener.incoming() {
        match connection {
            Ok(stream) => {
                thread::spawn(move || {
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

fn serve(mut stream: TcpStream) -> std::io::Result<()> {
    let mut request_line = String::new();
    BufReader::new(stream.try_clone()?).read_line(&mut request_line)?;
    let path = request_line.split_ascii_whitespace().nth(1).unwrap_or("/");

    let (content_type, body) = match path.split('?').next().unwrap_or(path) {
        "/" | "/index.html" => ("text/html; charset=utf-8", INDEX_HTML.to_vec()),
        "/first.js" => ("text/javascript; charset=utf-8", FIRST_JS.to_vec()),
        "/second.html" => ("text/html; charset=utf-8", SECOND_HTML.to_vec()),
        "/second.js" => ("text/javascript; charset=utf-8", SECOND_JS.to_vec()),
        "/stress.html" => ("text/html; charset=utf-8", stress_html()),
        "/stress-entry.js" => (
            "text/javascript; charset=utf-8",
            br#"import("/late.js").then(() => { document.title = "Stress ready"; });"#.to_vec(),
        ),
        "/late.js" => {
            thread::sleep(Duration::from_millis(150));
            (
                "text/javascript; charset=utf-8",
                b"export const ready = true;".to_vec(),
            )
        }
        path if path.starts_with("/slow-image/") => {
            thread::sleep(Duration::from_millis(50));
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
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)
}
