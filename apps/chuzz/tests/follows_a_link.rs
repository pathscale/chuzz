//! Clicking a link loads the next page, and what the first one stored is there
//! when the second one boots.
//!
//! Both halves are needed before a fleet site can be checked at all, and
//! neither is exercised by a single-page fixture.
//!
//! A plain `<a href>` is what the fleet has. `@solidjs/router` intercepts only
//! its own `<A>`, so every `Button href=` renders an ordinary anchor whose
//! activation belongs to the shell; a host with no shell acknowledges the click
//! and leaves the page where it was, which reads as a dead control.
//!
//! Storage is the other half. The web-API shim keeps it in memory per document,
//! so a navigation used to hand the next page an empty store. An application
//! reads its settings while it boots, which makes that read a miss and the
//! application look like it never saved -- and "save, go elsewhere, come back"
//! is the shape of most of what is worth checking on a settings page.
//!
//! The fixture is a directory rather than a file, so the host serves it as a
//! site and the link resolves against a real origin.

use nagoya::process::Command;
use std::process::Stdio;
use std::time::Duration;

use blitz_control_protocol::{
    AgentAction, AgentControlRequest, DebugEvent, DebugResponse, DebugStream, DiagnosticsRequest,
    JsonRpcId, MessageStream, NagoyaStream, TransportStream, decode_diagnostics_event,
    decode_response, encode_agent_request, encode_diagnostics_request, framed_json_neutral,
};

/// Kill the host however the test ends, including on a panic.
struct Host(nagoya::process::Child);

impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.0.start_kill();
        let _ = nagoya::block_on(self.0.wait());
    }
}

/// Remove the isolated browser profile after the child has been stopped.
struct TestProfile(std::path::PathBuf);

impl Drop for TestProfile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn request(
    stream: &mut dyn MessageStream,
    next_id: &mut i64,
    request: &AgentControlRequest,
) -> DebugResponse {
    *next_id += 1;
    let id = JsonRpcId::Number(*next_id);
    stream
        .send(encode_agent_request(id.clone(), request).expect("encode agent request"))
        .await
        .expect("send agent request");
    loop {
        let message = nagoya::timeout(Duration::from_secs(10), stream.recv())
            .await
            .unwrap_or_else(|_| panic!("host did not answer {request:?}"))
            .expect("the host keeps serving")
            .expect("read agent response");
        if let Ok((response_id, response)) = decode_response(message)
            && response_id == id
        {
            return response;
        }
    }
}

async fn observe_paint(stream: &mut dyn MessageStream, next_id: &mut i64) {
    *next_id += 1;
    let id = JsonRpcId::Number(*next_id);
    stream
        .send(
            encode_diagnostics_request(
                id.clone(),
                &DiagnosticsRequest::Observe {
                    streams: vec![DebugStream::Paint],
                },
            )
            .expect("encode observe request"),
        )
        .await
        .expect("send observe request");
    loop {
        let message = nagoya::timeout(Duration::from_secs(5), stream.recv())
            .await
            .expect("host did not answer paint observation")
            .expect("the host keeps serving")
            .expect("read observe response");
        if let Ok((response_id, DebugResponse::Ack)) = decode_response(message)
            && response_id == id
        {
            return;
        }
    }
}

async fn tree(
    stream: &mut dyn MessageStream,
    next_id: &mut i64,
) -> blitz_control_protocol::AgentSnapshot {
    let answer = request(
        stream,
        next_id,
        &AgentControlRequest::Inspect {
            root: None,
            max_depth: 20,
        },
    )
    .await;
    let DebugResponse::AgentSnapshot(snapshot) = answer else {
        panic!("inspect should return a semantic snapshot");
    };
    snapshot
}

#[test]
fn a_link_is_followed_and_storage_goes_with_it() {
    let site = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixture-site");
    let binary = env!("CARGO_BIN_EXE_chuzz-headless");
    let profile = TestProfile(std::env::temp_dir().join(format!(
        // Short: the host binds its control socket under this directory, and
        // macOS limits a Unix socket path to 104 bytes.
        "chuzz-link-{:x}",
        std::process::id()
    )));

    let reactor = nagoya::reactor::Reactor::start().expect("client reactor");
    let mut host = Host(
        Command::new(binary)
            .env("QA_INSPECT_PAGE", site)
            .env("CHUZZ_PROFILE_DIR", &profile.0)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn(&reactor.handle())
            .expect("the host binary should start"),
    );

    let stdout = host.0.stdout.take().expect("stdout was piped");
    let announced = nagoya::block_on(chuzz::headless_io::read_announcement(stdout))
        .expect("the host should announce a descriptor");
    let socket = std::path::PathBuf::from(announced.trim()).with_extension("sock");
    nagoya::block_on(async {
        use std::os::unix::ffi::OsStrExt as _;
        let addr = nagoya::reactor::Addr::path(socket.as_os_str().as_bytes())
            .expect("socket path fits a unix address");
        let socket = nagoya::reactor::TcpStream::connect(addr, &reactor.handle())
            .await
            .expect("connect async client");
        let mut stream = TransportStream::new(framed_json_neutral(NagoyaStream::new(socket)));
        let mut next_id = 0;
        let observer = nagoya::reactor::TcpStream::connect(addr, &reactor.handle())
            .await
            .expect("connect paint observer");
        let mut observer = TransportStream::new(framed_json_neutral(NagoyaStream::new(observer)));
        let mut observer_id = 0;
        observe_paint(&mut observer, &mut observer_id).await;

        let first = tree(&mut stream, &mut next_id).await;
        let link = first
            .nodes
            .iter()
            .find(|node| node.name == "Go to second")
            .unwrap_or_else(|| panic!("fixture link missing from {:?}", first.nodes))
            .id;
        assert!(
            !first.nodes.iter().any(|node| node.name == "carried"),
            "the second page is somehow already loaded"
        );

        assert!(matches!(
            request(
                &mut stream,
                &mut next_id,
                &AgentControlRequest::Act(AgentAction::Click { node_id: link }),
            )
            .await,
            DebugResponse::Ack
        ));

        // Navigation runs after the click answer. Wait for committed paint
        // events on a separate observer so Inspect replies cannot consume them.
        let carried = nagoya::timeout(Duration::from_secs(20), async {
            loop {
                let after = tree(&mut stream, &mut next_id).await;
                if let Some(node) = after.nodes.iter().find(|node| node.name == "carried") {
                    break node.value.clone().unwrap_or_default();
                }
                loop {
                    let event = observer
                        .recv()
                        .await
                        .expect("paint observer stays open")
                        .expect("read paint event");
                    if matches!(
                        decode_diagnostics_event(event),
                        Ok(DebugEvent::PaintCommitted { .. })
                    ) {
                        break;
                    }
                }
            }
        })
        .await
        .expect("clicking the link never loaded the second page");

        assert!(
            carried.contains("local=wss://saved.example"),
            "the second page booted without what the first page stored: {carried:?}"
        );
        assert!(
            carried.contains("session=first"),
            "sessionStorage did not survive the navigation: {carried:?}"
        );
        assert!(
            carried.contains("local-prototype=local-prototype"),
            "localStorage lost the __proto__ key during navigation: {carried:?}"
        );
        assert!(
            carried.contains("session-prototype=session-prototype"),
            "sessionStorage lost the __proto__ key during navigation: {carried:?}"
        );
    });
}
