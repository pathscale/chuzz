//! Linked functional inputs for the production reactor paths.

use std::future::{Future, poll_fn};
use std::path::Path;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use blitz_control_protocol::{AgentControlRequest, DebugResponse};

use super::{ControlBridgeRequest, egress, page_control};

fn argument(name: &str) -> Result<String, String> {
    let args: Vec<_> = std::env::args().collect();
    args.iter()
        .position(|value| value == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
        .ok_or_else(|| format!("missing {name}"))
}

fn page_ring() -> Result<(), String> {
    let (sender, receiver) = page_control::channel(4);
    let answer = page_control::Reply::new();
    let work = Arc::new((
        ControlBridgeRequest::Agent(AgentControlRequest::Inspect {
            root: None,
            max_depth: 3,
        }),
        Arc::clone(&answer),
    ));
    nagoya::block_on(async {
        let mut receiving = Box::pin(nagoya::timeout(Duration::from_secs(1), receiver.recv_arc()));
        let mut published = false;
        let received = poll_fn(|cx| {
            if let Poll::Ready(result) = receiving.as_mut().poll(cx) {
                return Poll::Ready(result);
            }
            // Publish only after the real receiver has registered its waker.
            if !published {
                if sender.try_send(Arc::clone(&work)).is_err() {
                    return Poll::Ready(Err(nagoya::Elapsed));
                }
                published = true;
            }
            Poll::Pending
        })
        .await
        .map_err(|_| "page-ring-wakeup: publication did not wake the receiver")?
        .map_err(|_| "page-ring-wakeup: page receiver closed")?;
        if !Arc::ptr_eq(&work, &received) {
            return Err("page-ring-ownership: received request pointer changed".into());
        }
        received.1.fill(DebugResponse::Ack);
        if !matches!(answer.recv().await.as_deref(), Some(DebugResponse::Ack)) {
            return Err("page-ring-reply: action answer missing".into());
        }
        Ok(())
    })
}

async fn cookies(directory: &Path) -> Result<(), String> {
    use crate::cookie_store::{BrowserCookieStore, reactor_receipt};
    let receipt = reactor_receipt();
    let store = BrowserCookieStore::open(directory)
        .await
        .map_err(|error| error.to_string())?;
    let url = url::Url::parse("https://cookie.example.com/").map_err(|error| error.to_string())?;
    store
        .set_script_cookie(&url, "reactor=debounced; Path=/; Secure; Max-Age=3600")
        .map_err(|error| error.to_string())?;
    let committed = nagoya::timeout(Duration::from_secs(3), receipt.recv())
        .await
        .map_err(|_| "cookie-debounce: timer did not commit its Arc snapshot")?
        .ok_or("cookie-debounce: writer closed")?;
    committed
        .as_ref()
        .clone()
        .map_err(|error| format!("cookie-debounce: {error}"))?;
    store.close().await.map_err(|error| error.to_string())?;
    let store = BrowserCookieStore::open(directory)
        .await
        .map_err(|error| error.to_string())?;
    if store.cookie_header(&url).as_deref() != Some("reactor=debounced") {
        return Err("cookie-debounce: reopened profile lost its timed snapshot".into());
    }
    store
        .set_script_cookie(&url, "reactor=before; Path=/; Secure; Max-Age=3600")
        .map_err(|error| error.to_string())?;
    store.flush().await.map_err(|error| error.to_string())?;
    store
        .set_script_cookie(&url, "reactor=after; Path=/; Secure; Max-Age=3600")
        .map_err(|error| error.to_string())?;
    store.flush().await.map_err(|error| error.to_string())?;
    store.close().await.map_err(|error| error.to_string())?;
    let reopened = BrowserCookieStore::open(directory)
        .await
        .map_err(|error| error.to_string())?;
    if reopened.cookie_header(&url).as_deref() != Some("reactor=after") {
        return Err("cookie-barrier: a Flush or Close restored older state".into());
    }
    reopened.close().await.map_err(|error| error.to_string())?;
    Ok(())
}

async fn wall_timeout(handle: &nagoya::reactor::Handle) -> Result<(), String> {
    let mut child =
        nagoya::process::Command::new(std::env::current_exe().map_err(|error| error.to_string())?)
            .arg("--wall-child")
            .kill_on_drop(true)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn(handle)
            .map_err(|error| error.to_string())?;
    match nagoya::timeout(Duration::from_secs(2), child.wait()).await {
        Ok(Ok(status)) if status.code() == Some(124) => Ok(()),
        Ok(status) => Err(format!(
            "wall-timeout: expected CLI exit 124, got {status:?}"
        )),
        Err(_) => {
            let _ = child.kill().await;
            Err("wall-timeout: timer did not stop the child".into())
        }
    }
}

async fn cli(handle: &nagoya::reactor::Handle, root: &Path) -> Result<(), String> {
    let host = argument("--host")?;
    let site = argument("--site")?;
    let hang = argument("--hang")?;
    let mut child = nagoya::process::Command::new(&host)
        .arg(&site)
        .env("CHUZZ_PROFILE_DIR", root.join("cli"))
        .env("CHUZZ_CONTROL_DIR", root.join("control"))
        .env("CHUZZ_HEADLESS_MODE", "qa")
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn(handle)
        .map_err(|error| error.to_string())?;
    let announced = nagoya::timeout(
        Duration::from_secs(2),
        crate::headless_io::read_announcement(
            child.stdout.take().ok_or("stdout-announce: pipe missing")?,
        ),
    )
    .await
    .map_err(|_| "stdout-announce: reactor pipe did not deliver the descriptor")??;
    let mut client = chuzz_control::client::Client::connect(Path::new(announced.trim()))
        .await
        .map_err(|error| error.to_string())?;
    let snapshot = nagoya::timeout(Duration::from_secs(15), client.inspect(None, 20))
        .await
        .map_err(|_| "cli-inspection: page did not answer")?
        .map_err(|error| error.to_string())?;
    if !snapshot.to_string().contains("Reactive page ready") {
        return Err("cli-inspection: real page heading missing".into());
    }
    let _ = child.kill().await;

    let mut child = nagoya::process::Command::new(&host)
        .arg(&hang)
        .env("CHUZZ_PROFILE_DIR", root.join("hang"))
        .env("CHUZZ_CONTROL_DIR", root.join("hang-control"))
        .env("CHUZZ_HEADLESS_MODE", "qa")
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn(handle)
        .map_err(|error| error.to_string())?;
    let announced = crate::headless_io::read_announcement(
        child.stdout.take().ok_or("stdout-announce: pipe missing")?,
    )
    .await?;
    let mut client = chuzz_control::client::Client::connect(Path::new(announced.trim()))
        .await
        .map_err(|error| error.to_string())?;
    let _ = client
        .inspect(None, 20)
        .await
        .map_err(|error| error.to_string())?;
    // SIGSTOP is a real native stall, independent of Boa's JS time limit.
    let parent = child
        .id()
        .ok_or("supervisor-deadline: host already exited")?;
    let workers = nagoya::process::Command::new("pgrep")
        .arg("-P")
        .arg(parent.to_string())
        .output(handle)
        .await
        .map_err(|error| error.to_string())?;
    let workers = std::str::from_utf8(&workers.stdout).map_err(|error| error.to_string())?;
    let mut ids = workers.split_whitespace();
    let pid: libc::pid_t = ids
        .next()
        .ok_or("supervisor-deadline: page worker pid missing")?
        .parse()
        .map_err(|_| "supervisor-deadline: invalid page worker pid")?;
    if ids.next().is_some() {
        return Err("supervisor-deadline: expected one isolated page worker".into());
    }
    // SAFETY: pgrep selected this host's live child. SIGSTOP creates the native
    // stall; the production deadline path must kill and reap this child.
    if unsafe { libc::kill(pid, libc::SIGSTOP) } != 0 {
        return Err("supervisor-deadline: could not stop page worker".into());
    }
    let reply = nagoya::timeout(
        Duration::from_secs(12),
        client.call(
            serde_json::json!({ "command": "inspect", "params": { "root": null, "maxDepth": 20 } }),
        ),
    )
    .await
    .map_err(|_| "supervisor-deadline: a hung page never produced its control error")?
    .map_err(|error| error.to_string())?;
    if reply["value"]["code"] != "pageUnresponsive" {
        return Err(format!(
            "supervisor-deadline: expected pageUnresponsive, got {reply}"
        ));
    }
    let recovery = url::Url::from_file_path(Path::new(&site).join("index.html"))
        .map_err(|_| "supervisor-recovery: invalid fixture path")?;
    let reply = nagoya::timeout(
        Duration::from_secs(12),
        client.call(
            serde_json::json!({ "command": "navigate", "params": { "url": recovery.as_str() } }),
        ),
    )
    .await
    .map_err(|_| "supervisor-recovery: Navigate never acknowledged a fresh page")?
    .map_err(|error| error.to_string())?;
    if reply["result"] != "ack" {
        return Err(format!(
            "supervisor-recovery: wrong Navigate result {reply}"
        ));
    }
    let snapshot = client
        .inspect(None, 20)
        .await
        .map_err(|error| error.to_string())?;
    if !snapshot.to_string().contains("Reactive page ready") {
        return Err("supervisor-recovery: recovered page heading missing".into());
    }
    let _ = child.kill().await;
    Ok(())
}

pub(super) fn run() -> Result<(), String> {
    if std::env::args().any(|value| value == "--wall-child") {
        let _guard = super::WallTimeoutGuard::start(Duration::from_millis(20))?;
        return nagoya::block_on(std::future::pending());
    }
    let root = std::path::PathBuf::from(argument("--profile")?);
    let proxy = egress::witness_proxy()?;
    let reactor = nagoya::reactor::Reactor::start().map_err(|error| error.to_string())?;
    page_ring()?;
    nagoya::block_on(nagoya::timeout(
        Duration::from_secs(5),
        egress::witness(&proxy),
    ))
    .map_err(|_| "egress-accept: socket readiness or forwarding did not progress")??;
    drop(proxy);
    nagoya::block_on(async {
        cookies(&root.join("cookies")).await?;
        wall_timeout(&reactor.handle()).await?;
        cli(&reactor.handle(), &root).await?;
        Ok(())
    })?;
    println!(
        "page-ring-wakeup page-ring-ownership egress-accept egress-connection-limit egress-client-forward egress-upstream-forward egress-byte-cap cookie-debounce cookie-barrier wall-timeout stdout-announce cli-inspection supervisor-deadline supervisor-recovery: passed"
    );
    Ok(())
}
