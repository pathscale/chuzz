use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use blitz_dom::Document as _;
use blitz_script::ScriptDocument;
use blitz_script::boa_engine::{JsString, JsValue};

use crate::document_loader::NetProvider;

use super::{Command, Entry, Host, MediaError};

pub(super) fn publish(entry: &mut Entry, document: &mut ScriptDocument, id: u64) {
    let network_state = if entry.error.is_some() {
        3
    } else if entry.decoder.is_none() || entry.enough || entry.paused && entry.metadata {
        1
    } else {
        2
    };
    let payload = serde_json::json!({
        "src": entry.src,
        "duration": entry.duration.filter(|duration| duration.is_finite()),
        "durationInfinite": entry.duration == Some(f64::INFINITY),
        "frameInk": entry.pixels.as_chunks::<4>().0.iter().any(|pixel| {
            pixel[..3].iter().any(|channel| *channel != 0)
        }),
        "time": entry.position,
        "paused": entry.paused,
        "ended": entry.ended,
        "seeking": entry.seeking,
        "readyState": entry.ready_state,
        "networkState": network_state,
        "buffered": entry.buffered,
        "seekable": entry.seekable,
        "hasFrame": entry.has_frame,
        "totalVideoFrames": entry.total_frames,
        "width": entry.width,
        "height": entry.height,
        "error": entry.error,
        "events": std::mem::take(&mut entry.events),
    });
    entry.dirty = false;
    document.eval(&format!(
        "globalThis.__chuzzVideoUpdate({id}, {}, {payload});",
        entry.generation
    ));
}

pub(super) fn poll(host: &mut Host, document: &mut ScriptDocument) -> bool {
    let now = Instant::now();
    let loaded_nodes = document.with_js_context(|context| {
        let requests = blitz_script::media::take_load_requests(context);
        let mut nodes = HashMap::new();
        if requests.is_empty() {
            return nodes;
        }
        let load = context
            .global_object()
            .get(JsString::from("__chuzzVideoLoad"), context)
            .ok()
            .and_then(|value| value.as_object())
            .filter(|function| function.is_callable());
        let Some(load) = load else {
            eprintln!("Media load hook is unavailable");
            return nodes;
        };
        for request in requests {
            let node = request.node_id;
            match load.call(
                &JsValue::undefined(),
                &[request.element.clone().into()],
                context,
            ) {
                Ok(value) => {
                    if let Some(id) = value.as_number() {
                        nodes.insert(id as u64, node);
                    }
                }
                Err(error) => eprintln!("Media load request: {error}"),
            }
        }
        nodes
    });
    // Called natively: a script eval here would request a redraw on every
    // poll, and the redraw polls again, so an idle page never stopped painting.
    let commands = document
        .with_js_context(|context| {
            let drain = context
                .global_object()
                .get(JsString::from("__chuzzVideoDrain"), context)
                .ok()?
                .as_object()
                .filter(|function| function.is_callable())?;
            let value = drain
                .call(&JsValue::undefined(), &[], context)
                .inspect_err(|error| eprintln!("Media drain: {error}"))
                .ok()?;
            if let Err(error) = context.run_jobs() {
                eprintln!("Media drain jobs: {error}");
            }
            value.to_json(context).ok().flatten()
        })
        .and_then(|value| serde_json::from_value::<Vec<Command>>(value).ok())
        .unwrap_or_default();
    let mut changed = !loaded_nodes.is_empty();

    for command in commands {
        if command.op == "load" || command.op == "release" {
            let existing_node = host.entries.get(&command.id).map(|entry| entry.node);
            host.frames.remove(command.id);
            changed |= host.streams.remove(document, command.id);
            if let Some(mut previous) = host.entries.remove(&command.id) {
                let _ = super::set_image(document, previous.node, None, &mut previous.pixels);
                changed = true;
            }
            if command.op == "release" {
                continue;
            }
            if command.tracks.is_some() {
                host.streams.load(&command, now);
                changed = true;
                continue;
            }
            if command.src.as_deref().is_none_or(str::is_empty) {
                continue;
            }
            let selector = format!(
                "video[data-chuzz-video='{}'],audio[data-chuzz-video='{}']",
                command.id, command.id
            );
            let node = loaded_nodes
                .get(&command.id)
                .copied()
                .or(existing_node)
                .or_else(|| document.inner().query_selector(&selector).ok().flatten());
            let Some(node) = node else {
                continue;
            };
            let url = document
                .inner()
                .url()
                .join(command.src.as_deref().unwrap_or_default());
            let Ok(url) = url else {
                continue;
            };
            let mut entry = Entry::new(node, url, &command, now);
            if let Some(source) = command.mse {
                entry.requested = true;
                match host.mse.decoder(source) {
                    Ok(decoder) => {
                        entry.decoder = Some(decoder);
                        entry.configure_decoder();
                    }
                    Err(error) => entry.fail(error),
                }
            } else if !matches!(entry.url.scheme(), "http" | "https" | "file" | "data") {
                entry.fail(MediaError::unsupported("unsupported video URL"));
            }
            host.entries.insert(command.id, entry);
            continue;
        }

        if host.streams.command(&command) {
            continue;
        }
        if let Some(entry) = host.entries.get_mut(&command.id)
            && entry.generation == command.generation
            && let Err(error) = entry.command(&command)
        {
            entry.fail(error);
        }
    }

    let frames = &host.frames;
    host.entries.retain(|id, entry| {
        let live = document.inner().get_node(entry.node).is_some();
        if !live {
            frames.remove(*id);
        }
        live
    });
    for (&id, entry) in &mut host.entries {
        match entry.tick(&host.net, now) {
            Ok(Some(frame)) => {
                let result = super::set_image(document, entry.node, Some(frame), &mut entry.pixels)
                    .and_then(|()| {
                        host.frames
                            .publish(
                                id,
                                entry.generation,
                                entry.width,
                                entry.height,
                                Arc::clone(&entry.pixels),
                            )
                            .map_err(MediaError::decode)
                    });
                match result {
                    Ok(()) => entry.total_frames += 1,
                    Err(error) => {
                        host.frames.remove(id);
                        entry.fail(error);
                    }
                }
                changed = true;
            }
            Ok(None) => {}
            Err(error) => entry.fail(error),
        }
        if entry.dirty || !entry.events.is_empty() {
            entry.publish(document, id);
            changed = true;
        }
    }
    changed | host.streams.poll(document, now)
}

pub(super) fn install(document: &mut ScriptDocument, net: Arc<NetProvider>) {
    let mse = super::mse::install(document);
    let (capture, frames) = document.with_js_context(|context| {
        let capture = crate::media::host(context).expect("page capture host");
        let frames = super::frame_registry(context);
        super::stream::install_frame_bridge(context);
        (capture, frames)
    });
    document.eval("globalThis.__chuzzVideoSupported = true;");
    document.eval_named(include_str!("video.ts"), "<chuzz-video>");
    document.eval_named(include_str!("video/mse.ts"), "<chuzz-mse>");
    document.eval_named(include_str!("video/stream.ts"), "<chuzz-stream>");
    let mut host = Host {
        net,
        mse,
        streams: super::stream::Host::new(capture, Arc::clone(&frames)),
        frames,
        entries: HashMap::new(),
    };
    document.add_poll_hook(move |document, _| poll(&mut host, document));
}
