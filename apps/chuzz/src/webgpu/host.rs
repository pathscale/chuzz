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
    startup: bool,
}

impl Flight {
    pub(super) fn live(&self) -> bool {
        Instant::now() < self.deadline
    }
}

impl Bridge {
    pub(super) fn enqueue(&self, request: Request) {
        let id = request.id();
        let startup = !matches!(&request, Request::Command { .. });
        let slot = if startup {
            let Some(slot) = StartupSlot::acquire() else {
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
                state.ready.push((
                    id,
                    json!({
                        "ok": false,
                        "error": "WebGPU request limit reached in chuzz",
                        "kind": "internal",
                        "reported": false
                    }),
                ));
                return;
            }
            state.in_flight.insert(
                id,
                Flight {
                    deadline: Instant::now() + if startup { STARTUP_DEADLINE } else { DEADLINE },
                    device: request.error_device(),
                    startup,
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
                continue;
            }
            // One command worker per document retains push/create/write/pop
            // ordering. Startup requests use separate tasks.
            let result = match nagoya::timeout(DEADLINE, self.execute(request)).await {
                Ok(result) => result,
                Err(_) => Err(GpuError::internal("WebGPU request timed out in chuzz")),
            };
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
        let payload = match result {
            Ok(value) => json!({"ok": true, "value": value}),
            Err(error) => {
                let device = flight
                    .device
                    .and_then(|id| self.state().devices.get(&id).cloned());
                if let Some(device) = &device {
                    device.report(&error);
                }
                json!({
                    "ok": false,
                    "error": error.message,
                    "kind": error.kind,
                    "reported": device.is_some()
                })
            }
        };
        self.state().ready.push((id, payload));
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
                        "unknown",
                        format!("WebGPU poll failed: {error}"),
                    );
                }
            }
            bridge.state().polling = false;
        });
    }
}

