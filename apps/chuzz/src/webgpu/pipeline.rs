use std::borrow::Cow;
use std::num::NonZeroU64;

use super::resources::{ALLOCATION_LIMIT, checked, decode};
use super::*;

pub(super) struct Shader {
    module: Arc<wgpu::ShaderModule>,
    source: Arc<str>,
    error: Option<String>,
}

#[derive(Default)]
pub(super) struct Objects {
    shaders: HashMap<u64, Shader>,
    pub layouts: HashMap<u64, Arc<wgpu::BindGroupLayout>>,
    pipeline_layouts: HashMap<u64, Arc<wgpu::PipelineLayout>>,
    pub groups: HashMap<u64, Arc<wgpu::BindGroup>>,
    pub render: HashMap<u64, Arc<wgpu::RenderPipeline>>,
    pub compute: HashMap<u64, Arc<wgpu::ComputePipeline>>,
}

impl Objects {
    pub(super) fn contains(&self, handle: u64) -> bool {
        self.shaders.contains_key(&handle)
            || self.layouts.contains_key(&handle)
            || self.pipeline_layouts.contains_key(&handle)
            || self.groups.contains_key(&handle)
            || self.render.contains_key(&handle)
            || self.compute.contains_key(&handle)
    }

    pub(super) fn count(&self) -> usize {
        self.shaders.len()
            + self.layouts.len()
            + self.pipeline_layouts.len()
            + self.groups.len()
            + self.render.len()
            + self.compute.len()
    }

    fn remove(&mut self, handle: u64) {
        self.layouts.remove(&handle);
        self.pipeline_layouts.remove(&handle);
        self.groups.remove(&handle);
        self.render.remove(&handle);
        self.compute.remove(&handle);
    }
}

pub(super) fn get<'a, T>(
    objects: &'a HashMap<u64, T>,
    handle: u64,
    member: &str,
) -> Result<&'a T, GpuError> {
    objects
        .get(&handle)
        .ok_or_else(|| GpuError::validation(format!("invalid or destroyed {member}")))
}

fn view_dimension(name: &str) -> Result<wgpu::TextureViewDimension, GpuError> {
    match name {
        "1d" => Ok(wgpu::TextureViewDimension::D1),
        "2d" => Ok(wgpu::TextureViewDimension::D2),
        "2d-array" => Ok(wgpu::TextureViewDimension::D2Array),
        "cube" => Ok(wgpu::TextureViewDimension::Cube),
        "cube-array" => Ok(wgpu::TextureViewDimension::CubeArray),
        "3d" => Ok(wgpu::TextureViewDimension::D3),
        _ => Err(GpuError::validation("invalid GPUTextureViewDimension")),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ShaderOptions {
    label: String,
    code: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BufferLayout {
    #[serde(rename = "type")]
    kind: String,
    has_dynamic_offset: bool,
    min_binding_size: u64,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SamplerLayout {
    #[serde(rename = "type")]
    kind: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TextureLayout {
    sample_type: String,
    view_dimension: String,
    multisampled: bool,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StorageLayout {
    access: String,
    format: String,
    view_dimension: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LayoutEntry {
    binding: u32,
    visibility: u32,
    buffer: Option<BufferLayout>,
    sampler: Option<SamplerLayout>,
    texture: Option<TextureLayout>,
    storage_texture: Option<StorageLayout>,
}

impl LayoutEntry {
    fn native(&self) -> Result<wgpu::BindGroupLayoutEntry, GpuError> {
        if [
            self.buffer.is_some(),
            self.sampler.is_some(),
            self.texture.is_some(),
            self.storage_texture.is_some(),
        ]
        .into_iter()
        .filter(|present| *present)
        .count()
            != 1
        {
            return Err(GpuError::validation(
                "a binding must have exactly one resource type",
            ));
        }
        let ty = if let Some(buffer) = &self.buffer {
            let ty = match buffer.kind.as_str() {
                "uniform" => wgpu::BufferBindingType::Uniform,
                "storage" => wgpu::BufferBindingType::Storage { read_only: false },
                "read-only-storage" => wgpu::BufferBindingType::Storage { read_only: true },
                _ => return Err(GpuError::validation("invalid GPUBufferBindingType")),
            };
            wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: buffer.has_dynamic_offset,
                min_binding_size: NonZeroU64::new(buffer.min_binding_size),
            }
        } else if let Some(sampler) = &self.sampler {
            let ty = match sampler.kind.as_str() {
                "filtering" => wgpu::SamplerBindingType::Filtering,
                "non-filtering" => wgpu::SamplerBindingType::NonFiltering,
                "comparison" => wgpu::SamplerBindingType::Comparison,
                _ => return Err(GpuError::validation("invalid GPUSamplerBindingType")),
            };
            wgpu::BindingType::Sampler(ty)
        } else if let Some(texture) = &self.texture {
            let sample_type = match texture.sample_type.as_str() {
                "float" => wgpu::TextureSampleType::Float { filterable: true },
                "unfilterable-float" => wgpu::TextureSampleType::Float { filterable: false },
                "depth" => wgpu::TextureSampleType::Depth,
                "sint" => wgpu::TextureSampleType::Sint,
                "uint" => wgpu::TextureSampleType::Uint,
                _ => return Err(GpuError::validation("invalid GPUTextureSampleType")),
            };
            wgpu::BindingType::Texture {
                sample_type,
                view_dimension: view_dimension(&texture.view_dimension)?,
                multisampled: texture.multisampled,
            }
        } else {
            let storage = self
                .storage_texture
                .as_ref()
                .ok_or_else(|| GpuError::validation("missing storage texture layout"))?;
            let access = match storage.access.as_str() {
                "write-only" => wgpu::StorageTextureAccess::WriteOnly,
                "read-only" => wgpu::StorageTextureAccess::ReadOnly,
                "read-write" => wgpu::StorageTextureAccess::ReadWrite,
                _ => return Err(GpuError::validation("invalid GPUStorageTextureAccess")),
            };
            wgpu::BindingType::StorageTexture {
                access,
                format: texture::format(&storage.format)?,
                view_dimension: view_dimension(&storage.view_dimension)?,
            }
        };
        Ok(wgpu::BindGroupLayoutEntry {
            binding: self.binding,
            visibility: wgpu::ShaderStages::from_bits(self.visibility)
                .ok_or_else(|| GpuError::validation("invalid GPUShaderStage"))?,
            ty,
            count: None,
        })
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LayoutOptions {
    label: String,
    entries: Vec<LayoutEntry>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PipelineLayoutOptions {
    label: String,
    bind_group_layouts: Vec<Option<u64>>,
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum Binding {
    Buffer {
        handle: u64,
        offset: u64,
        size: Option<u64>,
    },
    Sampler {
        handle: u64,
    },
    View {
        handle: u64,
    },
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupEntry {
    binding: u32,
    resource: Binding,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupOptions {
    label: String,
    layout: u64,
    entries: Vec<GroupEntry>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Attribute {
    format: wgpu::VertexFormat,
    offset: u64,
    shader_location: u32,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VertexBuffer {
    array_stride: u64,
    step_mode: wgpu::VertexStepMode,
    attributes: Vec<Attribute>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BlendComponent {
    src_factor: wgpu::BlendFactor,
    dst_factor: wgpu::BlendFactor,
    operation: wgpu::BlendOperation,
}

impl BlendComponent {
    fn native(&self) -> wgpu::BlendComponent {
        wgpu::BlendComponent {
            src_factor: self.src_factor,
            dst_factor: self.dst_factor,
            operation: self.operation,
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Blend {
    color: BlendComponent,
    alpha: BlendComponent,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Target {
    format: String,
    blend: Option<Blend>,
    write_mask: u32,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Stage {
    module: u64,
    entry_point: Option<String>,
    constants: HashMap<String, f64>,
    #[serde(default)]
    buffers: Vec<VertexBuffer>,
    #[serde(default)]
    targets: Vec<Option<Target>>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Primitive {
    topology: wgpu::PrimitiveTopology,
    strip_index_format: Option<wgpu::IndexFormat>,
    front_face: wgpu::FrontFace,
    cull_mode: Option<wgpu::Face>,
    unclipped_depth: bool,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StencilFace {
    compare: wgpu::CompareFunction,
    fail_op: wgpu::StencilOperation,
    depth_fail_op: wgpu::StencilOperation,
    pass_op: wgpu::StencilOperation,
}

impl StencilFace {
    fn native(&self) -> wgpu::StencilFaceState {
        wgpu::StencilFaceState {
            compare: self.compare,
            fail_op: self.fail_op,
            depth_fail_op: self.depth_fail_op,
            pass_op: self.pass_op,
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Depth {
    format: String,
    depth_write_enabled: Option<bool>,
    depth_compare: Option<wgpu::CompareFunction>,
    stencil_front: StencilFace,
    stencil_back: StencilFace,
    stencil_read_mask: u32,
    stencil_write_mask: u32,
    depth_bias: i32,
    depth_bias_slope_scale: f32,
    depth_bias_clamp: f32,
}

impl Depth {
    fn native(&self) -> Result<wgpu::DepthStencilState, GpuError> {
        Ok(wgpu::DepthStencilState {
            format: texture::format(&self.format)?,
            depth_write_enabled: self.depth_write_enabled,
            depth_compare: self.depth_compare,
            stencil: wgpu::StencilState {
                front: self.stencil_front.native(),
                back: self.stencil_back.native(),
                read_mask: self.stencil_read_mask,
                write_mask: self.stencil_write_mask,
            },
            bias: wgpu::DepthBiasState {
                constant: self.depth_bias,
                slope_scale: self.depth_bias_slope_scale,
                clamp: self.depth_bias_clamp,
            },
        })
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Multisample {
    count: u32,
    mask: u32,
    alpha_to_coverage_enabled: bool,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RenderOptions {
    label: String,
    layout: Option<u64>,
    vertex: Stage,
    fragment: Option<Stage>,
    primitive: Primitive,
    depth_stencil: Option<Depth>,
    multisample: Multisample,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ComputeOptions {
    label: String,
    layout: Option<u64>,
    compute: Stage,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct GetLayout {
    pipeline: u64,
    kind: String,
    index: u32,
}

impl DeviceConnection {
    pub(super) async fn execute_pipeline(
        &self,
        op: &str,
        handle: u64,
        args: Value,
    ) -> Result<Value, GpuError> {
        match op {
            "createShaderModule" => self.create_shader(handle, args).await,
            "getCompilationInfo" => self.compilation_info(handle).await,
            "createBindGroupLayout"
            | "createPipelineLayout"
            | "createBindGroup"
            | "createRenderPipeline"
            | "createRenderPipelineAsync"
            | "createComputePipeline"
            | "createComputePipelineAsync"
            | "getBindGroupLayout" => {
                self.resources().reserve(handle, 0)?;
                let result = checked(&self.device, || self.pipeline_sync(op, handle, args)).await;
                match result {
                    Ok(Ok(value)) => Ok(value),
                    Ok(Err(error)) | Err(error) => {
                        self.resources().pipelines.remove(handle);
                        Err(error)
                    }
                }
            }
            _ => Err(GpuError::validation(format!(
                "not implemented in chuzz: {op}"
            ))),
        }
    }

    async fn create_shader(&self, handle: u64, args: Value) -> Result<Value, GpuError> {
        let options: ShaderOptions = decode(args)?;
        if options.code.len() as u64 > ALLOCATION_LIMIT {
            return Err(GpuError::validation(
                "WebGPU shader source limit reached in chuzz",
            ));
        }
        self.resources().reserve(handle, 0)?;
        let source: Arc<str> = Arc::from(options.code);
        let mut module = None;
        let result = checked(&self.device, || {
            module = Some(Arc::new(self.device.create_shader_module(
                wgpu::ShaderModuleDescriptor {
                    label: Some(&options.label),
                    source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(&source)),
                },
            )));
        })
        .await;
        let error = result.err();
        if let Some(error) = &error {
            self.report(error);
        }
        let module = module.ok_or_else(|| GpuError::internal("shader creation did not return"))?;
        self.resources().pipelines.shaders.insert(
            handle,
            Shader {
                module,
                source,
                error: error.map(|error| error.message),
            },
        );
        // An invalid shader is still a GPUShaderModule. Its diagnostics must
        // remain queryable, rather than disappearing with a rejected creation.
        Ok(Value::Null)
    }

    async fn compilation_info(&self, handle: u64) -> Result<Value, GpuError> {
        let (module, source, error) = {
            let resources = self.resources();
            let shader = get(&resources.pipelines.shaders, handle, "GPUShaderModule")?;
            (
                Arc::clone(&shader.module),
                Arc::clone(&shader.source),
                shader.error.clone(),
            )
        };
        let info = module.get_compilation_info().await;
        let mut messages = Vec::with_capacity(info.messages.len());
        for message in info.messages {
            let kind = match message.message_type {
                wgpu::CompilationMessageType::Error => "error",
                wgpu::CompilationMessageType::Warning => "warning",
                wgpu::CompilationMessageType::Info => "info",
            };
            let (line, position, offset, length) = match message.location {
                Some(location) => {
                    let start = location.offset as usize;
                    let end = start.saturating_add(location.length as usize);
                    let before = source.get(..start).unwrap_or("");
                    let span = source.get(start..end).unwrap_or("");
                    let column = before.rsplit('\n').next().unwrap_or("");
                    (
                        location.line_number,
                        column.encode_utf16().count() + 1,
                        before.encode_utf16().count(),
                        span.encode_utf16().count(),
                    )
                }
                None => (0, 0, 0, 0),
            };
            messages.push(json!({
                "message": message.message,
                "type": kind,
                "lineNum": line,
                "linePos": position,
                "offset": offset,
                "length": length
            }));
        }
        if messages.is_empty()
            && let Some(error) = error
        {
            messages.push(json!({
                "message": error,
                "type": "error",
                "lineNum": 0,
                "linePos": 0,
                "offset": 0,
                "length": 0
            }));
        }
        Ok(json!({"messages": messages}))
    }

    fn pipeline_sync(&self, op: &str, handle: u64, args: Value) -> Result<Value, GpuError> {
        let mut resources = self.resources();
        match op {
            "createBindGroupLayout" => {
                let options: LayoutOptions = decode(args)?;
                let entries = options
                    .entries
                    .iter()
                    .map(LayoutEntry::native)
                    .collect::<Result<Vec<_>, _>>()?;
                let layout =
                    self.device
                        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                            label: Some(&options.label),
                            entries: &entries,
                        });
                resources.pipelines.layouts.insert(handle, Arc::new(layout));
            }
            "createPipelineLayout" => {
                let options: PipelineLayoutOptions = decode(args)?;
                let layouts = options
                    .bind_group_layouts
                    .iter()
                    .map(|handle| {
                        handle
                            .map(|handle| {
                                get(&resources.pipelines.layouts, handle, "GPUBindGroupLayout")
                                    .map(Arc::as_ref)
                            })
                            .transpose()
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let layout = self
                    .device
                    .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: Some(&options.label),
                        bind_group_layouts: &layouts,
                        immediate_size: 0,
                    });
                resources
                    .pipelines
                    .pipeline_layouts
                    .insert(handle, Arc::new(layout));
            }
            "createBindGroup" => {
                let options: GroupOptions = decode(args)?;
                let layout = get(
                    &resources.pipelines.layouts,
                    options.layout,
                    "GPUBindGroupLayout",
                )?;
                let entries = options
                    .entries
                    .iter()
                    .map(|entry| {
                        let resource = match &entry.resource {
                            Binding::Buffer {
                                handle,
                                offset,
                                size,
                            } => {
                                let buffer = get(&resources.buffers, *handle, "GPUBuffer")?;
                                let size =
                                    size.unwrap_or_else(|| buffer.size.saturating_sub(*offset));
                                if size == 0
                                    || offset.checked_add(size).is_none_or(|end| end > buffer.size)
                                {
                                    return Err(GpuError::validation(
                                        "invalid GPUBufferBinding range",
                                    ));
                                }
                                wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                    buffer: &buffer.buffer,
                                    offset: *offset,
                                    size: NonZeroU64::new(size),
                                })
                            }
                            Binding::Sampler { handle } => wgpu::BindingResource::Sampler(get(
                                &resources.samplers,
                                *handle,
                                "GPUSampler",
                            )?),
                            Binding::View { handle } => wgpu::BindingResource::TextureView(
                                &get(&resources.views, *handle, "GPUTextureView")?.1,
                            ),
                        };
                        Ok(wgpu::BindGroupEntry {
                            binding: entry.binding,
                            resource,
                        })
                    })
                    .collect::<Result<Vec<_>, GpuError>>()?;
                let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(&options.label),
                    layout,
                    entries: &entries,
                });
                resources.pipelines.groups.insert(handle, Arc::new(group));
            }
            "createRenderPipeline" | "createRenderPipelineAsync" => {
                let options: RenderOptions = decode(args)?;
                let vertex = get(
                    &resources.pipelines.shaders,
                    options.vertex.module,
                    "GPUShaderModule",
                )?;
                let layout = options
                    .layout
                    .map(|handle| {
                        get(
                            &resources.pipelines.pipeline_layouts,
                            handle,
                            "GPUPipelineLayout",
                        )
                        .map(Arc::as_ref)
                    })
                    .transpose()?;
                let attributes = options
                    .vertex
                    .buffers
                    .iter()
                    .map(|buffer| {
                        buffer
                            .attributes
                            .iter()
                            .map(|attribute| wgpu::VertexAttribute {
                                format: attribute.format,
                                offset: attribute.offset,
                                shader_location: attribute.shader_location,
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>();
                let buffers = options
                    .vertex
                    .buffers
                    .iter()
                    .zip(&attributes)
                    .map(|(buffer, attributes)| wgpu::VertexBufferLayout {
                        array_stride: buffer.array_stride,
                        step_mode: buffer.step_mode,
                        attributes,
                    })
                    .collect::<Vec<_>>();
                let vertex_constants = constants(&options.vertex);
                let fragment_constants =
                    options.fragment.as_ref().map(constants).unwrap_or_default();
                let targets = options
                    .fragment
                    .as_ref()
                    .map(|fragment| {
                        fragment
                            .targets
                            .iter()
                            .map(|target| {
                                target
                                    .as_ref()
                                    .map(|target| {
                                        Ok(wgpu::ColorTargetState {
                                            format: texture::format(&target.format)?,
                                            blend: target.blend.as_ref().map(|blend| {
                                                wgpu::BlendState {
                                                    color: blend.color.native(),
                                                    alpha: blend.alpha.native(),
                                                }
                                            }),
                                            write_mask: wgpu::ColorWrites::from_bits(
                                                target.write_mask,
                                            )
                                            .ok_or_else(|| {
                                                GpuError::validation("invalid GPUColorWrite")
                                            })?,
                                        })
                                    })
                                    .transpose()
                            })
                            .collect::<Result<Vec<_>, GpuError>>()
                    })
                    .transpose()?
                    .unwrap_or_default();
                let fragment = options
                    .fragment
                    .as_ref()
                    .map(|fragment| -> Result<_, GpuError> {
                        let shader = get(
                            &resources.pipelines.shaders,
                            fragment.module,
                            "GPUShaderModule",
                        )?;
                        Ok(wgpu::FragmentState {
                            module: &shader.module,
                            entry_point: fragment.entry_point.as_deref(),
                            compilation_options: wgpu::PipelineCompilationOptions {
                                constants: &fragment_constants,
                                ..Default::default()
                            },
                            targets: &targets,
                        })
                    })
                    .transpose()?;
                let native = self
                    .device
                    .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some(&options.label),
                        layout,
                        vertex: wgpu::VertexState {
                            module: &vertex.module,
                            entry_point: options.vertex.entry_point.as_deref(),
                            compilation_options: wgpu::PipelineCompilationOptions {
                                constants: &vertex_constants,
                                ..Default::default()
                            },
                            buffers: &buffers,
                        },
                        primitive: wgpu::PrimitiveState {
                            topology: options.primitive.topology,
                            strip_index_format: options.primitive.strip_index_format,
                            front_face: options.primitive.front_face,
                            cull_mode: options.primitive.cull_mode,
                            unclipped_depth: options.primitive.unclipped_depth,
                            ..Default::default()
                        },
                        depth_stencil: options
                            .depth_stencil
                            .as_ref()
                            .map(Depth::native)
                            .transpose()?,
                        multisample: wgpu::MultisampleState {
                            count: options.multisample.count,
                            mask: u64::from(options.multisample.mask),
                            alpha_to_coverage_enabled: options
                                .multisample
                                .alpha_to_coverage_enabled,
                        },
                        fragment,
                        multiview_mask: None,
                        cache: None,
                    });
                resources.pipelines.render.insert(handle, Arc::new(native));
            }
            "createComputePipeline" | "createComputePipelineAsync" => {
                let options: ComputeOptions = decode(args)?;
                let shader = get(
                    &resources.pipelines.shaders,
                    options.compute.module,
                    "GPUShaderModule",
                )?;
                let layout = options
                    .layout
                    .map(|handle| {
                        get(
                            &resources.pipelines.pipeline_layouts,
                            handle,
                            "GPUPipelineLayout",
                        )
                        .map(Arc::as_ref)
                    })
                    .transpose()?;
                let constants = constants(&options.compute);
                let native =
                    self.device
                        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                            label: Some(&options.label),
                            layout,
                            module: &shader.module,
                            entry_point: options.compute.entry_point.as_deref(),
                            compilation_options: wgpu::PipelineCompilationOptions {
                                constants: &constants,
                                ..Default::default()
                            },
                            cache: None,
                        });
                resources.pipelines.compute.insert(handle, Arc::new(native));
            }
            "getBindGroupLayout" => {
                let options: GetLayout = decode(args)?;
                if options.index >= self.device.limits().max_bind_groups {
                    return Err(GpuError::validation("invalid bind group layout index"));
                }
                let layout = match options.kind.as_str() {
                    "render" => get(
                        &resources.pipelines.render,
                        options.pipeline,
                        "GPURenderPipeline",
                    )?
                    .get_bind_group_layout(options.index),
                    "compute" => get(
                        &resources.pipelines.compute,
                        options.pipeline,
                        "GPUComputePipeline",
                    )?
                    .get_bind_group_layout(options.index),
                    _ => return Err(GpuError::validation("invalid GPU pipeline type")),
                };
                resources.pipelines.layouts.insert(handle, Arc::new(layout));
            }
            _ => {
                return Err(GpuError::validation(format!(
                    "not implemented in chuzz: {op}"
                )));
            }
        }
        Ok(Value::Null)
    }
}

fn constants(stage: &Stage) -> Vec<(&str, f64)> {
    stage
        .constants
        .iter()
        .map(|(name, value)| (name.as_str(), *value))
        .collect()
}
