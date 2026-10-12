//! Page-owned Web Audio host.
//!
//! One nagoya task drives each context's render clock. Codecs and processing
//! functions remain synchronous. Only metadata completions reach Boa.

mod analysis;
mod graph;
pub mod pcm;
mod resample;

use std::collections::{BTreeMap, VecDeque};
use std::future::{Future, poll_fn};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Poll, Waker};
use std::time::{Duration, Instant};

use blitz_script::ScriptDocument;
use blitz_script::boa_engine::object::builtins::{AlignedVec, JsArrayBuffer};
use blitz_script::boa_engine::{
    Context, JsNativeError, JsResult, JsString, JsValue, NativeFunction, Source,
};
use serde::Deserialize;
use serde_json::{Value, json};

use graph::{Event, Graph, QUANTUM};
use pcm::{Track, lock};

const CONTEXTS: usize = 16;
const TRACKS: usize = 512;

struct Engine {
    graph: Mutex<Graph>,
    reader: Mutex<Option<Waker>>,
}

impl Engine {
    fn wake(&self) {
        if let Some(reader) = lock(&self.reader).take() {
            reader.wake();
        }
    }
}

#[derive(Default)]
struct State {
    next_context: u64,
    next_track: u64,
    contexts: BTreeMap<u64, Arc<Engine>>,
    tracks: BTreeMap<u64, Arc<Track>>,
}

struct Page {
    epoch: Instant,
    state: Mutex<State>,
    events: Mutex<VecDeque<(u64, u64)>>,
    reader: Mutex<Option<Waker>>,
}

impl Page {
    fn engine(&self, id: u64) -> Result<Arc<Engine>, String> {
        lock(&self.state)
            .contexts
            .get(&id)
            .cloned()
            .ok_or_else(|| "InvalidAccessError: unknown AudioContext".to_owned())
    }

    fn track(&self, id: u64) -> Result<Arc<Track>, String> {
        lock(&self.state)
            .tracks
            .get(&id)
            .cloned()
            .ok_or_else(|| "InvalidAccessError: unknown audio track".to_owned())
    }

    fn register_track(&self, track: Arc<Track>) -> Result<u64, String> {
        let mut state = lock(&self.state);
        state.tracks.retain(|_, track| !track.ended());
        if state.tracks.len() == TRACKS {
            return Err("QuotaExceededError: audio track limit".to_owned());
        }
        state.next_track += 1;
        let id = state.next_track;
        state.tracks.insert(id, track);
        Ok(id)
    }

    fn ended(&self, context: u64, nodes: Vec<u64>) {
        if nodes.is_empty() {
            return;
        }
        lock(&self.events).extend(nodes.into_iter().map(|node| (context, node)));
        if let Some(reader) = lock(&self.reader).take() {
            reader.wake();
        }
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        let state = lock(&self.state);
        for engine in state.contexts.values() {
            lock(&engine.graph).close();
            engine.wake();
        }
        for track in state.tracks.values() {
            track.end_source();
        }
    }
}

#[derive(Clone)]
struct PageHost(Arc<Page>);

fn native_error(message: String) -> blitz_script::boa_engine::JsError {
    JsNativeError::typ().with_message(message).into()
}

fn page(context: &Context) -> JsResult<Arc<Page>> {
    context
        .get_data::<PageHost>()
        .map(|host| Arc::clone(&host.0))
        .ok_or_else(|| native_error("InvalidStateError: audio host is unavailable".to_owned()))
}

/// Capture hosts register their shared PCM track, then attach the returned
/// identifier to the JS track through __chuzzAudioAttachTrack.
pub fn register_track(context: &Context, track: Arc<Track>) -> JsResult<u64> {
    page(context)?.register_track(track).map_err(native_error)
}

/// Lease a graph output for the media host without copying its PCM history.
pub(crate) fn registered_track(context: &Context, id: u64) -> JsResult<Arc<Track>> {
    page(context)?.track(id).map_err(native_error)
}

/// The same timestamp origin must be used for capture and decoded PCM.
pub fn media_epoch(context: &Context) -> JsResult<Instant> {
    Ok(page(context)?.epoch)
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
enum Command {
    CreateContext {
        sample_rate: u32,
    },
    CreateNode {
        context: u64,
        kind: String,
        track: Option<u64>,
    },
    Connect {
        context: u64,
        from: u64,
        to: u64,
        param: Option<String>,
    },
    Disconnect {
        context: u64,
        from: u64,
        to: Option<u64>,
        param: Option<String>,
    },
    Param {
        context: u64,
        node: u64,
        name: String,
        event: Event,
    },
    ParamValue {
        context: u64,
        node: u64,
        name: String,
    },
    Cancel {
        context: u64,
        node: u64,
        name: String,
        time: f64,
        hold: bool,
    },
    Wave {
        context: u64,
        node: u64,
        wave: String,
    },
    Schedule {
        context: u64,
        node: u64,
        start: bool,
        time: f64,
    },
    Configure {
        context: u64,
        node: u64,
        fft_size: usize,
        min_decibels: f64,
        max_decibels: f64,
        smoothing: f64,
    },
    State {
        context: u64,
        state: String,
    },
    Inspect {
        context: u64,
    },
    Track {
        track: u64,
        action: String,
        enabled: Option<bool>,
    },
}

fn track_metadata(id: u64, track: &Track) -> Value {
    json!({
        "track": id,
        "sampleRate": track.sample_rate(),
        "channelCount": track.channels(),
        "enabled": track.enabled(),
        "ended": track.ended(),
    })
}

fn execute(page: &Arc<Page>, command: Command) -> Result<Value, String> {
    match command {
        Command::CreateContext { sample_rate } => {
            if !(8_000..=96_000).contains(&sample_rate) {
                return Err("NotSupportedError: unsupported AudioContext sample rate".to_owned());
            }
            let mut state = lock(&page.state);
            if state
                .contexts
                .values()
                .filter(|engine| !lock(&engine.graph).closed)
                .count()
                >= CONTEXTS
            {
                return Err("QuotaExceededError: AudioContext limit".to_owned());
            }
            state.next_context += 1;
            let id = state.next_context;
            let engine = Arc::new(Engine {
                graph: Mutex::new(Graph::new(id, sample_rate)),
                reader: Mutex::new(None),
            });
            state.contexts.insert(id, Arc::clone(&engine));
            drop(state);
            drop(nagoya::spawn(run(Arc::downgrade(&engine), Arc::downgrade(page))));
            Ok(json!({"context": id, "destination": 1, "sampleRate": sample_rate}))
        }
        Command::CreateNode {
            context,
            kind,
            track,
        } => {
            let engine = page.engine(context)?;
            let track = track.map(|id| page.track(id)).transpose()?;
            let mut state = lock(&page.state);
            state.tracks.retain(|_, track| !track.ended());
            if kind == "streamDestination" && state.tracks.len() == TRACKS {
                return Err("QuotaExceededError: audio track limit".to_owned());
            }
            let (node, output) = lock(&engine.graph).create(&kind, track)?;
            let mut result = json!({"node": node});
            if let Some(track) = output {
                state.next_track += 1;
                let id = state.next_track;
                result["audio"] = track_metadata(id, &track);
                state.tracks.insert(id, track);
            }
            Ok(result)
        }
        Command::Connect {
            context,
            from,
            to,
            param,
        } => {
            lock(&page.engine(context)?.graph).connect(from, to, param)?;
            Ok(Value::Null)
        }
        Command::Disconnect {
            context,
            from,
            to,
            param,
        } => {
            lock(&page.engine(context)?.graph).disconnect(from, to, param)?;
            Ok(Value::Null)
        }
        Command::Param {
            context,
            node,
            name,
            event,
        } => {
            lock(&page.engine(context)?.graph).automate(node, &name, event)?;
            Ok(Value::Null)
        }
        Command::ParamValue {
            context,
            node,
            name,
        } => Ok(json!(
            lock(&page.engine(context)?.graph).parameter_value(node, &name)?
        )),
        Command::Cancel {
            context,
            node,
            name,
            time,
            hold,
        } => {
            lock(&page.engine(context)?.graph).cancel(node, &name, time, hold)?;
            Ok(Value::Null)
        }
        Command::Wave {
            context,
            node,
            wave,
        } => {
            lock(&page.engine(context)?.graph).wave(node, wave)?;
            Ok(Value::Null)
        }
        Command::Schedule {
            context,
            node,
            start,
            time,
        } => {
            lock(&page.engine(context)?.graph).schedule(node, start, time)?;
            Ok(Value::Null)
        }
        Command::Configure {
            context,
            node,
            fft_size,
            min_decibels,
            max_decibels,
            smoothing,
        } => {
            lock(&page.engine(context)?.graph).configure(
                node,
                fft_size,
                min_decibels,
                max_decibels,
                smoothing,
            )?;
            Ok(Value::Null)
        }
        Command::State { context, state } => {
            let engine = page.engine(context)?;
            let mut graph = lock(&engine.graph);
            if graph.closed {
                return Err("InvalidStateError: AudioContext is closed".to_owned());
            }
            match state.as_str() {
                "running" => graph.running = true,
                "suspended" => graph.running = false,
                "closed" => graph.close(),
                _ => return Err("NotSupportedError: invalid AudioContext state".to_owned()),
            }
            let mut result = graph.inspect();
            drop(graph);
            if state == "closed" {
                result["endedTracks"] = json!(
                    lock(&page.state)
                        .tracks
                        .iter()
                        .filter(|(_, track)| {
                            track
                                .buffer
                                .producer
                                .is_some_and(|producer| producer.0 == context)
                        })
                        .map(|(&id, _)| id)
                        .collect::<Vec<_>>()
                );
            }
            engine.wake();
            Ok(result)
        }
        Command::Inspect { context } => Ok(lock(&page.engine(context)?.graph).inspect()),
        Command::Track {
            track,
            action,
            enabled,
        } => {
            let original = page.track(track)?;
            match action.as_str() {
                "inspect" => {}
                "enabled" => {
                    original.set_enabled(enabled.ok_or("TypeError: missing track enabled value")?)
                }
                "stop" => original.stop(),
                "clone" => {
                    let clone = original.fork();
                    let id = page.register_track(Arc::clone(&clone))?;
                    return Ok(track_metadata(id, &clone));
                }
                _ => return Err("NotSupportedError: unsupported audio track operation".to_owned()),
            }
            Ok(track_metadata(track, &original))
        }
    }
}

fn command(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let page = page(context)?;
    let message = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    if message.len() > 65_536 {
        return Err(native_error(
            "QuotaExceededError: audio command limit".to_owned(),
        ));
    }
    let command = serde_json::from_str(&message)
        .map_err(|error| native_error(format!("TypeError: invalid audio command: {error}")))?;
    let value = execute(&page, command).map_err(native_error)?;
    JsValue::from_json(&value, context)
}

fn integer(args: &[JsValue], index: usize, context: &mut Context) -> JsResult<u64> {
    let value = args
        .get(index)
        .cloned()
        .unwrap_or_default()
        .to_number(context)?;
    if !value.is_finite() || value <= 0.0 || value.fract() != 0.0 || value > 9_007_199_254_740_991.0
    {
        return Err(native_error(
            "TypeError: invalid audio identifier".to_owned(),
        ));
    }
    Ok(value as u64)
}

fn read(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let page = page(context)?;
    let context_id = integer(args, 0, context)?;
    let node = integer(args, 1, context)?;
    let kind = args
        .get(2)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let bytes = lock(&page.engine(context_id).map_err(native_error)?.graph)
        .analysis_bytes(node, &kind)
        .map_err(native_error)?;
    // This owned readback is constructed on the document thread. No Boa
    // allocation or borrow is retained by the render task.
    Ok(JsArrayBuffer::from_byte_block(AlignedVec::from_iter(0, bytes), context)?.into())
}

async fn run(engine: Weak<Engine>, page: Weak<Page>) {
    loop {
        let Some(engine) = engine.upgrade() else {
            return;
        };
        let running = poll_fn(|context| {
            let graph = lock(&engine.graph);
            if graph.closed {
                Poll::Ready(false)
            } else if graph.running {
                Poll::Ready(true)
            } else {
                *lock(&engine.reader) = Some(context.waker().clone());
                Poll::Pending
            }
        })
        .await;
        if !running {
            return;
        }

        let rate = lock(&engine.graph).sample_rate;
        let quantum = Duration::from_secs_f64(QUANTUM as f64 / f64::from(rate));
        let mut deadline = Instant::now();
        loop {
            let mut sleep = Box::pin(nagoya::sleep(
                (deadline + quantum).saturating_duration_since(Instant::now()),
            ));
            let elapsed = poll_fn(|context| {
                let graph = lock(&engine.graph);
                if graph.closed || !graph.running {
                    return Poll::Ready(false);
                }
                *lock(&engine.reader) = Some(context.waker().clone());
                drop(graph);
                sleep.as_mut().poll(context).map(|()| true)
            })
            .await;
            if !elapsed {
                break;
            }
            let Some(page) = page.upgrade() else {
                return;
            };
            let debt = (Instant::now()
                .saturating_duration_since(deadline)
                .as_secs_f64()
                / quantum.as_secs_f64())
            .floor()
            .max(1.0) as u64;
            let mut graph = lock(&engine.graph);
            if !graph.running || graph.closed {
                break;
            }
            // Bounded catch-up protects the reactor from an unbounded codec
            // or graph backlog. Dropped render quanta are reported.
            for _ in 0..debt.min(8) {
                let start = deadline.saturating_duration_since(page.epoch);
                let ended = graph.process(start);
                page.ended(graph.id, ended);
                deadline += quantum;
            }
            if debt > 8 {
                graph.skip(debt - 8);
                deadline += Duration::from_secs_f64((debt - 8) as f64 * quantum.as_secs_f64());
            }
        }
    }
}

pub(crate) fn install(document: &mut ScriptDocument) {
    let epoch = document.with_js_context(|context| {
        crate::media::host(context)
            .map(|host| host.media_epoch())
            .unwrap_or_else(|_| Instant::now())
    });
    let page = Arc::new(Page {
        epoch,
        state: Mutex::new(State::default()),
        events: Mutex::new(VecDeque::new()),
        reader: Mutex::new(None),
    });
    document.with_js_context(|context| {
        context.insert_data(PageHost(Arc::clone(&page)));
        context
            .register_global_builtin_callable(
                JsString::from("__chuzzAudioCall"),
                1,
                NativeFunction::from_fn_ptr(command),
            )
            .expect("audio command bridge");
        context
            .register_global_builtin_callable(
                JsString::from("__chuzzAudioRead"),
                3,
                NativeFunction::from_fn_ptr(read),
            )
            .expect("audio analyser bridge");
        context
            .eval(Source::from_bytes(include_str!("../audio.ts")))
            .expect("Web Audio bootstrap");
    });
    document.add_poll_hook(move |document, task_context| {
        // Register before draining so an event racing the drain cannot lose
        // the document wakeup.
        if let Some(context) = task_context {
            *lock(&page.reader) = Some(context.waker().clone());
        }
        let events: Vec<_> = {
            let mut queue = lock(&page.events);
            let count = queue.len().min(64);
            let events = queue.drain(..count).collect();
            if !queue.is_empty()
                && let Some(reader) = lock(&page.reader).as_ref()
            {
                reader.wake_by_ref();
            }
            events
        };
        if events.is_empty() {
            return false;
        }
        document.with_js_context(|context| {
            let result = (|| -> JsResult<()> {
                let callback = context
                    .global_object()
                    .get(JsString::from("__chuzzAudioEnded"), context)?
                    .as_object()
                    .filter(|object| object.is_callable())
                    .ok_or_else(|| JsNativeError::typ().with_message("missing audio event hook"))?;
                for (context_id, node) in &events {
                    callback.call(
                        &JsValue::undefined(),
                        &[
                            JsValue::from(*context_id as f64),
                            JsValue::from(*node as f64),
                        ],
                        context,
                    )?;
                }
                Ok(())
            })();
            if let Err(error) = result {
                eprintln!("Uncaught audio event handler error: {error}");
            }
        });
        true
    });
}
