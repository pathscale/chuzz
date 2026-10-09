//! Page WebGPU objects, owned by the document's bridge.
//!
//! Native commands execute in page call order. Boa receives handles over IPC;
//! GPU resources remain on the Rust side. Canvas readback uses Blitz's raster
//! image path, including the headless CPU painter.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use serde_json::{Map, Value, json};

mod binary;
mod buffer;
mod canvas;
mod encoder;
mod external;
mod host;
mod pipeline;
mod readback;
mod resources;
mod texture;

use resources::{Command, GpuError, Resources};

pub(crate) const WEBGPU_SHIM: &str =
    concat!(include_str!("webgpu.ts"), "\n", include_str!("webgpu/images.ts"));

const DEADLINE: Duration = Duration::from_secs(10);
const OBJECT_LIMIT: usize = 64;
const REQUEST_LIMIT: usize = 4096;

// Export only WebGPU limits, excluding wgpu's native extensions.
const LIMIT_NAMES: &[&str] = &[
    "maxTextureDimension1D",
    "maxTextureDimension2D",
    "maxTextureDimension3D",
    "maxTextureArrayLayers",
    "maxBindGroups",
    "maxBindingsPerBindGroup",
    "maxDynamicUniformBuffersPerPipelineLayout",
    "maxDynamicStorageBuffersPerPipelineLayout",
    "maxSampledTexturesPerShaderStage",
    "maxSamplersPerShaderStage",
    "maxStorageBuffersPerShaderStage",
    "maxStorageTexturesPerShaderStage",
    "maxUniformBuffersPerShaderStage",
    "maxUniformBufferBindingSize",
    "maxStorageBufferBindingSize",
    "minUniformBufferOffsetAlignment",
    "minStorageBufferOffsetAlignment",
    "maxVertexBuffers",
    "maxBufferSize",
    "maxVertexAttributes",
    "maxVertexBufferArrayStride",
    "maxInterStageShaderVariables",
    "maxColorAttachments",
    "maxColorAttachmentBytesPerSample",
    "maxComputeWorkgroupStorageSize",
    "maxComputeInvocationsPerWorkgroup",
    "maxComputeWorkgroupSizeX",
    "maxComputeWorkgroupSizeY",
    "maxComputeWorkgroupsPerDimension",
];

// A deliberately explicit mapping: native-only wgpu flags are never exposed
// as WebGPU feature names or accepted in a device descriptor.
const FEATURE_NAMES: &[(&str, wgpu::Features)] = &[
    ("depth-clip-control", wgpu::Features::DEPTH_CLIP_CONTROL),
    ("depth32float-stencil8", wgpu::Features::DEPTH32FLOAT_STENCIL8),
    ("texture-compression-bc", wgpu::Features::TEXTURE_COMPRESSION_BC),
    (
        "texture-compression-etc2",
        wgpu::Features::TEXTURE_COMPRESSION_ETC2,
    ),
    (
        "texture-compression-astc",
        wgpu::Features::TEXTURE_COMPRESSION_ASTC,
    ),
    // Do not advertise timestamps until timestampWrites can be replayed.
    ("indirect-first-instance", wgpu::Features::INDIRECT_FIRST_INSTANCE),
    ("shader-f16", wgpu::Features::SHADER_F16),
    (
        "rg11b10ufloat-renderable",
        wgpu::Features::RG11B10UFLOAT_RENDERABLE,
    ),
    ("bgra8unorm-storage", wgpu::Features::BGRA8UNORM_STORAGE),
    ("float32-filterable", wgpu::Features::FLOAT32_FILTERABLE),
];

#[derive(Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AdapterOptions {
    power_preference: Option<String>,
    #[serde(default)]
    force_fallback_adapter: bool,
}

#[derive(Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeviceOptions {
    #[serde(default)]
    label: String,
    #[serde(default)]
    required_features: Vec<String>,
    #[serde(default)]
    required_limits: Map<String, Value>,
}

#[derive(serde::Deserialize)]
#[serde(tag = "webgpu", deny_unknown_fields)]
enum Request {
    #[serde(rename = "requestAdapter")]
    Adapter {
        id: u64,
        options: AdapterOptions,
    },
    #[serde(rename = "requestDevice")]
    Device {
        id: u64,
        adapter: u64,
        options: DeviceOptions,
    },
    #[serde(rename = "command")]
    Command {
        id: u64,
        device: u64,
        command: Command,
        #[serde(skip)]
        bytes: binary::Lease,
    },
    // Only the native canvas callback can construct this request. Pixels
    // return directly as a typed array rather than as JSON through IPC.
    #[serde(skip)]
    Readback {
        id: u64,
        source: external::Source,
        completion: Arc<readback::Completion>,
    },
}

impl Request {
    fn id(&self) -> u64 {
        match self {
            Self::Adapter { id, .. }
            | Self::Device { id, .. }
            | Self::Command { id, .. }
            | Self::Readback { id, .. } => *id,
        }
    }

    fn reject_readback(&self, message: &str) {
        if let Self::Readback { completion, .. } = self {
            completion.finish(Err(GpuError::internal(message)));
        }
    }

    fn error_device(&self) -> Option<u64> {
        match self {
            Self::Command {
                device, command, ..
            } if !matches!(
                command.op.as_str(),
                "popErrorScope"
                    | "pushErrorScope"
                    | "destroy"
                    | "getCompilationInfo"
                    | "createRenderPipelineAsync"
                    | "createComputePipelineAsync"
            ) => Some(*device),
            _ => None,
        }
    }
}

struct ErrorScope {
    filter: String,
    error: Option<Value>,
}

struct DeviceConnection {
    id: u64,
    owner: Weak<Mutex<State>>,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    lost: Arc<AtomicBool>,
    scopes: Mutex<Vec<ErrorScope>>,
    resources: Mutex<Resources>,
}

impl DeviceConnection {
    fn resources(&self) -> MutexGuard<'_, Resources> {
        lock(&self.resources)
    }

    fn report(&self, error: &GpuError) {
        let value = error.value();
        {
            let mut scopes = lock(&self.scopes);
            if let Some(scope) = scopes
                .iter_mut()
                .rev()
                .find(|scope| scope.filter == error.kind)
            {
                if scope.error.is_none() {
                    scope.error = Some(value);
                }
                return;
            }
        }
        post_event(
            &self.owner,
            self.id,
            json!({"type": "error", "error": value}),
        );
    }

    async fn execute(&self, command: Command) -> Result<Value, GpuError> {
        match command.op.as_str() {
            "destroy" => {
                // Removing resources also releases mapped allocations and views.
                let resources = std::mem::take(&mut *self.resources());
                drop(resources);
                self.device.destroy();
                post_loss(
                    &self.owner,
                    self.id,
                    &self.lost,
                    "destroyed",
                    "GPUDevice.destroy() was called".to_owned(),
                );
                Ok(Value::Null)
            }
            "pushErrorScope" => {
                let filter = command.args.as_str().ok_or_else(|| {
                    GpuError::validation("invalid GPUErrorFilter")
                })?;
                if !matches!(filter, "validation" | "out-of-memory" | "internal") {
                    return Err(GpuError::validation("invalid GPUErrorFilter"));
                }
                let mut scopes = lock(&self.scopes);
                if scopes.len() >= 256 {
                    return Err(GpuError::validation(
                        "WebGPU error scope limit reached in chuzz",
                    ));
                }
                scopes.push(ErrorScope {
                    filter: filter.to_owned(),
                    error: None,
                });
                Ok(Value::Null)
            }
            "popErrorScope" => lock(&self.scopes)
                .pop()
                .map(|scope| scope.error.unwrap_or(Value::Null))
                .ok_or_else(|| GpuError::validation("GPUDevice has no error scope")),
            _ => {
                if self.lost.load(Ordering::Acquire) {
                    return Err(GpuError::validation("GPUDevice is lost or destroyed"));
                }
                self.execute_resource(command).await
            }
        }
    }
}

impl Drop for DeviceConnection {
    fn drop(&mut self) {
        self.device.destroy();
    }
}

#[derive(Default)]
struct State {
    adapters: HashMap<u64, Arc<wgpu::Adapter>>,
    devices: HashMap<u64, Arc<DeviceConnection>>,
    incoming: VecDeque<Request>,
    images: external::Images,
    bytes: Arc<binary::Table>,
    requests: VecDeque<Request>,
    running: bool,
    polling: bool,
    in_flight: HashMap<u64, host::Flight>,
    ready: Vec<(u64, Value)>,
    events: Vec<(u64, Value)>,
    frames: HashMap<u64, canvas::Frame>,
}

#[derive(Clone, Default)]
pub(crate) struct Bridge(Arc<Mutex<State>>);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

fn post_event(owner: &Weak<Mutex<State>>, device: u64, value: Value) {
    if let Some(state) = owner.upgrade() {
        lock(&state).events.push((device, value));
    }
}

fn post_loss(
    owner: &Weak<Mutex<State>>,
    device: u64,
    lost: &AtomicBool,
    reason: &str,
    message: String,
) {
    if !lost.swap(true, Ordering::AcqRel) {
        post_event(
            owner,
            device,
            json!({"type": "lost", "reason": reason, "message": message}),
        );
    }
}

impl Bridge {
    fn state(&self) -> MutexGuard<'_, State> {
        lock(&self.0)
    }

    pub(crate) fn handle(&self, message: &str) -> bool {
        let Ok(mut request) = serde_json::from_str::<Request>(message) else {
            return false;
        };
        if let Request::Command { command, bytes, .. } = &mut request {
            match self.byte_table().claim(command) {
                Ok(lease) => *bytes = lease,
                Err(error) => {
                    self.state().ready.push((
                        request.id(),
                        json!({
                            "ok": false,
                            "error": error.message,
                            "kind": error.kind,
                            "reported": false
                        }),
                    ));
                    return true;
                }
            }
        }
        // DOM snapshots belong to the document thread. Prepare the whole
        // incoming batch there before passing it to the ordered host worker.
        let mut state = self.state();
        if state.incoming.len() + state.in_flight.len() >= REQUEST_LIMIT {
            state.ready.push((
                request.id(),
                json!({
                    "ok": false,
                    "error": "WebGPU request limit reached in chuzz",
                    "kind": "internal",
                    "reported": false
                }),
            ));
        } else {
            state.incoming.push_back(request);
        }
        true
    }

    pub(crate) fn drain_into(&self, document: &mut blitz_script::ScriptDocument) -> bool {
        let images_changed = self.prepare_images(document);
        // Deadlines are host state, not page timers. No driver work runs on
        // the document thread, including nominally nonblocking device.poll.
        self.expire_requests();
        self.poll_devices();

        let (ready, events, frames) = {
            let mut state = self.state();
            (
                std::mem::take(&mut state.ready),
                std::mem::take(&mut state.events),
                std::mem::take(&mut state.frames),
            )
        };
        let mut changed = images_changed || !ready.is_empty() || !events.is_empty();
        // Install newly requested JS devices before delivering their events.
        for (id, payload) in ready {
            document.eval(&format!(
                "globalThis.__chuzzGpuResolve({id}, {payload});"
            ));
            // A missing or throwing resolver must not retain a readback.
            if let Some(value) = payload.get("value") {
                self.release_reply(value);
            }
        }
        for (id, payload) in events {
            document.eval(&format!(
                "globalThis.__chuzzGpuEvent({id}, {payload});"
            ));
        }
        for frame in frames.into_values() {
            changed |= frame.paint(document);
        }
        // Present each submitted canvas texture at the end of this host turn.
        // The shim expires it synchronously and queues the native readback.
        document.eval("globalThis.__chuzzGpuFlushCanvases();");
        changed
    }

    async fn execute(&self, request: Request) -> Result<Value, GpuError> {
        match request {
            Request::Adapter { id, options } => self
                .request_adapter(id, options)
                .await
                .map_err(GpuError::validation),
            Request::Device {
                id,
                adapter,
                options,
            } => self
                .request_device(id, adapter, options)
                .await
                .map_err(GpuError::validation),
            Request::Readback {
                source, completion, ..
            } => {
                completion.finish(self.resolve_source(source).await);
                Ok(Value::Null)
            }
            Request::Command {
                id,
                device,
                command,
                bytes,
            } => {
                let _bytes = bytes;
                // Image operations use the same ordered, bounded worker as
                // GPU commands, without requiring a GPUDevice.
                if device == 0 {
                    return self.execute_image(id, command).await;
                }
                let device = self
                    .state()
                    .devices
                    .get(&device)
                    .cloned()
                    .ok_or_else(|| GpuError::validation("invalid GPUDevice"))?;
                device.execute(command).await
            }
        }
    }

    async fn request_adapter(&self, id: u64, options: AdapterOptions) -> Result<Value, String> {
        let power_preference = match options.power_preference.as_deref() {
            None => wgpu::PowerPreference::None,
            Some("low-power") => wgpu::PowerPreference::LowPower,
            Some("high-performance") => wgpu::PowerPreference::HighPerformance,
            Some(_) => return Err("invalid GPURequestAdapterOptions.powerPreference".to_owned()),
        };
        // One guard: two `self.state()` temporaries in one expression would
        // lock the same mutex twice and deadlock this thread.
        let objects = {
            let state = self.state();
            state.adapters.len() + state.devices.len()
        };
        if objects >= OBJECT_LIMIT {
            return Err("WebGPU object limit reached in chuzz".to_owned());
        }

        // Headless capture uses the CPU painter and has no renderer device.
        // Open a native windowless adapter only when a page asks for WebGPU.
        let instance = wgpu::Instance::default();
        let adapter = match instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference,
                force_fallback_adapter: options.force_fallback_adapter,
                compatible_surface: None,
            })
            .await
        {
            Ok(adapter) => adapter,
            Err(_) => return Ok(Value::Null),
        };
        if !adapter.get_downlevel_capabilities().is_webgpu_compliant() {
            return Ok(Value::Null);
        }
        let value = json!({
            "handle": id,
            "features": feature_names(adapter.features()),
            "limits": limits_value(&adapter.limits())?,
            "info": adapter_info(&adapter),
            "isFallbackAdapter": adapter.get_info().device_type == wgpu::DeviceType::Cpu,
        });
        let mut state = self.state();
        if !state.in_flight.get(&id).is_some_and(host::Flight::live) {
            return Err("GPU adapter request expired in chuzz".to_owned());
        }
        if state.adapters.len() + state.devices.len() >= OBJECT_LIMIT {
            return Err("WebGPU object limit reached in chuzz".to_owned());
        }
        state.adapters.insert(id, Arc::new(adapter));
        Ok(value)
    }

    async fn request_device(
        &self,
        id: u64,
        adapter_id: u64,
        options: DeviceOptions,
    ) -> Result<Value, String> {
        // Consume the adapter before awaiting. wgpu warns that a second
        // request_device on the same adapter can panic.
        let adapter = self
            .state()
            .adapters
            .remove(&adapter_id)
            .ok_or_else(|| "GPUAdapter is invalid or has already requested a device".to_owned())?;

        let mut features = wgpu::Features::empty();
        for name in &options.required_features {
            let flag = FEATURE_NAMES
                .iter()
                .find(|(candidate, _)| candidate == name)
                .map(|(_, flag)| *flag)
                .ok_or_else(|| format!("unsupported WebGPU feature: {name}"))?;
            if !adapter.features().contains(flag) {
                return Err(format!("unsupported WebGPU feature: {name}"));
            }
            features |= flag;
        }

        let available = limits_value(&adapter.limits())?;
        for (name, value) in &options.required_limits {
            if !LIMIT_NAMES.contains(&name.as_str()) || available.get(name).is_none() {
                return Err(format!(
                    "not implemented in chuzz: GPUDeviceDescriptor.requiredLimits.{name}"
                ));
            }
            if value.as_u64().is_none_or(|value| value > 9_007_199_254_740_991) {
                return Err(format!("invalid WebGPU limit: {name}"));
            }
        }
        let requested: wgpu::Limits =
            serde_json::from_value(Value::Object(options.required_limits))
                .map_err(|error| format!("invalid WebGPU limits: {error}"))?;
        let limits = wgpu::Limits::default().or_better_values_from(&requested);
        if !limits.check_limits(&adapter.limits()) {
            return Err("requested WebGPU limits exceed adapter capabilities".to_owned());
        }

        let descriptor = wgpu::DeviceDescriptor {
            label: Some(options.label.as_str()),
            required_features: features,
            required_limits: limits,
            ..Default::default()
        };
        let (device, queue) = adapter
            .request_device(&descriptor)
            .await
            .map_err(|error| format!("GPUAdapter.requestDevice failed: {error}"))?;
        let value = json!({
            "handle": id,
            "label": options.label,
            "features": feature_names(device.features()),
            "limits": limits_value(&device.limits())?,
            "info": adapter_info(&adapter),
        });
        let connection = Arc::new(DeviceConnection {
            id,
            owner: Arc::downgrade(&self.0),
            device: Arc::new(device),
            queue: Arc::new(queue),
            lost: Arc::new(AtomicBool::new(false)),
            scopes: Mutex::new(Vec::new()),
            resources: Mutex::new(Resources::default()),
        });

        let owner = connection.owner.clone();
        let lost = Arc::clone(&connection.lost);
        connection.device.set_device_lost_callback(move |reason, message| {
            let reason = match reason {
                wgpu::DeviceLostReason::Destroyed => "destroyed",
                wgpu::DeviceLostReason::Unknown => "unknown",
            };
            post_loss(&owner, id, &lost, reason, message);
        });
        let weak = Arc::downgrade(&connection);
        connection.device.on_uncaptured_error(Arc::new(move |error| {
            if let Some(connection) = weak.upgrade() {
                connection.report(&GpuError::from(error));
            }
        }));

        let mut state = self.state();
        if !state.in_flight.get(&id).is_some_and(host::Flight::live) {
            return Err("GPU device request expired in chuzz".to_owned());
        }
        if state.adapters.len() + state.devices.len() >= OBJECT_LIMIT {
            return Err("WebGPU object limit reached in chuzz".to_owned());
        }
        state.devices.insert(id, connection);
        Ok(value)
    }
}

fn adapter_info(adapter: &wgpu::Adapter) -> Value {
    let info = adapter.get_info();
    json!({
        "vendor": format!("{:04x}", info.vendor),
        "architecture": "",
        "device": format!("{:04x}", info.device),
        "description": info.name,
        "isFallbackAdapter": adapter.get_info().device_type == wgpu::DeviceType::Cpu,
    })
}

fn feature_names(features: wgpu::Features) -> Vec<&'static str> {
    FEATURE_NAMES
        .iter()
        .filter_map(|(name, flag)| features.contains(*flag).then_some(*name))
        .collect()
}

fn limits_value(limits: &wgpu::Limits) -> Result<Value, String> {
    let value = serde_json::to_value(limits).map_err(|error| error.to_string())?;
    let mut public = Map::new();
    for name in LIMIT_NAMES {
        if let Some(value) = value.get(*name) {
            public.insert((*name).to_owned(), value.clone());
        }
    }
    Ok(Value::Object(public))
}

