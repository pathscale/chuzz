use serde::de::DeserializeOwned;

use super::*;

pub(super) const RESOURCE_LIMIT: usize = 8192;
pub(super) const ALLOCATION_LIMIT: u64 = 64 * 1024 * 1024;
const DEVICE_BYTES_LIMIT: u64 = 512 * 1024 * 1024;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Command {
    pub op: String,
    #[serde(default)]
    pub handle: u64,
    #[serde(default)]
    pub args: Value,
}

pub(super) struct GpuError {
    pub kind: &'static str,
    pub message: String,
}

impl GpuError {
    pub fn validation(message: impl Into<String>) -> Self {
        Self {
            kind: "validation",
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            kind: "internal",
            message: message.into(),
        }
    }

    pub fn value(&self) -> Value {
        json!({"kind": self.kind, "message": self.message})
    }
}

impl From<wgpu::Error> for GpuError {
    fn from(error: wgpu::Error) -> Self {
        let kind = match &error {
            wgpu::Error::Validation { .. } => "validation",
            wgpu::Error::OutOfMemory { .. } => "out-of-memory",
            wgpu::Error::Internal { .. } => "internal",
        };
        Self {
            kind,
            message: error.to_string(),
        }
    }
}

pub(super) fn decode<T: DeserializeOwned>(value: Value) -> Result<T, GpuError> {
    serde_json::from_value(value)
        .map_err(|error| GpuError::validation(format!("invalid WebGPU descriptor: {error}")))
}

/// wgpu 29 scopes are thread-local and their guards are not Send. Push, use
/// and pop all guards in the synchronous block, before the first suspension.
/// The document worker routes the resulting error to its logical page scope
/// before processing the next command.
pub(super) async fn checked<T>(
    device: &wgpu::Device,
    operation: impl FnOnce() -> T,
) -> Result<T, GpuError> {
    let (value, internal, memory, validation) = {
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
        let value = operation();
        (value, internal.pop(), memory.pop(), validation.pop())
    };
    let errors = [internal.await, memory.await, validation.await];
    if let Some(error) = errors.into_iter().flatten().next() {
        Err(error.into())
    } else {
        Ok(value)
    }
}

#[derive(Default)]
pub(super) struct Resources {
    pub buffers: HashMap<u64, buffer::Entry>,
    pub textures: HashMap<u64, texture::Entry>,
    pub views: HashMap<u64, (u64, Arc<wgpu::TextureView>)>,
    pub samplers: HashMap<u64, Arc<wgpu::Sampler>>,
    pub canvases: HashMap<u64, canvas::Configuration>,
    pub pipelines: pipeline::Objects,
    pub encoding: encoder::Objects,
}

impl Resources {
    pub fn reserve(&self, handle: u64, bytes: u64) -> Result<(), GpuError> {
        if handle == 0
            || self.buffers.contains_key(&handle)
            || self.textures.contains_key(&handle)
            || self.views.contains_key(&handle)
            || self.samplers.contains_key(&handle)
            || self.pipelines.contains(handle)
            || self.encoding.contains(handle)
        {
            return Err(GpuError::validation("invalid or reused WebGPU handle"));
        }
        let count = self.buffers.len()
            + self.textures.len()
            + self.views.len()
            + self.samplers.len()
            + self.pipelines.count()
            + self.encoding.count();
        if count >= RESOURCE_LIMIT {
            return Err(GpuError::validation(
                "WebGPU resource limit reached in chuzz",
            ));
        }
        let allocated = self.buffers.values().map(|entry| entry.size).sum::<u64>()
            + self.textures.values().map(|entry| entry.bytes).sum::<u64>();
        if bytes > ALLOCATION_LIMIT || allocated.saturating_add(bytes) > DEVICE_BYTES_LIMIT {
            return Err(GpuError {
                kind: "out-of-memory",
                message: "WebGPU allocation limit reached in chuzz".to_owned(),
            });
        }
        Ok(())
    }

    pub fn remove_texture(&mut self, handle: u64) -> Option<texture::Entry> {
        self.views.retain(|_, (texture, _)| *texture != handle);
        self.textures.remove(&handle)
    }

    pub fn release_canvas(&mut self, canvas: u64) {
        let handles: Vec<_> = self
            .textures
            .iter()
            .filter_map(|(&handle, entry)| {
                entry.canvas.is_some_and(|(id, _)| id == canvas).then_some(handle)
            })
            .collect();
        for handle in handles {
            if let Some(entry) = self.remove_texture(handle) {
                entry.texture.destroy();
            }
        }
        self.canvases.remove(&canvas);
    }
}

impl DeviceConnection {
    pub(super) async fn execute_resource(&self, command: Command) -> Result<Value, GpuError> {
        let Command { op, handle, args } = command;
        match op.as_str() {
            "createBuffer" => self.create_buffer(handle, args).await,
            "writeBuffer" => self.write_buffer(handle, args).await,
            "mapRead" => self.map_buffer(handle, args).await,
            "unmap" => self.unmap_buffer(handle, args).await,
            "destroyBuffer" => {
                let entry = self.resources().buffers.remove(&handle);
                if let Some(entry) = entry {
                    entry.buffer.destroy();
                }
                Ok(Value::Null)
            }
            "createTexture" => self.create_texture(handle, args, None).await,
            "createView" => self.create_view(handle, args).await,
            "writeTexture" => self.write_texture(args).await,
            "destroyTexture" => {
                let entry = self.resources().remove_texture(handle);
                if let Some(entry) = entry {
                    entry.texture.destroy();
                }
                Ok(Value::Null)
            }
            "createSampler" => self.create_sampler(handle, args).await,
            "configure" => self.configure_canvas(handle, args),
            "unconfigure" => self.unconfigure_canvas(handle, args),
            "currentTexture" => self.current_texture(handle, args).await,
            "present" => self.present_canvas(handle, args).await,
            "createCommandEncoder" | "finish" | "submit" | "onSubmittedWorkDone" => {
                self.execute_encoding(&op, handle, args).await
            }
            _ => self.execute_pipeline(&op, handle, args).await,
        }
    }
}

