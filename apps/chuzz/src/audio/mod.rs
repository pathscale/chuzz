//! Page-owned Web Audio host.
//!
//! One nagoya task drives each context's render clock. Codecs and processing
//! functions remain synchronous. Native host callbacks deliver owned
//! metadata and analysis buffers to the active script engine.

mod analysis;
mod graph;
#[path = "audio_host_bridge.rs"]
pub mod native_host;
pub mod pcm;
mod resample;

use std::collections::{BTreeMap, VecDeque};
use std::future::{Future, poll_fn};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Poll, Waker};
use std::time::{Duration, Instant};

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
            drop(nagoya::spawn(run(
                Arc::downgrade(&engine),
                Arc::downgrade(page),
            )));
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
