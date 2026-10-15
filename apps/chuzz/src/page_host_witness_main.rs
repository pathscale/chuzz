use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::{Duration, Instant};

mod page_host_socket_witness;

use chuzz::page_script_host::{self, NativeValue};
use chuzz::script_host::{
    ChuzzScriptHost, HostBuffer, HostFrame, HostFrameSource, HostInstallContext, HostObject,
    HostValue,
};

fn record(fields: impl IntoIterator<Item = (&'static str, HostValue)>) -> HostValue {
    HostValue::Record(
        fields
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}

fn text(value: &str) -> HostValue {
    HostValue::String(Rc::from(value))
}

fn number(value: u64) -> HostValue {
    HostValue::Number(value as f64)
}

fn json_text(value: serde_json::Value) -> HostValue {
    text(&value.to_string())
}

fn structured(value: serde_json::Value) -> HostValue {
    chuzz::script_host::from_json(value)
}

fn expect_number(value: HostValue) -> u64 {
    match value {
        HostValue::Number(value) if value.is_finite() && value >= 0.0 => value as u64,
        _ => panic!("expected native number"),
    }
}

fn expect_text(value: HostValue) -> String {
    match value {
        HostValue::String(value) => value.to_string(),
        HostValue::Utf16(value) => String::from_utf16(&value).expect("valid UTF-16 result"),
        _ => panic!("expected native text"),
    }
}

fn poll_until(
    host: &mut page_script_host::ChuzzPageRuntimeHost,
    ready: impl Fn() -> bool,
    message: &str,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "{message}");
        host.poll(None).expect("advance native host turn");
        nagoya::block_on(nagoya::sleep(Duration::from_millis(1)));
    }
}

fn send_gpu(
    host: &mut page_script_host::ChuzzPageRuntimeHost,
    request: serde_json::Value,
    uploads: Vec<u64>,
) -> Result<(), chuzz::script_host::HostError> {
    host.invoke_intrinsic(
        "__chuzzGpuSend",
        &[
            structured(request),
            HostValue::Array(uploads.into_iter().map(number).collect()),
        ],
    )?;
    Ok(())
}

fn make_host() -> (
    page_script_host::ChuzzPageRuntimeHost,
    blitz_traits::net::AbortSignal,
    Arc<blitz_dom::release::Teardown>,
) {
    let signal = blitz_traits::net::AbortSignal::default();
    let lifetime = blitz_dom::release::Teardown::new();
    let base = url::Url::parse("https://example.invalid/").expect("synthetic origin");
    let provider = chuzz::document_loader::NetProvider::new(None);
    let initiator = provider.browser_initiator(base.clone());
    let config = HostInstallContext {
        net: Arc::new(provider.for_document(base.clone(), &initiator)),
        base: Some(base),
        deadline: Duration::from_secs(2),
        wake: chuzz::net_bridge::PageWake::default(),
        lifetime: Arc::clone(&lifetime),
        signal: signal.clone(),
        session_backing: None,
    };
    (
        page_script_host::new_page_script_host(config).expect("install native page hosts"),
        signal,
        lifetime,
    )
}

fn main() {
    chuzz::install_crypto_provider();
    chuzz::set_device_audio_enabled(false);
    let (mut host, signal, lifetime) = make_host();

    // The conversion boundary retains nested structure, object identity, UTF-16
    // code units, and detached buffer state without going through JSON.
    let identity = HostObject::new(String::from("root"));
    let mut detached = HostBuffer::new(vec![1, 2, 3]);
    let _transfer = match detached.detach_with(|_| Ok::<(), ()>(())) {
        Ok(transfer) => transfer,
        Err(_) => panic!("detach fixture"),
    };
    let mut nested = BTreeMap::new();
    nested.insert(
        "surrogate".to_owned(),
        HostValue::Utf16(Rc::from([0xd800_u16])),
    );
    nested.insert("object".to_owned(), HostValue::Object(identity.clone()));
    nested.insert("bytes".to_owned(), HostValue::Buffer(detached));
    nested.insert(
        "attached".to_owned(),
        HostValue::Buffer(HostBuffer::new(vec![0, 128, 255])),
    );
    nested.insert("undefined".to_owned(), HostValue::Undefined);
    nested.insert("number".to_owned(), HostValue::Number(f64::NEG_INFINITY));
    nested.insert(
        "array".to_owned(),
        HostValue::Array(vec![HostValue::Boolean(true), HostValue::Null]),
    );
    let native: NativeValue = HostValue::Record(nested).into();
    let roundtrip: HostValue = native.into();
    let HostValue::Record(roundtrip) = roundtrip else {
        panic!("recursive conversion lost record")
    };
    assert!(
        matches!(roundtrip.get("surrogate"), Some(HostValue::Utf16(units)) if units.as_ref() == [0xd800])
    );
    assert!(
        matches!(roundtrip.get("object"), Some(HostValue::Object(value)) if value.same_identity(&identity))
    );
    assert!(
        matches!(roundtrip.get("bytes"), Some(HostValue::Buffer(value)) if value.is_detached() && value.as_bytes().is_err())
    );
    assert!(
        matches!(roundtrip.get("attached"), Some(HostValue::Buffer(value)) if value.as_bytes().expect("attached conversion") == [0,128,255])
    );
    assert!(matches!(
        roundtrip.get("undefined"),
        Some(HostValue::Undefined)
    ));
    assert!(
        matches!(roundtrip.get("number"), Some(HostValue::Number(value)) if *value == f64::NEG_INFINITY)
    );
    assert!(matches!(roundtrip.get("array"), Some(HostValue::Array(values)) if values.len() == 2));

    // Cookie storage is native and data URLs are resolved by the native fetch path.
    let cookie = expect_text(
        host.invoke_intrinsic("__chuzzCookieSet", &[text("witness=ok; Path=/")])
            .expect("set cookie"),
    );
    assert!(cookie.contains("witness=ok"));
    let data_response = host
        .invoke_intrinsic(
            "__chuzzNetSync",
            &[record([
                ("url", text("data:text/plain,witness")),
                ("method", text("GET")),
            ])],
        )
        .expect("fetch data URL");
    let HostValue::Record(response) = data_response else {
        panic!("native fetch response")
    };
    assert!(
        matches!(response.get("ok"), Some(HostValue::Boolean(true))),
        "native data fetch failed: {}",
        match response.get("error") {
            Some(HostValue::String(message)) => message.as_ref(),
            _ => "missing response error",
        }
    );
    assert!(
        matches!(response.get("bytes"), Some(HostValue::Buffer(bytes)) if bytes.as_bytes().expect("response bytes") == b"witness")
    );

    // Invalid local WebSocket connects yield native error and close events. The
    // same rooted callback remains callable across three distinct host polls.
    let deliveries = Rc::new(Cell::new(0_u32));
    let callback_root = Rc::new(String::from("socket callback root"));
    let callback_weak: Weak<String> = Rc::downgrade(&callback_root);
    let delivery_counter = Rc::clone(&deliveries);
    let persistent_capture = Rc::clone(&callback_root);
    let callback = host
        .retain_callback(move |_host, _args| {
            let _keep_root_alive = &persistent_capture;
            delivery_counter.set(delivery_counter.get() + 1);
            Ok(HostValue::Undefined)
        })
        .expect("retain WebSocket callback");
    drop(callback_root);
    assert!(callback_weak.upgrade().is_some());
    for id in 1..=3 {
        let per_socket_counter = Rc::clone(&deliveries);
        let per_socket = host
            .retain_callback(move |_host, _args| {
                per_socket_counter.set(per_socket_counter.get() + 1);
                Ok(HostValue::Undefined)
            })
            .expect("retain per-socket callback");
        let replaced_callback = host
            .retain_callback(|_host, _args| Ok(HostValue::Undefined))
            .expect("retain callback to replace");
        host.invoke_intrinsic(
            "__chuzzWsSubscribe",
            &[
                number(id),
                HostValue::Object(HostObject::new(replaced_callback.clone())),
            ],
        )
        .expect("initial socket subscription");
        host.invoke_intrinsic(
            "__chuzzWsSubscribe",
            &[
                number(id),
                HostValue::Object(HostObject::new(per_socket.clone())),
            ],
        )
        .expect("replace socket subscription");
        host.invoke_intrinsic(
            "__chuzzWsSubscribe",
            &[
                number(id),
                HostValue::Object(HostObject::new(per_socket.clone())),
            ],
        )
        .expect("repeat same socket subscription");
        assert!(host.invoke_callback(replaced_callback, &[]).is_err());
        host.invoke_intrinsic(
            "__chuzzWsConnect",
            &[number(id), HostValue::Null, HostValue::Array(Vec::new())],
        )
        .expect("queue synthetic socket failure");
        assert!(host.poll(None).expect("dispatch socket events"));
        assert!(host.callback_count() > 0);
        host.invoke_callback(callback.clone(), &[])
            .expect("persistent callback across polls");
    }
    assert!(
        deliveries.get() >= 6,
        "each invalid connect emits error and close events"
    );

    // Compression round trips binary bytes; digest returns a 32-byte buffer.
    let source = HostValue::Buffer(HostBuffer::new(vec![0, 1, 2, 127, 255]));
    let encoded = host
        .invoke_intrinsic(
            "__chuzzCodec",
            &[text("gzip"), source, HostValue::Boolean(false)],
        )
        .expect("gzip encode");
    let decoded = host
        .invoke_intrinsic(
            "__chuzzCodec",
            &[text("gzip"), encoded, HostValue::Boolean(true)],
        )
        .expect("gzip decode");
    assert!(
        matches!(decoded, HostValue::Buffer(buffer) if buffer.as_bytes().expect("decoded bytes") == [0, 1, 2, 127, 255])
    );
    let digest = host
        .invoke_intrinsic(
            "__chuzzDigest",
            &[
                text("SHA-256"),
                HostValue::Buffer(HostBuffer::new(b"witness".to_vec())),
            ],
        )
        .expect("SHA-256 digest");
    assert!(
        matches!(digest, HostValue::Buffer(buffer) if buffer.as_bytes().expect("digest bytes").len() == 32)
    );

    // Media device enumeration uses the deterministic native device inventory.
    let media = host
        .invoke_intrinsic(
            "__chuzzMediaControl",
            &[json_text(serde_json::json!({"op":"enumerate"}))],
        )
        .expect("enumerate media devices");
    assert!(expect_text(media).contains("audioinput"));

    // Native audio graph commands allocate and inspect a context.
    let audio = host
        .invoke_intrinsic(
            "__chuzzAudioCall",
            &[text(r#"{"op":"createContext","sampleRate":48000}"#)],
        )
        .expect("create offline audio context");
    let audio_json = chuzz::script_host::to_json(&audio).expect("audio context reply");
    let context = audio_json["context"].as_u64().expect("audio context id");
    let node = host
        .invoke_intrinsic(
            "__chuzzAudioCall",
            &[json_text(
                serde_json::json!({"op":"createNode","context":context,"kind":"oscillator"}),
            )],
        )
        .expect("create oscillator node");
    let node_json = chuzz::script_host::to_json(&node).expect("audio node reply");
    assert!(node_json["node"].as_u64().is_some());

    // MSE accepts and releases a valid binary upload, then opens and closes a source.
    let mse_upload = host
        .invoke_intrinsic(
            "__chuzzMsePut",
            &[
                HostValue::Buffer(HostBuffer::new(vec![0, 0, 0, 1])),
                number(0),
                number(4),
            ],
        )
        .expect("stage MSE bytes");
    let upload_id = expect_number(mse_upload);
    host.invoke_intrinsic("__chuzzMseRelease", &[number(upload_id)])
        .expect("release MSE upload");
    let opened = host
        .invoke_intrinsic(
            "__chuzzMseControl",
            &[structured(serde_json::json!({"op":"open","source":41}))],
        )
        .expect("open MSE source");
    assert!(
        matches!(opened, HostValue::Record(fields) if matches!(fields.get("ok"), Some(HostValue::Boolean(true))))
    );
    host.invoke_intrinsic(
        "__chuzzMseControl",
        &[structured(serde_json::json!({"op":"close","source":41}))],
    )
    .expect("close MSE source");

    // Peer create/close is a local native capability path and releases its callback.
    let peer_callback_hits = Rc::new(Cell::new(0_u32));
    let peer_hits = Rc::clone(&peer_callback_hits);
    let peer_callback = host
        .retain_callback(move |_host, _args| {
            peer_hits.set(peer_hits.get() + 1);
            Ok(HostValue::Undefined)
        })
        .expect("retain peer callback");
    let peer_reply = host
        .invoke_intrinsic(
            "__chuzzPeerControl",
            &[structured(
                serde_json::json!({"op":"create","configuration":{}}),
            )],
        )
        .expect("create peer");
    let peer = expect_number(peer_reply);
    let replacement_peer_callback = host
        .retain_callback(|_host, _args| Ok(HostValue::Undefined))
        .expect("retain replacement peer callback");
    host.invoke_intrinsic(
        "__chuzzPeerSubscribe",
        &[
            number(peer),
            HostValue::Object(HostObject::new(peer_callback.clone())),
        ],
    )
    .expect("subscribe peer events");
    host.invoke_intrinsic(
        "__chuzzPeerSubscribe",
        &[
            number(peer),
            HostValue::Object(HostObject::new(replacement_peer_callback.clone())),
        ],
    )
    .expect("replace peer callback");
    host.invoke_intrinsic(
        "__chuzzPeerSubscribe",
        &[
            number(peer),
            HostValue::Object(HostObject::new(replacement_peer_callback.clone())),
        ],
    )
    .expect("repeat peer subscription");
    let rooted_before_close = host.callback_count();
    host.invoke_intrinsic(
        "__chuzzPeerControl",
        &[structured(serde_json::json!({"op":"close","peer":peer}))],
    )
    .expect("close peer");
    assert_eq!(host.callback_count(), rooted_before_close - 1);
    assert!(host.poll(None).is_ok());
    assert!(host.callback_count() > 0);

    // Native 2D canvas reset/readback and paint exercise the CPU raster path and
    // enqueue a frame for the renderer sink without requiring a GPU device.
    let canvas_request = |action: &str| {
        structured(serde_json::json!({
            "action": action, "id": 77, "width": 2, "height": 2,
            "commands": []
        }))
    };
    host.invoke_intrinsic(
        "__chuzzCanvas2D",
        &[canvas_request("reset"), HostValue::Array(Vec::new())],
    )
    .expect("create native canvas");
    let pixels = host
        .invoke_intrinsic(
            "__chuzzCanvas2D",
            &[
                structured(
                    serde_json::json!({"action":"read","id":77,"width":2,"height":2,"n":[0,0,2,2]}),
                ),
                HostValue::Array(Vec::new()),
            ],
        )
        .expect("read native canvas");
    assert!(
        matches!(pixels, HostValue::Buffer(buffer) if buffer.as_bytes().expect("canvas pixels").len() == 16)
    );
    host.invoke_intrinsic(
        "__chuzzCanvas2D",
        &[canvas_request("paint"), HostValue::Array(Vec::new())],
    )
    .expect("paint native canvas");
    let frames = Rc::new(Cell::new(0_u32));
    let frame_count = Rc::clone(&frames);
    host.set_frame_sink(move |frame: HostFrame| {
        assert_eq!(frame.source, HostFrameSource::Canvas);
        assert_eq!(frame.target, 77);
        assert!(frame.image.is_some());
        frame_count.set(frame_count.get() + 1);
        Ok(())
    })
    .expect("install frame sink");
    assert!(host.poll(None).expect("deliver dirty canvas frame"));
    assert_eq!(frames.get(), 1);
    assert!(host.drain_frames().is_empty());

    // Realm scoping rejects a callback in another host. Explicit release frees
    // captured native roots, and teardown aborts the signal and retires leases.
    let (mut other, _, _) = make_host();
    assert!(other.invoke_callback(callback.clone(), &[]).is_err());
    ChuzzScriptHost::release_callback(&mut host, callback.clone())
        .expect("release socket callback");
    assert!(callback_weak.upgrade().is_none());
    assert!(host.invoke_callback(callback, &[]).is_err());
    assert!(host.invoke_callback(peer_callback, &[]).is_err());
    assert!(
        host.invoke_callback(replacement_peer_callback, &[])
            .is_err()
    );
    host.teardown_page_script_host();
    host.teardown_page_script_host();
    assert!(host.is_torn_down());
    assert!(signal.aborted());
    assert!(lifetime.complete());
    assert!(host.poll(None).is_err());
    other.teardown_page_script_host();
    frame_and_teardown_checks();
    timer_wake_checks();
    page_host_socket_witness::run();
    println!(
        "PASS concrete page runtime: storage, fetch, timers, socket frames, media, GPU presentation/readback, teardown"
    );
}

fn frame_and_teardown_checks() {
    let (mut host, signal, _) = make_host();
    let capture = host
        .invoke_intrinsic(
            "__chuzzMediaControl",
            &[json_text(serde_json::json!({
                "op":"acquire", "constraints":{"video":{"width":16,"height":16}}
            }))],
        )
        .expect("acquire synthetic video track");
    let capture: serde_json::Value =
        serde_json::from_str(&expect_text(capture)).expect("capture reply");
    assert_eq!(capture["ok"], true);
    let track = capture["value"][0]["handle"]
        .as_u64()
        .expect("capture handle");
    host.invoke_intrinsic(
        "__chuzzVideoCommand",
        &[structured(serde_json::json!({
            "op":"load", "id":91, "generation":4, "tracks":[track], "muted":true
        }))],
    )
    .expect("attach captured video");
    let mut frames = Vec::new();
    for _ in 0..4 {
        host.poll(None).expect("poll captured video turn");
        frames.extend(host.drain_frames());
    }
    assert!(frames.iter().any(|frame| {
        frame.source == HostFrameSource::Video
            && frame.target == 91
            && frame.generation == 4
            && frame
                .image
                .as_ref()
                .is_some_and(|image| !image.data.is_empty())
    }));
    let acquired = host
        .invoke_intrinsic("__chuzzVideoFrame", &[number(91), number(4)])
        .expect("read native video frame");
    assert!(matches!(acquired, HostValue::Record(_)));

    // A callback failure in an earlier subsystem cannot starve raster delivery.
    let failing = host
        .retain_callback(|_, _| {
            Err(chuzz::script_host::HostError::new(
                "Error",
                "synthetic callback failure",
            ))
        })
        .expect("retain failing callback");
    host.invoke_intrinsic(
        "__chuzzWsSubscribe",
        &[number(55), HostValue::Object(HostObject::new(failing))],
    )
    .expect("subscribe failing callback");
    host.invoke_intrinsic(
        "__chuzzWsConnect",
        &[number(55), HostValue::Null, HostValue::Array(Vec::new())],
    )
    .expect("queue callback failure");
    host.invoke_intrinsic(
        "__chuzzCanvas2D",
        &[structured(serde_json::json!({
            "action":"paint", "id":92, "width":2, "height":2, "commands":[]
        }))],
    )
    .expect("queue canvas raster");
    let delivered = Rc::new(Cell::new(0));
    let sink_count = Rc::clone(&delivered);
    host.set_frame_sink(move |frame| {
        assert_eq!(frame.target, 92);
        sink_count.set(sink_count.get() + 1);
        Ok(())
    })
    .expect("attach renderer sink");
    assert!(host.poll(None).is_err());
    assert_eq!(delivered.get(), 1);
    for _ in 0..3 {
        host.poll(None)
            .expect("idle host turn after callback failure");
    }
    assert_eq!(
        delivered.get(),
        1,
        "unchanged canvas must not be delivered twice"
    );

    // Failed sinks retain their newest frame for a later poll.
    host.deliver_frame(HostFrame {
        source: HostFrameSource::WebGpu,
        target: 93,
        generation: 7,
        image: None,
    })
    .expect("queue newer frame");
    host.deliver_frame(HostFrame {
        source: HostFrameSource::WebGpu,
        target: 93,
        generation: 6,
        image: None,
    })
    .expect("ignore stale frame");
    host.deliver_frame(HostFrame {
        source: HostFrameSource::WebGpu,
        target: 94,
        generation: 1,
        image: None,
    })
    .expect("queue independent target");
    let independent = Rc::new(Cell::new(false));
    let independent_sink = Rc::clone(&independent);
    host.set_frame_sink(move |frame| {
        if frame.target == 94 {
            independent_sink.set(true);
            return Ok(());
        }
        Err(chuzz::script_host::HostError::new(
            "Error",
            "synthetic sink failure",
        ))
    })
    .expect("set failing sink");
    assert!(host.poll(None).is_err());
    assert!(
        independent.get(),
        "a failed target must not starve another sink delivery"
    );
    let pending = host.drain_frames();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].generation, 7);
    host.deliver_frame(pending.into_iter().next().expect("pending frame"))
        .expect("retry frame");
    host.set_frame_sink(|frame| {
        assert_eq!(frame.generation, 7);
        Ok(())
    })
    .expect("replace sink");
    assert!(host.poll(None).expect("retry sink delivery"));
    assert!(host.drain_frames().is_empty());

    let captured = Rc::new(());
    let weak = Rc::downgrade(&captured);
    let id = host
        .retain_callback(move |_, _| {
            let _root = &captured;
            Ok(HostValue::Undefined)
        })
        .expect("root until teardown");
    assert!(weak.upgrade().is_some());
    host.teardown_page_script_host();
    assert!(
        weak.upgrade().is_none(),
        "surviving IDs must not retain callables"
    );
    assert!(signal.aborted());
    assert_eq!(host.callback_count(), 0);
    assert!(host.invoke_callback(id, &[]).is_err());
    assert!(host.set_frame_sink(|_| Ok(())).is_err());
    assert!(host.invoke_intrinsic("__chuzzSessionLength", &[]).is_err());
}

fn session_timer_checks(host: &mut page_script_host::ChuzzPageRuntimeHost) {
    // Seed and capture native session state through the host lifecycle API.
    let seed = [
        ("seed".to_owned(), "native".to_owned()),
        ("count".to_owned(), "3".to_owned()),
    ];
    host.seed_session(&seed).expect("seed native session");
    assert!(
        host.seed_session(&[("oversized".to_owned(), "x".repeat(8193))])
            .is_err()
    );
    assert_eq!(
        expect_text(
            host.invoke_intrinsic("__chuzzSessionGetItem", &[text("seed")])
                .expect("rejected seed preserves storage")
        ),
        "native"
    );
    assert!(
        host.capture_session(&chuzz::tab_stash::SleepingBudget::new(1))
            .expect("snapshot with no budget")
            .is_none()
    );
    let budget = chuzz::tab_stash::SleepingBudget::new(64 * 1024);
    let captured = host
        .capture_session(&budget)
        .expect("capture native session")
        .expect("session snapshot");
    assert_eq!(captured.pairs().len(), 2);
    assert!(captured.pairs().iter().any(|pair| {
        pair.key.units() == "seed".encode_utf16().collect::<Vec<_>>()
            && pair.value.units() == "native".encode_utf16().collect::<Vec<_>>()
    }));
    assert_eq!(
        expect_text(
            host.invoke_intrinsic("__chuzzSessionGetItem", &[text("seed")])
                .expect("snapshot preserves live storage")
        ),
        "native"
    );
    assert!(captured.pairs().iter().any(|pair| {
        pair.key.units() == "count".encode_utf16().collect::<Vec<_>>()
            && pair.value.units() == "3".encode_utf16().collect::<Vec<_>>()
    }));

    // Session storage carries lone UTF-16 surrogates through the native store.
    let lone = HostValue::Utf16(Rc::from([0xd800_u16, b'x' as u16]));
    host.invoke_intrinsic("__chuzzSessionSetItem", &[text("edge"), lone])
        .expect("store UTF-16 value");
    let saved = host
        .invoke_intrinsic("__chuzzSessionGetItem", &[text("edge")])
        .expect("read UTF-16 value");
    assert!(matches!(saved, HostValue::Utf16(value) if value.as_ref() == [0xd800, b'x' as u16]));
    assert_eq!(
        expect_number(
            host.invoke_intrinsic("__chuzzSessionLength", &[])
                .expect("session length")
        ),
        3
    );
    host.invoke_intrinsic("__chuzzSessionRemoveItem", &[text("edge")])
        .expect("remove session value");
    assert_eq!(
        expect_number(
            host.invoke_intrinsic("__chuzzSessionLength", &[])
                .expect("session length after edge removal")
        ),
        2
    );

    // A repeating zero-delay timer is clamped by the host and runs across turns.
    let timer_hits = Rc::new(Cell::new(0_u32));
    let timer_counter = Rc::clone(&timer_hits);
    let timer_callback = host
        .retain_callback(move |_host, _args| {
            timer_counter.set(timer_counter.get() + 1);
            Ok(HostValue::Undefined)
        })
        .expect("retain timer callback");
    let timer = host
        .schedule_timer(timer_callback.clone(), Duration::ZERO, true)
        .expect("schedule repeating timer");
    for _ in 0..3 {
        nagoya::block_on(nagoya::sleep(Duration::from_millis(2)));
        host.poll(None).expect("run timer host turn");
    }
    assert!(timer_hits.get() >= 3, "timer did not repeat across turns");
    assert!(host.cancel_timer(timer).expect("cancel repeating timer"));
    let fired = timer_hits.get();
    nagoya::block_on(nagoya::sleep(Duration::from_millis(2)));
    host.poll(None).expect("poll after cancellation");
    assert_eq!(timer_hits.get(), fired);
    ChuzzScriptHost::release_callback(host, timer_callback).expect("release timer root");

    host.invoke_intrinsic(
        "__chuzzSessionSetItem",
        &[text("secret"), text("private-value")],
    )
    .expect("set sensitive value");
    host.invoke_intrinsic("__chuzzSessionMarkSensitive", &[text("secret")])
        .expect("mark sensitive key");
    let filtered = host
        .capture_session(&budget)
        .expect("filtered snapshot")
        .expect("snapshot bucket");
    assert_eq!(filtered.pairs().len(), 2);
    assert_eq!(
        expect_text(
            host.invoke_intrinsic("__chuzzSessionGetItem", &[text("secret")])
                .expect("sensitive value remains live")
        ),
        "private-value"
    );
}

fn queued_fetch_checks(host: &mut page_script_host::ChuzzPageRuntimeHost) {
    // Async fetch completes through the queued native network path.
    let async_reply = Rc::new(Cell::new(false));
    let async_reply_sink = Rc::clone(&async_reply);
    let fetch_callback = host
        .retain_callback(move |_host, args| {
            let Some(HostValue::Record(reply)) = args.first() else {
                return Err(chuzz::script_host::HostError::new(
                    "Error",
                    "missing async fetch reply",
                ));
            };
            assert!(matches!(reply.get("ok"), Some(HostValue::Boolean(true))));
            assert!(matches!(
                reply.get("bytes"),
                Some(HostValue::Buffer(bytes))
                    if bytes.as_bytes().is_ok_and(|bytes| bytes == b"queued")
            ));
            async_reply_sink.set(true);
            Ok(HostValue::Undefined)
        })
        .expect("retain async fetch callback");
    host.invoke_intrinsic(
        "__chuzzNetAsync",
        &[
            record([
                ("url", text("data:text/plain,queued")),
                ("method", text("GET")),
            ]),
            HostValue::Object(HostObject::new(fetch_callback)),
        ],
    )
    .expect("queue async data URL fetch");
    poll_until(host, || async_reply.get(), "async fetch did not complete");
}

fn gpu_checks(host: &mut page_script_host::ChuzzPageRuntimeHost) {
    host.invoke_intrinsic("__chuzzGpuInspect", &[])
        .expect("activate native buffer inspection before allocation");
    // Exercise a real adapter, device, GPU buffer write, and native readback.
    let gpu_replies = Rc::new(std::cell::RefCell::new(
        Vec::<(u64, serde_json::Value)>::new(),
    ));
    let gpu_reply_sink = Rc::clone(&gpu_replies);
    let gpu_callback = host
        .retain_callback(move |_host, args| {
            if matches!(
                args.first(),
                Some(HostValue::String(kind)) if kind.as_ref() == "reply"
            ) {
                let id = match args.get(1) {
                    Some(HostValue::Number(value)) if value.is_finite() && *value >= 0.0 => {
                        *value as u64
                    }
                    _ => {
                        return Err(chuzz::script_host::HostError::new(
                            "Error",
                            "missing WebGPU reply id",
                        ));
                    }
                };
                let payload = args
                    .get(2)
                    .ok_or_else(|| {
                        chuzz::script_host::HostError::new("Error", "missing WebGPU reply payload")
                    })
                    .and_then(|value| {
                        chuzz::script_host::to_json(value).map_err(|error| {
                            chuzz::script_host::HostError::new("Error", error.to_string())
                        })
                    })?;
                gpu_reply_sink.borrow_mut().push((id, payload));
            }
            Ok(HostValue::Undefined)
        })
        .expect("retain WebGPU reply callback");
    host.invoke_intrinsic(
        "__chuzzGpuSubscribe",
        &[HostValue::Object(HostObject::new(gpu_callback))],
    )
    .expect("subscribe WebGPU replies");

    let request_gpu =
        |host: &mut page_script_host::ChuzzPageRuntimeHost, id: u64, request: serde_json::Value| {
            let before = gpu_replies.borrow().len();
            send_gpu(host, request, Vec::new()).expect("send WebGPU request");
            poll_until(
                host,
                || gpu_replies.borrow().len() > before,
                "WebGPU request did not complete",
            );
            let replies = gpu_replies.borrow();
            let (reply_id, payload) = replies.last().expect("WebGPU reply");
            assert_eq!(*reply_id, id);
            assert_eq!(payload["ok"], true, "WebGPU request failed: {payload}");
            payload.clone()
        };

    let adapter = request_gpu(
        host,
        101,
        serde_json::json!({"webgpu":"requestAdapter","id":101,"options":{}}),
    );
    assert!(
        adapter["value"]["handle"].as_u64().is_some(),
        "no real WebGPU adapter"
    );
    let device = request_gpu(
        host,
        102,
        serde_json::json!({
            "webgpu":"requestDevice","id":102,
            "adapter":adapter["value"]["handle"],"options":{"label":"witness-device"}
        }),
    );
    let device_id = device["value"]["handle"]
        .as_u64()
        .expect("real WebGPU device");
    request_gpu(
        host,
        103,
        serde_json::json!({
            "webgpu":"command","id":103,"device":device_id,
            "command":{"op":"createBuffer","handle":103,
                "args":{"label":"witness-readback","size":16,"usage":140,"mappedAtCreation":false}}
        }),
    );
    let upload = expect_number(
        host.invoke_intrinsic(
            "__chuzzGpuPutBytes",
            &[
                HostValue::Buffer(HostBuffer::new(vec![17, 23, 42, 91])),
                number(0),
                number(4),
            ],
        )
        .expect("stage GPU write"),
    );
    let write_request = serde_json::json!({
        "webgpu":"command","id":104,"device":device_id,
        "command":{"op":"writeBuffer","handle":103,"args":{"offset":0,"data":upload}}
    });
    let before_write = gpu_replies.borrow().len();
    send_gpu(host, write_request, vec![upload]).expect("submit real GPU buffer write");
    poll_until(
        host,
        || gpu_replies.borrow().len() > before_write,
        "GPU buffer write did not complete",
    );
    {
        let replies = gpu_replies.borrow();
        let (reply_id, payload) = replies.last().expect("GPU write reply");
        assert_eq!(*reply_id, 104);
        assert_eq!(payload["ok"], true, "GPU write failed: {payload}");
    }
    let capture_deadline = Instant::now() + Duration::from_secs(10);
    let snapshot = loop {
        match host.invoke_intrinsic(
            "__chuzzGpuCaptureBuffer",
            &[text("buffer:witness-readback"), number(0), number(16)],
        ) {
            Ok(value) => {
                break chuzz::script_host::to_json(&value).expect("GPU buffer snapshot");
            }
            Err(error) if error.name == "pending" && Instant::now() < capture_deadline => {
                host.poll(None).expect("advance GPU readback turn");
                nagoya::block_on(nagoya::sleep(Duration::from_millis(1)));
            }
            Err(error) => panic!("GPU buffer capture failed: {error}"),
        }
    };
    let encoded = snapshot["dataBase64"]
        .as_str()
        .expect("base64 GPU snapshot");
    let readback = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded)
        .expect("decode GPU snapshot");
    assert_eq!(&readback[..4], &[17, 23, 42, 91]);
    assert_eq!(&readback[4..], &[0; 12]);

    // Configure and present a real WebGPU canvas texture, then verify both
    // the delivered frame and native canvas capture contain its written pixels.
    let gpu_frames = Rc::new(std::cell::RefCell::new(Vec::<Vec<u8>>::new()));
    let frame_sink = Rc::clone(&gpu_frames);
    host.set_frame_sink(move |frame: HostFrame| {
        if frame.source == HostFrameSource::WebGpu && frame.target == 200 {
            assert_eq!(frame.generation, 1);
            let image = frame.image.expect("presented GPU frame image");
            frame_sink.borrow_mut().push(image.data.data().to_vec());
        }
        Ok(())
    })
    .expect("install GPU frame sink");
    request_gpu(
        host,
        105,
        serde_json::json!({
            "webgpu":"command","id":105,"device":device_id,
            "command":{"op":"setCanvasId","handle":200,"args":"canvas:#witness-canvas"}
        }),
    );
    request_gpu(
        host,
        106,
        serde_json::json!({
            "webgpu":"command","id":106,"device":device_id,
            "command":{"op":"configure","handle":200,
                "args":{"generation":1,"format":"rgba8unorm","usage":3,
                    "alphaMode":"opaque","viewFormats":[],"width":2,"height":2}}
        }),
    );
    for (turn, color) in [[17, 23, 42, 255], [81, 11, 49, 255], [5, 201, 33, 255]]
        .into_iter()
        .enumerate()
    {
        let expected_pixels = color.repeat(4);
        let current_id = 107 + turn as u64 * 3;
        let texture_handle = 201 + turn as u64;
        let write_id = current_id + 1;
        let present_id = write_id + 1;
        request_gpu(
            host,
            current_id,
            serde_json::json!({
                "webgpu":"command","id":current_id,"device":device_id,
                "command":{"op":"currentTexture","handle":texture_handle,
                    "args":{"canvas":200,"generation":1,"width":2,"height":2}}
            }),
        );
        let canvas_upload = expect_number(
            host.invoke_intrinsic(
                "__chuzzGpuPutBytes",
                &[
                    HostValue::Buffer(HostBuffer::new(expected_pixels.clone())),
                    number(0),
                    number(16),
                ],
            )
            .expect("stage canvas pixels"),
        );
        let canvas_write = serde_json::json!({
            "webgpu":"command","id":write_id,"device":device_id,
            "command":{"op":"writeTexture","handle":0,
                "args":{"texture":texture_handle,"mipLevel":0,"origin":[0,0,0],"aspect":"all",
                    "data":canvas_upload,"offset":0,"bytesPerRow":8,"rowsPerImage":2,
                    "size":[2,2,1]}}
        });
        let before_canvas_write = gpu_replies.borrow().len();
        send_gpu(host, canvas_write, vec![canvas_upload]).expect("write canvas texture");
        poll_until(
            host,
            || gpu_replies.borrow().len() > before_canvas_write,
            "GPU canvas write did not complete",
        );
        {
            let replies = gpu_replies.borrow();
            let (reply_id, payload) = replies.last().expect("canvas write reply");
            assert_eq!(*reply_id, write_id);
            assert_eq!(payload["ok"], true, "canvas write failed: {payload}");
        }
        let before_present = gpu_frames.borrow().len();
        request_gpu(
            host,
            present_id,
            serde_json::json!({
                "webgpu":"command","id":present_id,"device":device_id,
                "command":{"op":"present","handle":texture_handle,
                    "args":{"canvas":200,"generation":1}}
            }),
        );
        poll_until(
            host,
            || gpu_frames.borrow().len() > before_present,
            "GPU frame was not delivered",
        );
        assert_eq!(
            gpu_frames.borrow().last().expect("presented frame"),
            &expected_pixels
        );
        let image = chuzz::script_host::to_json(
            &host
                .invoke_intrinsic("__chuzzGpuCapture", &[text("canvas:#witness-canvas")])
                .expect("capture presented GPU canvas"),
        )
        .expect("GPU canvas capture metadata");
        assert_eq!(image["width"], 2);
        assert_eq!(image["height"], 2);
        let capture_bytes = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            image["rgbaBase64"]
                .as_str()
                .expect("base64 GPU canvas capture"),
        )
        .expect("decode GPU canvas capture");
        assert_eq!(capture_bytes, expected_pixels);
        assert_eq!(
            expect_text(
                host.invoke_intrinsic("__chuzzSessionGetItem", &[text("seed")])
                    .expect("session persists across GPU turns")
            ),
            "native"
        );
    }
}

fn composite_turns(host: &mut page_script_host::ChuzzPageRuntimeHost) {
    session_timer_checks(host);
    queued_fetch_checks(host);

    let capture = host
        .invoke_intrinsic(
            "__chuzzMediaControl",
            &[json_text(serde_json::json!({
                "op":"acquire", "constraints":{"video":{"width":16,"height":16}}
            }))],
        )
        .expect("acquire composite media track");
    let capture: serde_json::Value =
        serde_json::from_str(&expect_text(capture)).expect("capture metadata");
    assert_eq!(capture["ok"], true);
    let track = capture["value"][0]["handle"].as_u64().expect("video track");
    host.invoke_intrinsic(
        "__chuzzVideoCommand",
        &[structured(serde_json::json!({
            "op":"load", "id":91, "generation":4, "tracks":[track], "muted":true
        }))],
    )
    .expect("load composite video");
    host.invoke_intrinsic(
        "__chuzzVideoCommand",
        &[structured(serde_json::json!({
            "op":"play", "id":91, "generation":4, "muted":true
        }))],
    )
    .expect("play composite video across turns");
    let mut video_frames = 0;
    for _ in 0..3 {
        nagoya::block_on(nagoya::sleep(Duration::from_millis(40)));
        host.poll(None).expect("advance composite media turn");
        for frame in host.drain_frames() {
            assert_eq!(frame.source, HostFrameSource::Video);
            assert_eq!((frame.target, frame.generation), (91, 4));
            assert!(frame.image.is_some_and(|image| !image.data.is_empty()));
            video_frames += 1;
        }
    }
    assert!(
        video_frames >= 2,
        "media must advance across interactive turns"
    );
    gpu_checks(host);
    assert_eq!(
        expect_text(
            host.invoke_intrinsic("__chuzzSessionGetItem", &[text("seed")])
                .expect("session survives composite workflow")
        ),
        "native"
    );
}

fn timer_wake_checks() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct WakeCount(AtomicUsize);
    impl std::task::Wake for WakeCount {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    let (mut host, _, _) = make_host();
    let (mut other, _, _) = make_host();
    let hits = Rc::new(Cell::new(0));
    let counter = Rc::clone(&hits);
    let root = Rc::new(());
    let weak = Rc::downgrade(&root);
    let callback = host
        .retain_callback(move |_, _| {
            let _keep_alive = &root;
            counter.set(counter.get() + 1);
            Ok(HostValue::Undefined)
        })
        .expect("retain wake callback");
    assert!(
        other
            .schedule_timer(callback.clone(), Duration::ZERO, false)
            .is_err()
    );
    let wakes = Arc::new(WakeCount(AtomicUsize::new(0)));
    let waker = std::task::Waker::from(Arc::clone(&wakes));
    let timer = host
        .schedule_timer(callback.clone(), Duration::from_millis(10), true)
        .expect("schedule wake-driven interval");
    assert!(host.next_timer_deadline_ns().is_some());
    host.poll(Some(&waker)).expect("arm initial deadline");
    for expected in 1..=2 {
        nagoya::block_on(nagoya::sleep(Duration::from_millis(20)));
        assert!(
            wakes.0.swap(0, Ordering::Relaxed) > 0,
            "each deadline must wake the host"
        );
        host.poll(Some(&waker))
            .expect("dispatch and rearm interval");
        assert_eq!(hits.get(), expected);
    }
    assert!(host.cancel_timer(timer).expect("cancel armed deadline"));
    assert!(!host.cancel_timer(timer).expect("cancel absent deadline"));
    assert!(host.next_timer_deadline_ns().is_none());
    let released_timer = host
        .schedule_timer(callback.clone(), Duration::from_secs(1), false)
        .expect("schedule timer owned by callback");
    ChuzzScriptHost::release_callback(&mut host, callback.clone()).expect("release callback root");
    assert!(host.next_timer_deadline_ns().is_none());
    assert!(
        !host
            .cancel_timer(released_timer)
            .expect("callback release cancels its timer")
    );
    assert!(weak.upgrade().is_none());
    assert!(
        host.schedule_timer(callback, Duration::ZERO, false)
            .is_err()
    );

    let failure = host
        .retain_callback(|_, _| {
            Err(chuzz::script_host::HostError::new(
                "Error",
                "synthetic timer failure",
            ))
        })
        .expect("retain failing timer");
    let success_hits = Rc::new(Cell::new(0));
    let success_counter = Rc::clone(&success_hits);
    let success = host
        .retain_callback(move |_, _| {
            success_counter.set(success_counter.get() + 1);
            Ok(HostValue::Undefined)
        })
        .expect("retain following timer");
    host.schedule_timer(failure, Duration::ZERO, false)
        .expect("queue failing timer");
    host.schedule_timer(success, Duration::ZERO, false)
        .expect("queue following timer");
    assert!(host.poll(None).is_err());
    assert_eq!(
        success_hits.get(),
        1,
        "timer failure must not starve later callbacks"
    );
    assert!(host.next_timer_deadline_ns().is_none());
    host.poll(None).expect("failed one-shot must be removed");
    assert_eq!(success_hits.get(), 1);
}
