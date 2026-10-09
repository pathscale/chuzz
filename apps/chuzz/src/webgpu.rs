//! The first page WebGPU slice: real adapters, devices and queues.
//!
//! Boa's public document interface provides IPC and polling rather than host
//! function registration. Share that channel with the existing bridges.
//! Native objects remain owned by this document's bridge; JavaScript receives
//! handles and capability snapshots, not simulated GPU resources.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde_json::{Map, Value, json};

pub(crate) const WEBGPU_SHIM: &str = include_str!("webgpu.ts");

const DEADLINE: Duration = Duration::from_secs(10);
const OBJECT_LIMIT: usize = 64;

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
    "maxComputeWorkgroupSizeZ",
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
    ("timestamp-query", wgpu::Features::TIMESTAMP_QUERY),
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
}

impl Request {
    fn id(&self) -> u64 {
        match self {
            Self::Adapter { id, .. } | Self::Device { id, .. } => *id,
        }
    }
}

/// Retain the native connection for subsequent resource and command slices.
/// There is no device-destroy API in this slice; document teardown releases it.
struct DeviceConnection {
    _device: Arc<wgpu::Device>,
    _queue: Arc<wgpu::Queue>,
}

#[derive(Default)]
struct State {
    adapters: HashMap<u64, Arc<wgpu::Adapter>>,
    devices: HashMap<u64, Arc<DeviceConnection>>,
    ready: Vec<(u64, Value)>,
}

#[derive(Clone, Default)]
pub(crate) struct Bridge(Arc<Mutex<State>>);

impl Bridge {
    fn state(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    pub(crate) fn handle(&self, message: &str) -> bool {
        let Ok(request) = serde_json::from_str::<Request>(message) else {
            return false;
        };
        let bridge = self.clone();
        let id = request.id();
        let _ = nagoya::spawn(async move {
            let result = match nagoya::timeout(DEADLINE, bridge.execute(request)).await {
                Ok(result) => result,
                Err(_) => Err("WebGPU request timed out in chuzz".to_owned()),
            };
            let payload = match result {
                Ok(value) => json!({"ok": true, "value": value}),
                Err(error) => json!({"ok": false, "error": error}),
            };
            bridge.state().ready.push((id, payload));
        });
        true
    }

    pub(crate) fn drain_into(&self, document: &mut blitz_script::ScriptDocument) -> bool {
        let ready = std::mem::take(&mut self.state().ready);
        let changed = !ready.is_empty();
        for (id, payload) in ready {
            document.eval(&format!(
                "globalThis.__chuzzGpuResolve({id}, {payload});"
            ));
        }
        changed
    }

    async fn execute(&self, request: Request) -> Result<Value, String> {
        match request {
            Request::Adapter { id, options } => self.request_adapter(id, options).await,
            Request::Device {
                id,
                adapter,
                options,
            } => self.request_device(id, adapter, options).await,
        }
    }

    async fn request_adapter(&self, id: u64, options: AdapterOptions) -> Result<Value, String> {
        let power_preference = match options.power_preference.as_deref() {
            None => wgpu::PowerPreference::None,
            Some("low-power") => wgpu::PowerPreference::LowPower,
            Some("high-performance") => wgpu::PowerPreference::HighPerformance,
            Some(_) => return Err("invalid GPURequestAdapterOptions.powerPreference".to_owned()),
        };
        if self.state().adapters.len() + self.state().devices.len() >= OBJECT_LIMIT {
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
            "isFallbackAdapter": adapter.get_info().device_type == wgpu::DeviceType::Cpu,
        });
        let mut state = self.state();
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
        });
        let mut state = self.state();
        if state.adapters.len() + state.devices.len() >= OBJECT_LIMIT {
            return Err("WebGPU object limit reached in chuzz".to_owned());
        }
        state.devices.insert(
            id,
            Arc::new(DeviceConnection {
                _device: Arc::new(device),
                _queue: Arc::new(queue),
            }),
        );
        Ok(value)
    }
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

