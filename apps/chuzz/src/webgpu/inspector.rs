use std::collections::BTreeMap;
use std::sync::atomic::AtomicU64;
use std::time::Instant;

use blitz_control_protocol::{
    DebugError, WebGpuCompilationMessage, WebGpuDevice, WebGpuEntryPoint, WebGpuError,
    WebGpuErrors, WebGpuFrame, WebGpuReport, WebGpuResource, WebGpuResourceDetails,
};

use super::*;

pub(super) const PASS_LIMIT: usize = 512;
const ERROR_LIMIT: usize = 1024;
const FRAME_LIMIT: usize = 8;
const LEASE_MS: u64 = 10_000;

#[derive(Clone)]
pub(super) struct Call {
    pub device: u64,
    pub api_call: String,
    pub object_label: String,
}

#[derive(Default)]
struct ErrorLog {
    entries: VecDeque<WebGpuError>,
    total: u64,
    validation_total: u64,
    uncaptured_total: u64,
    dropped: u64,
}

pub(super) struct Session {
    origin: Instant,
    active_until: AtomicU64,
    errors: Mutex<ErrorLog>,
    frames: Mutex<VecDeque<Arc<WebGpuFrame>>>,
    sequence: AtomicU64,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            origin: Instant::now(),
            active_until: AtomicU64::new(0),
            errors: Mutex::new(ErrorLog::default()),
            frames: Mutex::new(VecDeque::new()),
            sequence: AtomicU64::new(0),
        }
    }
}

impl Session {
    fn now(&self) -> u64 {
        u64::try_from(self.origin.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn renew(&self) {
        self.active_until
            .store(self.now().saturating_add(LEASE_MS), Ordering::Release);
    }

    pub(super) fn active(&self) -> bool {
        self.now() < self.active_until.load(Ordering::Acquire)
    }

    fn error(&self, call: &Call, error: &GpuError, uncaptured: bool) {
        let mut log = lock(&self.errors);
        log.total = log.total.saturating_add(1);
        if error.kind == "validation" {
            log.validation_total = log.validation_total.saturating_add(1);
        }
        if uncaptured {
            log.uncaptured_total = log.uncaptured_total.saturating_add(1);
        }
        if log.entries.len() == ERROR_LIMIT {
            log.entries.pop_front();
            log.dropped = log.dropped.saturating_add(1);
        }
        log.entries.push_back(WebGpuError {
            device: call.device,
            error_type: error.kind.to_owned(),
            message: error.message.clone(),
            api_call: call.api_call.clone(),
            object_label: call.object_label.clone(),
            uncaptured,
        });
    }

    fn frame(&self, mut frame: WebGpuFrame) {
        let mut frames = lock(&self.frames);
        frame.sequence = self.sequence.fetch_add(1, Ordering::Relaxed) + 1;
        if frames.len() == FRAME_LIMIT {
            frames.pop_front();
        }
        frames.push_back(Arc::new(frame));
    }

    fn take_errors(&self) -> WebGpuErrors {
        let mut log = lock(&self.errors);
        WebGpuErrors {
            since_previous: log.entries.drain(..).collect(),
            total: log.total,
            validation_total: log.validation_total,
            uncaptured_total: log.uncaptured_total,
            dropped_since_previous: std::mem::take(&mut log.dropped),
        }
    }

    fn frame_snapshots(&self) -> Vec<Arc<WebGpuFrame>> {
        lock(&self.frames).iter().cloned().collect()
    }
}

pub(super) struct DeviceState {
    pub device: WebGpuDevice,
    objects: HashMap<u64, Arc<WebGpuResource>>,
    labels: HashMap<u64, String>,
    canvases: HashMap<u64, String>,
    current_call: Option<Call>,
    pending: VecDeque<WebGpuFrame>,
}

impl DeviceState {
    pub(super) fn new(device: WebGpuDevice) -> Self {
        Self {
            device,
            objects: HashMap::new(),
            labels: HashMap::new(),
            canvases: HashMap::new(),
            current_call: None,
            pending: VecDeque::new(),
        }
    }

    fn label(&self, handle: u64) -> String {
        self.labels.get(&handle).cloned().unwrap_or_default()
    }

    fn stage_entry(&self, stage: Option<&Value>, kind: &str) -> Option<String> {
        let stage = stage?;
        if let Some(name) = stage.get("entryPoint").and_then(Value::as_str) {
            return Some(name.to_owned());
        }
        let module = stage.get("module")?.as_u64()?;
        let shader = self.objects.get(&module)?;
        let WebGpuResourceDetails::ShaderModule { entry_points, .. } = &shader.details else {
            return None;
        };
        let mut matches = entry_points.iter().filter(|entry| entry.stage == kind);
        let first = matches.next()?;
        matches.next().is_none().then(|| first.name.clone())
    }

    fn observe(&mut self, command: &Command) {
        let args = &command.args;
        let label = args
            .get("label")
            .or_else(|| args.get("descriptor").and_then(|value| value.get("label")))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let details = match command.op.as_str() {
            "createBuffer" => Some(WebGpuResourceDetails::Buffer {
                size: number(args, "size"),
                usage: number(args, "usage") as u32,
            }),
            "createTexture" => Some(WebGpuResourceDetails::Texture {
                size: args
                    .get("size")
                    .cloned()
                    .and_then(|value| serde_json::from_value(value).ok())
                    .unwrap_or_default(),
                format: string(args, "format"),
                mip_levels: number(args, "mipLevelCount") as u32,
                sample_count: number(args, "sampleCount") as u32,
                usage: number(args, "usage") as u32,
            }),
            "createSampler" => Some(WebGpuResourceDetails::Sampler),
            "createShaderModule" => Some(WebGpuResourceDetails::ShaderModule {
                entry_points: Vec::new(),
                compilation_messages: Vec::new(),
            }),
            "createRenderPipeline" | "createRenderPipelineAsync" => {
                Some(WebGpuResourceDetails::RenderPipeline {
                    vertex_entry_point: self.stage_entry(args.get("vertex"), "vertex"),
                    fragment_entry_point: self.stage_entry(args.get("fragment"), "fragment"),
                    color_targets: args
                        .get("fragment")
                        .and_then(|value| value.get("targets"))
                        .and_then(Value::as_array)
                        .map(|targets| {
                            targets
                                .iter()
                                .map(|target| {
                                    target
                                        .get("format")
                                        .and_then(Value::as_str)
                                        .map(str::to_owned)
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                })
            }
            "createComputePipeline" | "createComputePipelineAsync" => {
                Some(WebGpuResourceDetails::ComputePipeline {
                    compute_entry_point: self.stage_entry(args.get("compute"), "compute"),
                })
            }
            "createBindGroup" => Some(WebGpuResourceDetails::BindGroup),
            "createBindGroupLayout" | "getBindGroupLayout" => {
                Some(WebGpuResourceDetails::BindGroupLayout)
            }
            "createPipelineLayout" => Some(WebGpuResourceDetails::PipelineLayout),
            _ => None,
        };
        if command.handle != 0
            && (details.is_some()
                || matches!(
                    command.op.as_str(),
                    "createView"
                        | "createQuerySet"
                        | "createCommandEncoder"
                        | "createRenderBundleEncoder"
                        | "finish"
                        | "finishRenderBundle"
                ))
        {
            self.labels.entry(command.handle).or_insert(label.clone());
        }
        if let Some(details) = details {
            self.objects.entry(command.handle).or_insert_with(|| {
                Arc::new(WebGpuResource {
                    device: self.device.handle,
                    handle: command.handle,
                    label,
                    details,
                })
            });
        }
    }
}

fn number(value: &Value, name: &str) -> u64 {
    value.get(name).and_then(Value::as_u64).unwrap_or_default()
}

fn string(value: &Value, name: &str) -> String {
    value
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}

pub(super) fn device_report(
    handle: u64,
    adapter: &wgpu::Adapter,
    label: &str,
    device: &wgpu::Device,
) -> Result<WebGpuDevice, String> {
    let enabled = limits_value(&device.limits())?;
    let defaults = limits_value(&wgpu::Limits::default())?;
    let limits: BTreeMap<_, _> = LIMIT_NAMES
        .iter()
        .filter_map(|name| {
            let value = enabled.get(*name)?.as_u64()?;
            (defaults.get(*name).and_then(Value::as_u64) != Some(value))
                .then(|| ((*name).to_owned(), value))
        })
        .collect();
    let info = adapter.get_info();
    Ok(WebGpuDevice {
        handle,
        adapter_name: info.name,
        backend: format!("{:?}", info.backend).to_ascii_lowercase(),
        label: label.to_owned(),
        enabled_features: feature_names(device.features())
            .into_iter()
            .map(str::to_owned)
            .collect(),
        limits,
        lost: false,
        lost_reason: None,
    })
}

struct InspectorHost(Bridge);

impl Bridge {
    pub(crate) fn install_inspector(&self, document: &mut blitz_script::ScriptDocument) {
        document.with_js_context(|context| {
            context.insert_data(InspectorHost(self.clone()));
        });
    }

    fn inspection_session(&self) -> Arc<Session> {
        Arc::clone(&self.state().inspection)
    }

    pub(super) fn inspection_call(&self, request: &Request) -> Option<Call> {
        match request {
            Request::Adapter { id, .. } => Some(Call {
                device: *id,
                api_call: "GPU.requestAdapter".to_owned(),
                object_label: String::new(),
            }),
            Request::Device { id, options, .. } => Some(Call {
                device: *id,
                api_call: "GPUAdapter.requestDevice".to_owned(),
                object_label: options.label.clone(),
            }),
            Request::Command {
                device, command, ..
            } if *device != 0 => {
                let connection = self.state().devices.get(device).cloned();
                Some(match connection {
                    Some(connection) => connection.inspection_call(command),
                    None => Call {
                        device: *device,
                        api_call: command.op.clone(),
                        object_label: string(&command.args, "label"),
                    },
                })
            }
            _ => None,
        }
    }

    pub(super) fn record_request_error(&self, request: &Request, error: &GpuError) {
        if let Some(call) = self.inspection_call(request) {
            self.record_inspection_error(&call, error, false);
        }
    }

    pub(super) fn record_inspection_error(
        &self,
        call: &Call,
        error: &GpuError,
        route_to_page: bool,
    ) {
        let (session, connection) = {
            let state = self.state();
            (
                Arc::clone(&state.inspection),
                state.devices.get(&call.device).cloned(),
            )
        };
        for error in std::iter::once(error).chain(&error.other) {
            let scoped = connection.as_ref().is_some_and(|connection| {
                lock(&connection.scopes)
                    .iter()
                    .any(|scope| scope.filter == error.kind)
            });
            session.error(call, error, route_to_page && !scoped);
        }
    }

    fn inspect(&self) -> WebGpuReport {
        let (session, devices) = {
            let state = self.state();
            (
                Arc::clone(&state.inspection),
                state.devices.values().cloned().collect::<Vec<_>>(),
            )
        };
        session.renew();
        let mut report = WebGpuReport::default();
        for connection in devices {
            let resources = connection.resources();
            let mut inspection = lock(&connection.inspection);
            inspection
                .objects
                .retain(|handle, _| resources.live(*handle));
            inspection
                .labels
                .retain(|handle, _| resources.live(*handle));
            inspection
                .canvases
                .retain(|handle, _| resources.canvases.contains_key(handle));
            let mut device = inspection.device.clone();
            let reason = lock(&connection.loss_reason);
            device.lost = connection.lost.load(Ordering::Acquire);
            device.lost_reason = reason.clone();
            drop(reason);
            report.devices.push(device);
            let mut snapshots = inspection.objects.values().cloned().collect::<Vec<_>>();
            // Canvas textures are created by currentTexture rather than by a
            // page createTexture descriptor. Their native registry is authoritative.
            for (&handle, entry) in &resources.textures {
                if inspection.objects.contains_key(&handle) {
                    continue;
                }
                let texture = &entry.texture;
                let usage = entry
                    .canvas
                    .and_then(|(canvas, _)| resources.canvases.get(&canvas))
                    .map_or(texture.usage().bits(), |configuration| configuration.usage);
                let format = serde_json::to_value(texture.format())
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .unwrap_or_else(|| format!("{:?}", texture.format()));
                snapshots.push(Arc::new(WebGpuResource {
                    device: connection.id,
                    handle,
                    label: inspection
                        .labels
                        .get(&handle)
                        .cloned()
                        .unwrap_or_else(|| "chuzz canvas current texture".to_owned()),
                    details: WebGpuResourceDetails::Texture {
                        size: [
                            texture.width(),
                            texture.height(),
                            texture.depth_or_array_layers(),
                        ],
                        format,
                        mip_levels: texture.mip_level_count(),
                        sample_count: texture.sample_count(),
                        usage,
                    },
                }));
            }
            drop(inspection);
            drop(resources);
            report
                .resources
                .extend(snapshots.iter().map(|value| (**value).clone()));
        }
        report.devices.sort_by_key(|device| device.handle);
        report
            .resources
            .sort_by_key(|resource| (resource.device, resource.handle));
        let frames = session.frame_snapshots();
        report.frames = frames.iter().map(|frame| (**frame).clone()).collect();
        report.errors = session.take_errors();
        report
    }
}

pub(super) fn inspect_document(
    document: &mut blitz_script::ScriptDocument,
) -> Result<WebGpuReport, DebugError> {
    let bridge = document.with_js_context(|context| {
        context
            .get_data::<InspectorHost>()
            .map(|host| host.0.clone())
    });
    bridge
        .map(|bridge| bridge.inspect())
        .ok_or_else(|| DebugError {
            code: "unsupported".to_owned(),
            message: "the document has no page WebGPU inspector".to_owned(),
        })
}

impl DeviceConnection {
    pub(super) fn inspection_call(&self, command: &Command) -> Call {
        let inspection = lock(&self.inspection);
        let handle = if command.handle != 0 {
            command.handle
        } else {
            ["texture", "buffer", "pipeline", "encoder"]
                .iter()
                .find_map(|name| command.args.get(*name).and_then(Value::as_u64))
                .unwrap_or(0)
        };
        let label = command
            .args
            .get("label")
            .and_then(Value::as_str)
            .filter(|label| !label.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                let label = inspection.label(handle);
                if label.is_empty() {
                    inspection.label(number(&command.args, "encoder"))
                } else {
                    label
                }
            });
        let owner = match command.op.as_str() {
            "submit"
            | "writeBuffer"
            | "writeTexture"
            | "copyExternalImageToTexture"
            | "onSubmittedWorkDone" => "GPUQueue",
            "finish" => "GPUCommandEncoder",
            "finishRenderBundle" => "GPURenderBundleEncoder",
            "configure" | "unconfigure" | "currentTexture" | "present" => "GPUCanvasContext",
            _ => "GPUDevice",
        };
        Call {
            device: self.id,
            api_call: format!("{owner}.{}", command.op),
            object_label: label,
        }
    }

    pub(super) fn begin_inspection_call(&self, command: &Command) {
        let call = self.inspection_call(command);
        let mut inspection = lock(&self.inspection);
        inspection.current_call = Some(call);
        inspection.observe(command);
    }

    pub(super) fn end_inspection_call(&self) {
        lock(&self.inspection).current_call = None;
    }

    pub(super) fn record_native_error(&self, error: &GpuError) {
        let call = lock(&self.inspection)
            .current_call
            .clone()
            .unwrap_or_else(|| Call {
                device: self.id,
                api_call: "native WebGPU callback".to_owned(),
                object_label: String::new(),
            });
        if let Some(owner) = self.owner.upgrade() {
            Bridge(owner).record_inspection_error(&call, error, true);
        }
    }

    pub(super) fn inspection_active(&self) -> bool {
        self.owner
            .upgrade()
            .is_some_and(|owner| Bridge(owner).inspection_session().active())
    }

    pub(super) fn inspection_label(&self, handle: u64) -> String {
        lock(&self.inspection).label(handle)
    }

    pub(super) fn inspection_target(&self, resources: &Resources, view: u64) -> String {
        let Some((texture, _)) = resources.views.get(&view) else {
            return format!("view:@{view}");
        };
        let inspection = lock(&self.inspection);
        if let Some((canvas, _)) = resources
            .textures
            .get(texture)
            .and_then(|entry| entry.canvas)
        {
            return inspection
                .canvases
                .get(&canvas)
                .cloned()
                .unwrap_or_else(|| format!("canvas:@{canvas}"));
        }
        let label = inspection.label(*texture);
        if label.is_empty() {
            format!("texture:@{texture}")
        } else {
            label
        }
    }

    pub(super) fn update_shader_inspection(
        &self,
        handle: u64,
        entry_points: Vec<WebGpuEntryPoint>,
        compilation_messages: Vec<WebGpuCompilationMessage>,
    ) {
        if let Some(resource) = lock(&self.inspection).objects.get_mut(&handle) {
            Arc::make_mut(resource).details = WebGpuResourceDetails::ShaderModule {
                entry_points,
                compilation_messages,
            };
        }
    }

    pub(super) fn inspection_command(&self, command: &Command) -> Option<Result<Value, GpuError>> {
        match command.op.as_str() {
            "setLabel" => {
                let Some(label) = command.args.as_str() else {
                    return Some(Err(GpuError::validation("invalid GPUObjectBase.label")));
                };
                let mut inspection = lock(&self.inspection);
                if command.handle == self.id {
                    inspection.device.label = label.to_owned();
                } else {
                    inspection.labels.insert(command.handle, label.to_owned());
                    if let Some(resource) = inspection.objects.get_mut(&command.handle) {
                        Arc::make_mut(resource).label = label.to_owned();
                    }
                }
                Some(Ok(Value::Null))
            }
            "setCanvasId" => {
                let Some(name) = command.args.as_str() else {
                    return Some(Err(GpuError::validation("invalid canvas inspector id")));
                };
                lock(&self.inspection)
                    .canvases
                    .insert(command.handle, name.to_owned());
                Some(Ok(Value::Null))
            }
            "inspectionFlush" => {
                self.inspection_flush(false);
                Some(Ok(Value::Null))
            }
            _ => None,
        }
    }

    pub(super) fn inspection_submit(&self, frame: WebGpuFrame) {
        let mut inspection = lock(&self.inspection);
        if inspection.pending.len() == FRAME_LIMIT {
            inspection.pending.pop_front();
            if let Some(first) = inspection.pending.front_mut() {
                first.complete = false;
            }
        }
        inspection.pending.push_back(frame);
    }

    pub(super) fn inspection_discard_pending(&self) {
        lock(&self.inspection).pending.clear();
    }

    pub(super) fn inspection_flush(&self, presented: bool) {
        let pending = std::mem::take(&mut lock(&self.inspection).pending);
        let Some(owner) = self.owner.upgrade() else {
            return;
        };
        let session = Bridge(owner).inspection_session();
        if !presented {
            for frame in pending {
                session.frame(frame);
            }
            return;
        }
        if pending.is_empty() {
            return;
        }
        let mut frame = WebGpuFrame {
            device: self.id,
            complete: true,
            ..Default::default()
        };
        for submit in pending {
            frame.submits = frame.submits.saturating_add(submit.submits);
            frame.complete &= submit.complete;
            let available = PASS_LIMIT.saturating_sub(frame.passes.len());
            frame.complete &= submit.passes.len() <= available;
            frame
                .passes
                .extend(submit.passes.into_iter().take(available));
        }
        session.frame(frame);
    }
}
