//! Dedicated workers. Only owned bytes and Rust metadata cross threads.
//!
//! Every worker owns a one-thread Nagoya runtime. Boa is constructed, used and
//! dropped inside a submitted closure, so a context never crosses threads.
//! The page hook reports work only when it dispatches a message or error.

mod clone;
mod modules;
mod runtime;

use std::collections::{HashMap, VecDeque};
use std::future::{Future, poll_fn};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::task::{Poll, Waker};
use std::time::Duration;

use blitz_script::ScriptDocument;
use blitz_script::boa_engine::{
    Context, JsError, JsNativeError, JsResult, JsString, JsValue, NativeFunction, Source,
};
use blitz_traits::net::Url;

use crate::document_loader::NetProvider;

use self::clone::Wire;

const DEADLINE: Duration = Duration::from_secs(30);
const DELIVERIES_PER_POLL: usize = 64;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

struct QueueState<T> {
    entries: VecDeque<T>,
    reader: Option<Waker>,
}

struct Queue<T>(Mutex<QueueState<T>>);

impl<T> Default for Queue<T> {
    fn default() -> Self {
        Self(Mutex::new(QueueState {
            entries: VecDeque::new(),
            reader: None,
        }))
    }
}

impl<T> Queue<T> {
    fn push(&self, entry: T) {
        let reader = {
            let mut state = lock(&self.0);
            state.entries.push_back(entry);
            state.reader.take()
        };
        if let Some(reader) = reader {
            reader.wake();
        }
    }

    fn wake(&self) {
        let reader = lock(&self.0).reader.take();
        if let Some(reader) = reader {
            reader.wake();
        }
    }

    fn clear(&self) {
        lock(&self.0).entries.clear();
        self.wake();
    }

    fn take(&self, count: usize, reader: Option<&Waker>) -> Vec<T> {
        let mut state = lock(&self.0);
        if let Some(reader) = reader {
            state.reader = Some(reader.clone());
        }
        let count = count.min(state.entries.len());
        let entries = state.entries.drain(..count).collect();
        if !state.entries.is_empty()
            && let Some(reader) = &state.reader
        {
            reader.wake_by_ref();
        }
        entries
    }

    async fn recv(&self) -> T {
        poll_fn(|context| {
            let mut state = lock(&self.0);
            if let Some(entry) = state.entries.pop_front() {
                Poll::Ready(entry)
            } else {
                state.reader = Some(context.waker().clone());
                Poll::Pending
            }
        })
        .await
    }
}

enum Input {
    Message(Wire),
    Fetch {
        id: u64,
        metadata: String,
        body: blitz_script::boa_engine::object::builtins::AlignedVec<u8>,
    },
}

enum Output {
    Message(Wire),
    Error(serde_json::Value),
    Closed,
}

struct Delivery {
    endpoint: Arc<Endpoint>,
    output: Output,
}

struct Endpoint {
    id: u64,
    canceled: Arc<AtomicBool>,
    closing: AtomicBool,
    finished: AtomicBool,
    inbox: Queue<Input>,
    runtime: Arc<nagoya::runtime::Runtime>,
}

impl Endpoint {
    fn terminate(&self) {
        self.canceled.store(true, Ordering::Release);
        self.inbox.clear();
        self.runtime.pool().shut_down();
    }

    fn stopped(&self) -> bool {
        self.canceled.load(Ordering::Acquire) || self.finished.load(Ordering::Acquire)
    }

    fn post(self: &Arc<Self>, page: &Weak<Page>, output: Output) {
        if !self.canceled.load(Ordering::Acquire)
            && let Some(page) = page.upgrade()
        {
            page.ready.push(Delivery {
                endpoint: Arc::clone(self),
                output,
            });
        }
    }
}

struct Page {
    net: Arc<NetProvider>,
    base: Option<Url>,
    next_id: AtomicU64,
    workers: Mutex<HashMap<u64, Arc<Endpoint>>>,
    ready: Queue<Delivery>,
}

impl Drop for Page {
    fn drop(&mut self) {
        for endpoint in lock(&self.workers).values() {
            endpoint.terminate();
        }
    }
}

#[derive(Clone)]
struct PageHost(Arc<Page>);

fn argument(args: &[JsValue], index: usize) -> JsValue {
    args.get(index).cloned().unwrap_or_default()
}

fn string(value: &JsValue, context: &mut Context) -> JsResult<String> {
    Ok(value.to_string(context)?.to_std_string_escaped())
}

fn named_error(context: &mut Context, name: &str, message: &str) -> JsError {
    match call(
        context,
        "__chuzzWorkerException",
        &[JsString::from(name).into(), JsString::from(message).into()],
    ) {
        Ok(value) => JsError::from_opaque(value),
        Err(error) => error,
    }
}

fn call(context: &mut Context, name: &str, arguments: &[JsValue]) -> JsResult<JsValue> {
    let function = context
        .global_object()
        .get(JsString::from(name), context)?
        .as_object()
        .filter(|function| function.is_callable())
        .ok_or_else(|| JsNativeError::typ().with_message(format!("missing worker hook {name}")))?;
    function.call(&JsValue::undefined(), arguments, context)
}

fn register(
    context: &mut Context,
    name: &str,
    length: usize,
    function: fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>,
) -> JsResult<()> {
    context.register_global_builtin_callable(
        JsString::from(name),
        length,
        NativeFunction::from_fn_ptr(function),
    )
}

/// Cancel I/O independently of whether JavaScript is currently executing.
/// Socket futures continue to use the process's existing Nagoya networking.
async fn cancellable<F: Future>(endpoint: &Endpoint, future: F) -> Result<F::Output, &'static str> {
    let mut future = Box::pin(future);
    let mut cancellation = Box::pin(async {
        loop {
            if endpoint.stopped() {
                return;
            }
            nagoya::sleep(Duration::from_millis(5)).await;
        }
    });
    poll_fn(|context| {
        if endpoint.stopped() {
            return Poll::Ready(Err("worker terminated"));
        }
        if let Poll::Ready(()) = cancellation.as_mut().poll(context) {
            return Poll::Ready(Err("worker terminated"));
        }
        future.as_mut().poll(context).map(Ok)
    })
    .await
}

/// Virtual reservation for a worker thread, the same as the page thread's: Boa's parser and AST
/// clone recurse on the native stack, and a minified bundle's deep expression chains overflowed the
/// 2 MB a default thread gets, which aborts the whole process. Pages are committed only when touched.
const WORKER_STACK_BYTES: usize = 256 * 1024 * 1024;

/// A one-thread runtime for one worker, on a thread sized for Boa.
fn worker_runtime(id: u64) -> nagoya::runtime::Runtime {
    nagoya::runtime::Runtime::builder()
        .workers(1)
        .label(&format!("chuzz-worker-{id}"))
        .stack_size(WORKER_STACK_BYTES)
        .build()
}

fn create(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let page = context
        .get_data::<PageHost>()
        .ok_or_else(|| JsNativeError::typ().with_message("worker host is unavailable"))?
        .0
        .clone();
    let address = string(&argument(args, 0), context)?;
    let kind = string(&argument(args, 1), context)?;
    let name = string(&argument(args, 2), context)?;
    let url = Url::options()
        .base_url(page.base.as_ref())
        .parse(&address)
        .map_err(|error| named_error(context, "SyntaxError", &error.to_string()))?;
    if !matches!(url.scheme(), "http" | "https" | "file" | "data") {
        return Err(named_error(
            context,
            "SecurityError",
            "unsupported worker URL",
        ));
    }
    if url.scheme() != "data"
        && let Some(base) = &page.base
        && url.origin() != base.origin()
    {
        return Err(named_error(
            context,
            "SecurityError",
            "worker script must have the creator's origin",
        ));
    }

    let id = page.next_id.fetch_add(1, Ordering::Relaxed);
    let worker_runtime = Arc::new(worker_runtime(id));
    let endpoint = Arc::new(Endpoint {
        id,
        canceled: Arc::new(AtomicBool::new(false)),
        closing: AtomicBool::new(false),
        finished: AtomicBool::new(false),
        inbox: Queue::default(),
        runtime: Arc::clone(&worker_runtime),
    });
    lock(&page.workers).insert(id, Arc::clone(&endpoint));
    let weak_page = Arc::downgrade(&page);
    let net = Arc::clone(&page.net);
    worker_runtime.executor().submit(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            runtime::run(
                Arc::clone(&endpoint),
                weak_page.clone(),
                net,
                url,
                kind == "module",
                name,
            );
        }));
        if result.is_err() {
            endpoint.post(
                &weak_page,
                Output::Error(serde_json::json!({
                    "message": "worker runtime panicked",
                    "filename": "",
                    "lineno": 0,
                    "colno": 0,
                    "name": "Error",
                    "stack": "",
                })),
            );
        }
        endpoint.finished.store(true, Ordering::Release);
        endpoint.inbox.clear();
        endpoint.post(&weak_page, Output::Closed);
        endpoint.runtime.pool().shut_down();
    });
    Ok(JsValue::from(id as f64))
}

fn post(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let page = context
        .get_data::<PageHost>()
        .ok_or_else(|| JsNativeError::typ().with_message("worker host is unavailable"))?
        .0
        .clone();
    let id = argument(args, 0).to_number(context)? as u64;
    let endpoint = lock(&page.workers).get(&id).cloned();
    let wire = clone::encode(argument(args, 1), argument(args, 2), context)?;
    if let Some(endpoint) = endpoint
        && !endpoint.stopped()
        && !endpoint.closing.load(Ordering::Acquire)
    {
        endpoint.inbox.push(Input::Message(wire));
    }
    Ok(JsValue::undefined())
}

fn terminate(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let page = context
        .get_data::<PageHost>()
        .ok_or_else(|| JsNativeError::typ().with_message("worker host is unavailable"))?
        .0
        .clone();
    let id = argument(args, 0).to_number(context)? as u64;
    let endpoint = lock(&page.workers).remove(&id);
    if let Some(endpoint) = endpoint {
        endpoint.terminate();
    }
    Ok(JsValue::undefined())
}

pub(crate) fn install(document: &mut ScriptDocument, net: Arc<NetProvider>) {
    let page = Arc::new(Page {
        net,
        base: document.current_url().cloned(),
        next_id: AtomicU64::new(1),
        workers: Mutex::new(HashMap::new()),
        ready: Queue::default(),
    });
    document.with_js_context(|context| {
        context.insert_data(PageHost(Arc::clone(&page)));
        context
            .eval(Source::from_bytes(include_str!("events.ts")))
            .expect("worker event bootstrap");
        clone::install(context).expect("worker clone bootstrap");
        register(context, "__chuzzWorkerCreate", 3, create).expect("worker constructor bridge");
        register(context, "__chuzzWorkerPost", 3, post).expect("worker message bridge");
        register(context, "__chuzzWorkerTerminate", 1, terminate)
            .expect("worker termination bridge");
        context
            .eval(Source::from_bytes(include_str!("page.ts")))
            .expect("worker page bootstrap");
    });
    document.add_poll_hook(move |document, task_context| {
        let ready = page.ready.take(
            DELIVERIES_PER_POLL,
            task_context.map(|context| context.waker()),
        );
        let mut delivered = false;
        for Delivery { endpoint, output } in ready {
            if endpoint.canceled.load(Ordering::Acquire) {
                continue;
            }
            if matches!(&output, Output::Closed) {
                lock(&page.workers).remove(&endpoint.id);
                document.with_js_context(|context| {
                    let _ = call(
                        context,
                        "__chuzzWorkerForget",
                        &[JsValue::from(endpoint.id as f64)],
                    );
                });
                continue;
            }
            delivered = true;
            document.with_js_context(|context| {
                let (kind, payload) = match output {
                    Output::Message(wire) => match clone::decode(wire, context) {
                        Ok(value) => ("message", value),
                        Err(_) => ("messageerror", JsValue::undefined()),
                    },
                    Output::Error(error) => (
                        "error",
                        JsValue::from_json(&error, context).unwrap_or_default(),
                    ),
                    Output::Closed => unreachable!(),
                };
                let result = call(
                    context,
                    "__chuzzWorkerDeliver",
                    &[
                        JsValue::from(endpoint.id as f64),
                        JsString::from(kind).into(),
                        payload,
                    ],
                );
                if let Err(error) = result {
                    eprintln!("Uncaught worker event handler error: {error}");
                }
                // The handler's microtasks run on the document's next poll, under its job budget:
                // draining them here would let one message (a planet bake result) hold the page.
            });
        }
        delivered
    });
}
