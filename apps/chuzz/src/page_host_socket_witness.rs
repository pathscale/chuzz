use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use chuzz::page_script_host::{self, ChuzzPageRuntimeHost};
use chuzz::script_host::{HostCallbackId, HostInstallContext, HostValue};
use nago_wss::proto::message::{Limits, Message};
use nagoya::net::TcpListener;
use nagoya::reactor::{Addr, Reactor};
use url::Url;

const SOCKET_ID: u64 = 7;
const DEADLINE: Duration = Duration::from_secs(30);

#[derive(Clone, Debug)]
struct Event {
    kind: String,
    data: String,
}

fn field_text(value: &HostValue, key: &str) -> String {
    match value {
        HostValue::Record(fields) => match fields.get(key) {
            Some(HostValue::String(value)) => value.to_string(),
            Some(HostValue::Utf16(value)) => String::from_utf16(value).expect("valid event text"),
            _ => String::new(),
        },
        _ => String::new(),
    }
}

fn make_host(base: Url) -> ChuzzPageRuntimeHost {
    let signal = blitz_traits::net::AbortSignal::default();
    let lifetime = blitz_dom::release::Teardown::new();
    let provider = chuzz::document_loader::NetProvider::new(None);
    let initiator = provider.browser_initiator(base.clone());
    let config = HostInstallContext {
        net: Arc::new(provider.for_document(base.clone(), &initiator)),
        base: Some(base),
        deadline: DEADLINE,
        wake: chuzz::net_bridge::PageWake::default(),
        lifetime,
        signal,
        session_backing: None,
    };
    page_script_host::new_page_script_host(config).expect("install native page hosts")
}

async fn wait_for(
    host: &mut ChuzzPageRuntimeHost,
    events: &Rc<RefCell<Vec<Event>>>,
    predicate: impl Fn(&[Event]) -> bool,
) {
    let started = std::time::Instant::now();
    loop {
        host.poll(None).expect("poll native page host");
        if predicate(&events.borrow()) {
            return;
        }
        assert!(
            started.elapsed() < DEADLINE,
            "timed out waiting for socket event"
        );
        nagoya::time::sleep(Duration::from_millis(2)).await;
    }
}

async fn serve_one(listener: TcpListener) -> String {
    let (stream, _) = listener.accept().await.expect("accept loopback socket");
    let outcome = nago_wss::upgrade::accept(stream, Limits::default(), &[], |_| (None, Vec::new()))
        .await
        .expect("accept WebSocket upgrade");
    let nago_wss::upgrade::UpgradeOutcome::Upgraded(mut connection, request) = outcome else {
        panic!("expected WebSocket upgrade");
    };
    assert_eq!(request.path, "/socket");

    let received = match connection.read().await.expect("read client frame") {
        Some(Message::Text(bytes)) => {
            String::from_utf8(bytes.to_vec()).expect("client frame is UTF-8")
        }
        _ => panic!("expected a text frame from the page"),
    };
    connection
        .write(Message::Text(Bytes::copy_from_slice(received.as_bytes())))
        .await
        .expect("write echo frame");

    match connection.read().await.expect("read close frame") {
        Some(Message::Close(frame)) => {
            connection.close(frame).await.expect("reply to close frame");
        }
        _ => panic!("expected close frame from the page"),
    }
    received
}

async fn drive_host(host: &mut ChuzzPageRuntimeHost, url: String) {
    let events = Rc::new(RefCell::new(Vec::<Event>::new()));
    let callback_events = Rc::clone(&events);
    let callback: HostCallbackId = host
        .retain_callback(move |_host, args| {
            let event = args.first().expect("socket event payload");
            callback_events.borrow_mut().push(Event {
                kind: field_text(event, "type"),
                data: field_text(event, "data"),
            });
            Ok(HostValue::Undefined)
        })
        .expect("retain socket event callback");

    host.invoke_intrinsic(
        "__chuzzWsSubscribe",
        &[
            HostValue::Number(SOCKET_ID as f64),
            HostValue::Object(chuzz::script_host::HostObject::new(callback.clone())),
        ],
    )
    .expect("subscribe socket callback");
    host.invoke_intrinsic(
        "__chuzzWsConnect",
        &[
            HostValue::Number(SOCKET_ID as f64),
            HostValue::String(Rc::from(url)),
            HostValue::Array(Vec::new()),
        ],
    )
    .expect("connect loopback WebSocket");

    // Turn one: the native host reports a completed opening handshake.
    wait_for(host, &events, |events| {
        events.iter().any(|event| event.kind == "open")
    })
    .await;

    host.invoke_intrinsic(
        "__chuzzWsSend",
        &[
            HostValue::Number(SOCKET_ID as f64),
            HostValue::String(Rc::from("native-frame")),
        ],
    )
    .expect("send text frame");

    // The echo is in flight while storage, timers, fetch, media and GPU work
    // progress on this same host. Their polls also dispatch the socket frame.
    super::composite_turns(host);

    // Turn two: only a frame received through the socket can satisfy this.
    wait_for(host, &events, |events| {
        events
            .iter()
            .any(|event| event.kind == "message" && event.data == "native-frame")
    })
    .await;

    let callback_count_before_close = host.callback_count();

    host.invoke_intrinsic(
        "__chuzzWsClose",
        &[
            HostValue::Number(SOCKET_ID as f64),
            HostValue::Number(1000.0),
        ],
    )
    .expect("close WebSocket");

    // Turn three: close delivery also retires the native callback root.
    wait_for(host, &events, |events| {
        events.iter().any(|event| event.kind == "close")
    })
    .await;
    assert_eq!(
        host.callback_count(),
        callback_count_before_close - 1,
        "close must retire the socket callback root"
    );
    assert!(host.invoke_callback(callback, &[]).is_err());
}

pub fn run() {
    chuzz::install_crypto_provider();

    let reactor = Reactor::start().expect("start socket reactor");
    let listener =
        TcpListener::bind(Addr::localhost(0), &reactor.handle()).expect("bind loopback listener");
    let port = listener.local_addr().expect("listener address").port();
    let base = Url::parse(&format!("http://127.0.0.1:{port}/")).expect("loopback HTTP origin");
    let socket_url = format!("ws://127.0.0.1:{port}/socket");
    let mut host = make_host(base);

    let mut server =
        nagoya::spawn(async move { nagoya::timeout(DEADLINE, serve_one(listener)).await });
    let result = nagoya::block_on(async {
        nagoya::time::timeout(DEADLINE, async {
            drive_host(&mut host, socket_url).await;
            (&mut server).await.expect("socket server task completed")
        })
        .await
    });
    if result.is_err() {
        server.cancel();
    }
    let received = result
        .expect("socket witness deadline")
        .expect("server deadline");
    assert_eq!(
        received, "native-frame",
        "server must receive the sent frame"
    );

    host.teardown_page_script_host();
    assert!(host.is_torn_down());
    reactor.shutdown();
}
