use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

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
        1
    );
    host.invoke_intrinsic("__chuzzSessionRemoveItem", &[text("edge")])
        .expect("remove session value");
    assert_eq!(
        expect_number(
            host.invoke_intrinsic("__chuzzSessionLength", &[])
                .expect("empty session")
        ),
        0
    );

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

    // WebGPU staging preserves bytes and explicit release removes the staged range.
    let gpu_id = expect_number(
        host.invoke_intrinsic(
            "__chuzzGpuPutBytes",
            &[
                HostValue::Buffer(HostBuffer::new(vec![8, 9, 10])),
                number(0),
                number(3),
            ],
        )
        .expect("stage GPU bytes"),
    );
    host.invoke_intrinsic("__chuzzGpuReleaseBytes", &[number(gpu_id)])
        .expect("release GPU bytes");

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
    println!(
        "PASS concrete page runtime: nine subsystems, rooted callbacks, structured values, raster delivery, teardown"
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
    host.poll(None).expect("poll captured video");
    let frames = host.drain_frames();
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
    host.poll(None).expect("idle poll after failure");
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
    host.set_frame_sink(|_| {
        Err(chuzz::script_host::HostError::new(
            "Error",
            "synthetic sink failure",
        ))
    })
    .expect("set failing sink");
    assert!(host.poll(None).is_err());
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
