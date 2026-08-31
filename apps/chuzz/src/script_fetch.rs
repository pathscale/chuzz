//! Fetching the scripts a page asks for after it has started running.
//!
//! Both loading paths prefetch: they walk the parsed HTML, load every script it
//! names, and serve those from a map. That map cannot contain a script the page
//! builds for itself, because nobody knew the URL until the page was already
//! running. Such a request used to reach `DefaultScriptFetcher`, which serves
//! `file:` and `data:` only, so it was refused with `unsupported URL scheme for
//! script: https` and the script was dropped.
//!
//! Measured over a hundred-site corpus that was the most common single defect,
//! reaching a quarter of the sites. It hides well, because the scripts the
//! parser found load perfectly: a page fails only in the parts it assembles
//! itself.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use blitz_traits::net::{Request, Url};

use crate::decode::decode_body;
use crate::document_loader::NetProvider;

/// Serves prefetched sources, and fetches anything else from the network.
pub struct PageScripts {
    scripts: HashMap<Url, String>,
    net: Arc<NetProvider>,
    /// Used only when this is called from outside a runtime. Inside one, the
    /// current handle is the right one: it is the runtime already driving the
    /// page, and a second would run the fetch on a pool with its own shutdown.
    runtime: Option<tokio::runtime::Handle>,
    /// How long a runtime-discovered script may take before the page moves on.
    ///
    /// Bounded because this blocks script execution. A server that accepts the
    /// connection and never answers would otherwise hang the caller, and a
    /// missing script is a far better outcome than a page that never finishes.
    deadline: Duration,
}

impl PageScripts {
    pub fn new(scripts: HashMap<Url, String>, net: Arc<NetProvider>, deadline: Duration) -> Self {
        Self {
            scripts,
            net,
            runtime: tokio::runtime::Handle::try_current().ok(),
            deadline,
        }
    }

    fn fetch_over_network(&self, url: &Url) -> Result<String, blitz_script::FetchError> {
        let net = Arc::clone(&self.net);
        let target = url.clone();
        let deadline = self.deadline;
        let fetch = async move {
            tokio::time::timeout(deadline, net.fetch_async(Request::get(target))).await
        };

        // Called from inside the runtime driving the page, so blocking the
        // thread outright would stall the executor being waited on.
        // `block_in_place` hands the other tasks to a sibling worker first; it
        // exists only on the multi-threaded runtime, hence the flavour test
        // rather than an unwrap that would be right until someone built a
        // current-thread one.
        let joined = match tokio::runtime::Handle::try_current() {
            Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
                tokio::task::block_in_place(|| handle.block_on(fetch))
            }
            // Not on a runtime: nothing here is waiting on this thread.
            Err(_) => match &self.runtime {
                Some(handle) => handle.block_on(fetch),
                None => {
                    return Err(blitz_script::FetchError::InvalidData(
                        "no runtime available to fetch a script".to_owned(),
                    ));
                }
            },
            // A current-thread runtime has one worker and it is this one, so
            // blocking it and re-entering it both deadlock. Refusing the script
            // drops it exactly as this path did before, which is bad but
            // survivable; a load that never returns is not.
            Ok(_) => {
                return Err(blitz_script::FetchError::InvalidData(
                    "cannot fetch a script from a current-thread runtime".to_owned(),
                ));
            }
        };

        match joined {
            Ok(Ok((_, bytes))) => Ok(decode_body(&bytes)),
            Ok(Err(error)) => Err(blitz_script::FetchError::InvalidData(format!("{error:?}"))),
            Err(_) => Err(blitz_script::FetchError::InvalidData(format!(
                "script fetch timed out after {}s",
                deadline.as_secs()
            ))),
        }
    }
}

impl blitz_script::ScriptFetcher for PageScripts {
    fn fetch(&self, url: &Url) -> Result<String, blitz_script::FetchError> {
        if let Some(source) = self.scripts.get(url) {
            return Ok(source.clone());
        }
        match url.scheme() {
            "http" | "https" => self.fetch_over_network(url),
            // `file:` and `data:` still belong to the default fetcher: it
            // decodes data URLs correctly and needs no network.
            _ => blitz_script::DefaultScriptFetcher.fetch(url),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A script the page asks for at runtime is fetched, not refused.
    ///
    /// The prefetch map is deliberately empty: that is the state a page reaches
    /// when its own JavaScript builds a `<script>` for a URL the parser never
    /// saw. Before this path existed the request fell through to a fetcher that
    /// serves `file:` and `data:` only, and the script was dropped.
    ///
    /// Served from a real socket rather than a stub fetcher, because the thing
    /// worth proving is that a synchronous `fetch` can complete a network round
    /// trip from inside the runtime that is driving the page. A stub would pass
    /// without ever testing that.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_script_discovered_at_runtime_is_fetched_over_the_network() {
        use std::io::{Read, Write};

        const SOURCE: &str = "globalThis.__loaded = 42;";

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("loopback is available");
        let port = listener
            .local_addr()
            .expect("the socket has an address")
            .port();
        // Non-blocking with a deadline rather than a plain `accept`. If the
        // fetcher regresses to refusing the URL no connection is ever made, and
        // a blocking accept would hang this test forever instead of failing it.
        // A test that wedges on the very defect it exists to catch is worse
        // than no test: it stops CI rather than reporting.
        listener
            .set_nonblocking(true)
            .expect("the listener takes a nonblocking mode");
        let server = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(ref error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if std::time::Instant::now() >= deadline {
                            return false;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                    Err(_) => return false,
                }
            };
            stream
                .set_nonblocking(false)
                .expect("the accepted stream returns to blocking reads");
            let mut seen = Vec::new();
            let mut byte = [0u8; 1];
            while !seen.ends_with(b"\r\n\r\n") {
                match stream.read(&mut byte) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => seen.push(byte[0]),
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: {}\r\n\r\n{}",
                SOURCE.len(),
                SOURCE
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
            true
        });

        let fetcher = PageScripts::new(
            HashMap::new(),
            Arc::new(NetProvider::new(None)),
            Duration::from_secs(10),
        );
        let url = Url::parse(&format!("http://127.0.0.1:{port}/late.js")).expect("a valid URL");

        let fetched =
            tokio::task::spawn_blocking(move || blitz_script::ScriptFetcher::fetch(&fetcher, &url))
                .await
                .expect("the blocking fetch does not panic");

        let connected = server.join().expect("the server thread finishes");
        assert!(connected, "the fetcher never opened a connection");
        assert_eq!(
            fetched.expect("a runtime-discovered script is fetched"),
            SOURCE
        );
    }

    /// A `data:` script still resolves without touching the network.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_data_url_script_still_resolves_locally() {
        let fetcher = PageScripts::new(
            HashMap::new(),
            Arc::new(NetProvider::new(None)),
            Duration::from_secs(10),
        );
        // "let x = 1" as base64.
        let url = Url::parse("data:text/javascript;base64,bGV0IHggPSAx").expect("a valid data URL");

        let fetched = blitz_script::ScriptFetcher::fetch(&fetcher, &url);

        assert_eq!(fetched.expect("a data URL needs no network"), "let x = 1");
    }
}
