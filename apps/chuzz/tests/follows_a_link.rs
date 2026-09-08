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

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tauri_runtime_blitz::control_protocol::{
    AgentAction, AgentControlRequest, DebugResponse, JsonRpcId, MessageStream, TransportStream,
    decode_response, encode_agent_request, framed_json,
};

/// Kill the host however the test ends, including on a panic.
struct Host(std::process::Child);

impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
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
        let message = tokio::time::timeout(Duration::from_secs(10), stream.recv())
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

async fn tree(
    stream: &mut dyn MessageStream,
    next_id: &mut i64,
) -> tauri_runtime_blitz::control_protocol::AgentSnapshot {
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

    let mut host = Host(
        Command::new(binary)
            .env("QA_INSPECT_PAGE", site)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("the host binary should start"),
    );

    let stdout = host.0.stdout.take().expect("stdout was piped");
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(stdout).read_line(&mut line);
        let _ = sender.send(line);
    });
    let announced = receiver
        .recv_timeout(Duration::from_secs(60))
        .expect("the host should announce a descriptor");
    let socket = std::path::PathBuf::from(announced.trim()).with_extension("sock");

    // The host writes the descriptor before it binds, so a connection can lose
    // that race.
    let deadline = Instant::now() + Duration::from_secs(30);
    while std::os::unix::net::UnixStream::connect(&socket).is_err() {
        assert!(
            Instant::now() < deadline,
            "the inspection socket never accepted a connection at {}",
            socket.display()
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    runtime.block_on(async {
        let socket = tokio::net::UnixStream::connect(&socket)
            .await
            .expect("connect async client");
        let mut stream = TransportStream::new(framed_json(socket));
        let mut next_id = 0;

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

        /*
         * The load happens after the click is answered, so the reply is timed
         * as the click rather than as a page load. A caller inspects again
         * before it asserts anything, which is what this does -- with a retry,
         * because the next document is fetched and parsed rather than swapped
         * in.
         */
        let deadline = Instant::now() + Duration::from_secs(20);
        let carried = loop {
            let after = tree(&mut stream, &mut next_id).await;
            if let Some(node) = after.nodes.iter().find(|node| node.name == "carried") {
                break node.value.clone().unwrap_or_default();
            }
            assert!(
                Instant::now() < deadline,
                "clicking the link never loaded the second page; the tree is still {:?}",
                after.nodes
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        };

        assert!(
            carried.contains("local=wss://saved.example"),
            "the second page booted without what the first page stored: {carried:?}"
        );
        assert!(
            carried.contains("session=first"),
            "sessionStorage did not survive the navigation: {carried:?}"
        );
    });
}
