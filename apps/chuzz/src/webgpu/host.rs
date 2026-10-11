use std::sync::OnceLock;
use std::sync::atomic::AtomicUsize;
use std::time::Instant;

use super::*;

const STARTUP_DEADLINE: Duration = Duration::from_secs(1);
const STARTUP_LIMIT: usize = 8;
static STARTING: AtomicUsize = AtomicUsize::new(0);

fn pool() -> &'static nagoya::runtime::Runtime {
    static POOL: OnceLock<nagoya::runtime::Runtime> = OnceLock::new();
    POOL.get_or_init(|| {
        nagoya::runtime::Runtime::builder()
            .workers(2)
            .label("chuzz-webgpu")
            .build()
    })
}

struct StartupSlot;

impl StartupSlot {
    fn acquire() -> Option<Self> {
        STARTING
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < STARTUP_LIMIT).then_some(count + 1)
            })
            .ok()
            .map(|_| Self)
    }
}

impl Drop for StartupSlot {
    fn drop(&mut self) {
        STARTING.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(super) struct Flight {
    deadline: Instant,
    device: Option<u64>,
    fire_device: Option<u64>,
    startup: bool,
    /// False for a diagnostics capture: its pixels go to the caller, not to
    /// `__chuzzGpuResolve`.
    delivers_reply: bool,
    pub(super) inspection: Option<inspector::Call>,
}

impl Flight {
    pub(super) fn live(&self) -> bool {
        Instant::now() < self.deadline
    }
}

impl Bridge {
    pub(super) fn enqueue(&self, request: Request) {
        let id = request.id();
        let inspection = self.inspection_call(&request);
        let startup = matches!(&request, Request::Adapter { .. } | Request::Device { .. });
        let slot = if startup {
            let Some(slot) = StartupSlot::acquire() else {
                self.record_request_error(
                    &request,
                    &GpuError::internal("WebGPU startup is busy in chuzz"),
                );
                self.state().ready.push((
                    id,
                    json!({
                        "ok": false,
                        "error": "WebGPU startup is busy in chuzz",
                        "kind": "internal",
                        "reported": false
                    }),
                ));
                return;
            };
            Some(slot)
        } else {
            None
        };

        {
            let mut state = self.state();
            if state.in_flight.len() >= REQUEST_LIMIT || state.in_flight.contains_key(&id) {
                request.reject_readback("WebGPU request limit reached in chuzz");
                if request.delivers_reply() {
                    state.ready.push((
                        id,
                        json!({
                            "ok": false,
                            "error": "WebGPU request limit reached in chuzz",
                            "kind": "internal",
                            "reported": false,
                            "fireDevice": request.fire_device()
                        }),
                    ));
                }
                drop(state);
                self.record_request_error(
                    &request,
                    &GpuError::internal("WebGPU request limit reached in chuzz"),
                );
                return;
            }
            state.in_flight.insert(
                id,
                Flight {
                    deadline: Instant::now() + if startup { STARTUP_DEADLINE } else { DEADLINE },
                    device: request.error_device(),
                    fire_device: request.fire_device(),
                    startup,
                    delivers_reply: request.delivers_reply(),
                    inspection,
                },
            );
            if !startup {
                state.requests.push_back(request);
                if state.running {
                    return;
                }
                state.running = true;
                drop(state);
                let bridge = self.clone();
                let _ = pool().spawn(async move {
                    bridge.run_commands().await;
                });
                return;
            }
        }

        // Wake the window's deadline check even if native adapter discovery
        // blocks the GPU pool. This timer runs on the process Nagoya runtime.
        let owner = Arc::downgrade(&self.0);
        let _ = nagoya::spawn(async move {
            nagoya::sleep(STARTUP_DEADLINE).await;
            if let Some(owner) = owner.upgrade() {
                let wake = {
                    let state = lock(&owner);
                    state
                        .in_flight
                        .contains_key(&id)
                        .then(|| state.wake.clone())
                };
                if let Some(wake) = wake {
                    wake.wake();
                }
            }
        });

        // Native adapter enumeration and device creation can block inside
        // their first poll. The document executor must never steal this work.
        // Host polling owns the deadline even if the driver never yields.
        let bridge = self.clone();
        let _ = pool().spawn(async move {
            let _slot = slot;
            if !bridge.request_live(id) {
                return;
            }
            let result = nagoya::timeout(STARTUP_DEADLINE, bridge.execute(request)).await;
            let result = match result {
                Ok(result) => result,
                Err(_) => Err(GpuError::internal("WebGPU startup timed out in chuzz")),
            };
            bridge.complete_request(id, result);
        });
    }

    fn request_live(&self, id: u64) -> bool {
        self.state().in_flight.get(&id).is_some_and(Flight::live)
    }

    async fn run_commands(&self) {
        loop {
            let request = {
                let mut state = self.state();
                match state.requests.pop_front() {
                    Some(request) => request,
                    None => {
                        state.running = false;
                        return;
                    }
                }
            };
            let id = request.id();
            if !self.request_live(id) {
                request.reject_readback("WebGPU request timed out in chuzz");
                continue;
            }
            let completion = match &request {
                Request::Readback { completion, .. } => Some(Arc::clone(completion)),
                _ => None,
            };
            let capture = match &request {
                Request::Capture { slot, .. } => Some(Arc::clone(slot)),
                _ => None,
            };
            // One command worker per document retains push/create/write/pop
            // ordering. Startup requests use separate tasks.
            let result = match nagoya::timeout(DEADLINE, self.execute(request)).await {
                Ok(result) => result,
                Err(_) => Err(GpuError::internal("WebGPU request timed out in chuzz")),
            };
            if let Some(completion) = completion
                && let Err(error) = &result
            {
                completion.finish(Err(GpuError::internal(error.message.clone())));
            }
            if let Some(slot) = capture
                && let Err(error) = &result
            {
                slot.finish(Err(blitz_control_protocol::DebugError {
                    code: error.kind.to_owned(),
                    message: error.message.clone(),
                }));
            }
            self.complete_request(id, result);
        }
    }

    fn complete_request(&self, id: u64, result: Result<Value, GpuError>) {
        let flight = self.state().in_flight.remove(&id);
        let Some(flight) = flight else {
            if let Ok(value) = &result {
                self.release_reply(value);
            }
            return;
        };
        let result = if flight.live() {
            result
        } else {
            if let Ok(value) = &result {
                self.release_reply(value);
            }
            if flight.startup {
                let mut state = self.state();
                state.adapters.remove(&id);
                state.devices.remove(&id);
            }
            Err(GpuError::internal("WebGPU request timed out in chuzz"))
        };
        if let Err(error) = &result
            && let Some(call) = &flight.inspection
        {
            // Independent of page routing, including rejected async pipelines
            // and validation later swallowed by a logical page error scope.
            self.record_inspection_error(call, error, flight.device.is_some());
        }
        if !flight.delivers_reply {
            if let Ok(value) = &result {
                self.release_reply(value);
            }
            return;
        }
        let payload = match result {
            Ok(value) => {
                if flight.fire_device.is_some() {
                    // Fire commands have no JS continuation. Avoid a reply,
                    // JSON serialization and document.eval on success.
                    self.release_reply(&value);
                    return;
                }
                json!({"ok": true, "value": value})
            }
            Err(error) => {
                let device = flight
                    .device
                    .and_then(|id| self.state().devices.get(&id).cloned());
                if let Some(device) = &device {
                    // The single ordered worker reports before the next
                    // push/pop operation, including a failed finish batch.
                    device.report(&error);
                    if flight.fire_device.is_some() {
                        return;
                    }
                }
                json!({
                    "ok": false,
                    "error": error.message,
                    "kind": error.kind,
                    "reported": device.is_some(),
                    "fireDevice": flight.fire_device
                })
            }
        };
        let wake = {
            let mut state = self.state();
            state.ready.push((id, payload));
            state.wake.clone()
        };
        wake.wake();
    }

    pub(super) fn expire_requests(&self) {
        let expired = {
            let state = self.state();
            state
                .in_flight
                .iter()
                .filter_map(|(&id, flight)| (!flight.live()).then_some(id))
                .collect::<Vec<_>>()
        };
        for id in expired {
            self.complete_request(
                id,
                Err(GpuError::internal("WebGPU request timed out in chuzz")),
            );
        }
    }

    pub(super) fn poll_devices(&self) {
        let devices = {
            let mut state = self.state();
            if state.polling || state.devices.is_empty() {
                return;
            }
            state.polling = true;
            state.devices.values().cloned().collect::<Vec<_>>()
        };
        let bridge = self.clone();
        let _ = pool().spawn(async move {
            for connection in devices {
                if let Err(error) = connection.device.poll(wgpu::PollType::Poll) {
                    post_loss(
                        &connection.owner,
                        connection.id,
                        &connection.lost,
                        &connection.loss_reason,
                        "unknown",
                        format!("WebGPU poll failed: {error}"),
                    );
                }
            }
            bridge.state().polling = false;
        });
    }
}
