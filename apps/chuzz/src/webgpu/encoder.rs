use std::ops::Range;

use super::pipeline::get;
use super::resources::{checked, decode};
use super::*;

const COMMAND_LIMIT: usize = 65536;

#[derive(Default)]
pub(super) struct Objects {
    encoders: HashMap<u64, wgpu::CommandEncoder>,
    commands: HashMap<u64, wgpu::CommandBuffer>,
    // Native bundle encoders borrow resources and are not Send. Keep only
    // their owned descriptors between IPC calls, then replay at finish.
    bundle_encoders: HashMap<u64, BundleDescriptor>,
    bundles: HashMap<u64, Bundle>,
}

impl Objects {
    pub(super) fn contains(&self, handle: u64) -> bool {
        self.encoders.contains_key(&handle)
            || self.commands.contains_key(&handle)
            || self.bundle_encoders.contains_key(&handle)
            || self.bundles.contains_key(&handle)
    }

    pub(super) fn count(&self) -> usize {
        self.encoders.len() + self.commands.len() + self.bundle_encoders.len() + self.bundles.len()
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct EncoderOptions {
    label: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Finish {
    encoder: u64,
    label: String,
    operations: Vec<Operation>,
}

struct Bundle {
    native: Arc<wgpu::RenderBundle>,
    command_count: usize,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BundleOptions {
    label: String,
    color_formats: Vec<Option<String>>,
    depth_stencil_format: Option<String>,
    sample_count: u32,
    depth_read_only: bool,
    stencil_read_only: bool,
}

struct BundleDescriptor {
    label: String,
    color_formats: Vec<Option<wgpu::TextureFormat>>,
    depth_stencil: Option<wgpu::RenderBundleDepthStencil>,
    sample_count: u32,
}

impl BundleDescriptor {
    fn new(options: BundleOptions) -> Result<Self, GpuError> {
        let color_formats = options
            .color_formats
            .iter()
            .map(|format| format.as_deref().map(texture::format).transpose())
            .collect::<Result<Vec<_>, _>>()?;
        let depth_stencil = options
            .depth_stencil_format
            .as_deref()
            .map(|format| -> Result<_, GpuError> {
                Ok(wgpu::RenderBundleDepthStencil {
                    format: texture::format(format)?,
                    depth_read_only: options.depth_read_only,
                    stencil_read_only: options.stencil_read_only,
                })
            })
            .transpose()?;
        Ok(Self {
            label: options.label,
            color_formats,
            depth_stencil,
            sample_count: options.sample_count,
        })
    }

    fn native(&self) -> wgpu::RenderBundleEncoderDescriptor<'_> {
        wgpu::RenderBundleEncoderDescriptor {
            label: Some(&self.label),
            color_formats: &self.color_formats,
            depth_stencil: self.depth_stencil,
            sample_count: self.sample_count,
            ..Default::default()
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleFinish {
    encoder: u64,
    label: String,
    commands: Vec<PassCommand>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TextureCopy {
    texture: u64,
    mip_level: u32,
    origin: [u32; 3],
    aspect: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BufferCopy {
    buffer: u64,
    offset: u64,
    bytes_per_row: Option<u32>,
    rows_per_image: Option<u32>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ColorAttachment {
    view: u64,
    resolve_target: Option<u64>,
    depth_slice: Option<u32>,
    clear_value: [f64; 4],
    load_op: String,
    store_op: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DepthAttachment {
    view: u64,
    depth_clear_value: f32,
    depth_load_op: Option<String>,
    depth_store_op: Option<String>,
    depth_read_only: bool,
    stencil_clear_value: u32,
    stencil_load_op: Option<String>,
    stencil_store_op: Option<String>,
    stencil_read_only: bool,
}

#[derive(serde::Deserialize)]
#[serde(
    tag = "op",
    content = "args",
    rename_all = "camelCase",
    deny_unknown_fields
)]
enum Operation {
    Render {
        label: String,
        #[serde(rename = "colorAttachments")]
        color_attachments: Vec<Option<ColorAttachment>>,
        #[serde(rename = "depthStencilAttachment")]
        depth_stencil_attachment: Option<DepthAttachment>,
        #[serde(rename = "occlusionQuerySet")]
        occlusion_query_set: Option<u64>,
        commands: Vec<PassCommand>,
    },
    Compute {
        label: String,
        commands: Vec<PassCommand>,
    },
    ResolveQuerySet {
        query: u64,
        first: u32,
        count: u32,
        destination: u64,
        offset: u64,
    },
    ClearBuffer {
        buffer: u64,
        offset: u64,
        size: Option<u64>,
    },
    CopyBufferToBuffer {
        source: u64,
        #[serde(rename = "sourceOffset")]
        source_offset: u64,
        destination: u64,
        #[serde(rename = "destinationOffset")]
        destination_offset: u64,
        size: u64,
    },
    CopyBufferToTexture {
        source: BufferCopy,
        destination: TextureCopy,
        size: [u32; 3],
    },
    CopyTextureToBuffer {
        source: TextureCopy,
        destination: BufferCopy,
        size: [u32; 3],
    },
    CopyTextureToTexture {
        source: TextureCopy,
        destination: TextureCopy,
        size: [u32; 3],
    },
}

#[derive(serde::Deserialize)]
#[serde(
    tag = "op",
    content = "args",
    rename_all = "camelCase",
    deny_unknown_fields
)]
enum PassCommand {
    Pipeline(u64),
    BindGroup {
        index: u32,
        group: Option<u64>,
        offsets: Vec<u32>,
    },
    VertexBuffer {
        slot: u32,
        buffer: u64,
        offset: u64,
        size: Option<u64>,
    },
    IndexBuffer {
        buffer: u64,
        format: wgpu::IndexFormat,
        offset: u64,
        size: Option<u64>,
    },
    Draw {
        count: u32,
        instances: u32,
        first: u32,
        #[serde(rename = "firstInstance")]
        first_instance: u32,
    },
    DrawIndexed {
        count: u32,
        instances: u32,
        first: u32,
        #[serde(rename = "baseVertex")]
        base_vertex: i32,
        #[serde(rename = "firstInstance")]
        first_instance: u32,
    },
    DrawIndirect {
        buffer: u64,
        offset: u64,
        indexed: bool,
    },
    DispatchIndirect {
        buffer: u64,
        offset: u64,
    },
    BeginOcclusionQuery(u32),
    EndOcclusionQuery,
    Viewport([f32; 6]),
    Scissor([u32; 4]),
    BlendConstant([f64; 4]),
    StencilReference(u32),
    Dispatch([u32; 3]),
    ExecuteBundles(Vec<u64>),
}

fn color(value: [f64; 4]) -> wgpu::Color {
    wgpu::Color {
        r: value[0],
        g: value[1],
        b: value[2],
        a: value[3],
    }
}

fn operations<T: Copy>(load: &str, store: &str, clear: T) -> Result<wgpu::Operations<T>, GpuError> {
    Ok(wgpu::Operations {
        load: match load {
            "load" => wgpu::LoadOp::Load,
            "clear" => wgpu::LoadOp::Clear(clear),
            _ => return Err(GpuError::validation("invalid GPULoadOp")),
        },
        store: match store {
            "store" => wgpu::StoreOp::Store,
            "discard" => wgpu::StoreOp::Discard,
            _ => return Err(GpuError::validation("invalid GPUStoreOp")),
        },
    })
}

fn attachment_ops<T: Copy>(
    load: Option<&str>,
    store: Option<&str>,
    clear: T,
    read_only: bool,
) -> Result<Option<wgpu::Operations<T>>, GpuError> {
    if read_only {
        if load.is_some() || store.is_some() {
            return Err(GpuError::validation(
                "read-only attachment cannot have load/store operations",
            ));
        }
        return Ok(None);
    }
    match (load, store) {
        (None, None) => Ok(None),
        (Some(load), Some(store)) => operations(load, store, clear).map(Some),
        _ => Err(GpuError::validation(
            "attachment requires both loadOp and storeOp",
        )),
    }
}

fn draw_range(first: u32, count: u32) -> Result<Range<u32>, GpuError> {
    let end = first
        .checked_add(count)
        .ok_or_else(|| GpuError::validation("GPU draw range overflow"))?;
    Ok(first..end)
}

fn buffer_range(
    resources: &Resources,
    handle: u64,
    offset: u64,
    size: Option<u64>,
) -> Result<Range<u64>, GpuError> {
    let buffer = get(&resources.buffers, handle, "GPUBuffer")?;
    if buffer.mapped.is_some() {
        return Err(GpuError::validation("GPUBuffer is mapped"));
    }
    let size = size.unwrap_or_else(|| buffer.size.saturating_sub(offset));
    let end = offset
        .checked_add(size)
        .filter(|end| *end <= buffer.size)
        .ok_or_else(|| GpuError::validation("GPUBuffer range is out of bounds"))?;
    Ok(offset..end)
}

fn buffer_slice(
    resources: &Resources,
    handle: u64,
    offset: u64,
    size: Option<u64>,
) -> Result<wgpu::BufferSlice<'_>, GpuError> {
    let range = buffer_range(resources, handle, offset, size)?;
    if range.is_empty() {
        return Err(GpuError::validation("empty GPUBuffer binding"));
    }
    Ok(get(&resources.buffers, handle, "GPUBuffer")?
        .buffer
        .slice(range))
}

fn texture_copy<'a>(
    resources: &'a Resources,
    copy: &TextureCopy,
) -> Result<wgpu::TexelCopyTextureInfo<'a>, GpuError> {
    let texture = get(&resources.textures, copy.texture, "GPUTexture")?;
    let aspect = match copy.aspect.as_str() {
        "all" => wgpu::TextureAspect::All,
        "depth-only" => wgpu::TextureAspect::DepthOnly,
        "stencil-only" => wgpu::TextureAspect::StencilOnly,
        _ => return Err(GpuError::validation("invalid GPUTextureAspect")),
    };
    Ok(wgpu::TexelCopyTextureInfo {
        texture: &texture.texture,
        mip_level: copy.mip_level,
        origin: wgpu::Origin3d {
            x: copy.origin[0],
            y: copy.origin[1],
            z: copy.origin[2],
        },
        aspect,
    })
}

fn buffer_copy<'a>(
    resources: &'a Resources,
    copy: &BufferCopy,
) -> Result<wgpu::TexelCopyBufferInfo<'a>, GpuError> {
    buffer_range(resources, copy.buffer, copy.offset, Some(0))?;
    Ok(wgpu::TexelCopyBufferInfo {
        buffer: &get(&resources.buffers, copy.buffer, "GPUBuffer")?.buffer,
        layout: wgpu::TexelCopyBufferLayout {
            offset: copy.offset,
            bytes_per_row: copy.bytes_per_row,
            rows_per_image: copy.rows_per_image,
        },
    })
}

fn extent(size: [u32; 3]) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width: size[0],
        height: size[1],
        depth_or_array_layers: size[2],
    }
}

impl DeviceConnection {
    pub(super) async fn execute_encoding(
        &self,
        op: &str,
        handle: u64,
        args: Value,
    ) -> Result<Value, GpuError> {
        if op == "onSubmittedWorkDone" {
            return self.submitted_work_done().await;
        }
        let result = checked(&self.device, || self.encoding_sync(op, handle, args)).await;
        match result {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) | Err(error) => {
                let mut resources = self.resources();
                resources.encoding.encoders.remove(&handle);
                resources.encoding.commands.remove(&handle);
                resources.encoding.bundle_encoders.remove(&handle);
                resources.encoding.bundles.remove(&handle);
                Err(error)
            }
        }
    }

    async fn submitted_work_done(&self) -> Result<Value, GpuError> {
        let completed = Arc::new(AtomicBool::new(false));
        let callback = Arc::clone(&completed);
        self.queue.on_submitted_work_done(move || {
            callback.store(true, Ordering::Release);
        });
        loop {
            if self.lost.load(Ordering::Acquire) {
                return Err(GpuError::internal(
                    "GPUDevice was lost while waiting for submitted work",
                ));
            }
            self.device
                .poll(wgpu::PollType::Poll)
                .map_err(|error| GpuError::internal(format!("WebGPU poll failed: {error}")))?;
            if completed.load(Ordering::Acquire) {
                return Ok(Value::Null);
            }
            nagoya::sleep(Duration::from_millis(1)).await;
        }
    }

    fn encoding_sync(&self, op: &str, handle: u64, args: Value) -> Result<Value, GpuError> {
        let mut resources = self.resources();
        match op {
            "createCommandEncoder" => {
                let options: EncoderOptions = decode(args)?;
                resources.reserve(handle, 0)?;
                let encoder = self
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some(&options.label),
                    });
                resources.encoding.encoders.insert(handle, encoder);
            }
            "createRenderBundleEncoder" => {
                resources.reserve(handle, 0)?;
                let descriptor = BundleDescriptor::new(decode(args)?)?;
                if !matches!(descriptor.sample_count, 1 | 4) {
                    return Err(GpuError::validation("invalid render bundle sampleCount"));
                }
                if descriptor.color_formats.len()
                    > self.device.limits().max_color_attachments as usize
                {
                    return Err(GpuError::validation("too many render bundle color formats"));
                }
                // Validate now, without retaining a non-Send native encoder.
                drop(
                    self.device
                        .create_render_bundle_encoder(&descriptor.native()),
                );
                resources
                    .encoding
                    .bundle_encoders
                    .insert(handle, descriptor);
            }
            "finishRenderBundle" => {
                let finish: BundleFinish = decode(args)?;
                let descriptor = resources
                    .encoding
                    .bundle_encoders
                    .remove(&finish.encoder)
                    .ok_or_else(|| {
                        GpuError::validation("invalid or finished GPURenderBundleEncoder")
                    })?;
                resources.reserve(handle, 0)?;
                if finish.commands.len() > COMMAND_LIMIT {
                    return Err(GpuError::validation(
                        "WebGPU command limit reached in chuzz",
                    ));
                }
                let mut encoder = self
                    .device
                    .create_render_bundle_encoder(&descriptor.native());
                replay_bundle(&resources, &mut encoder, &finish.commands)?;
                let bundle = encoder.finish(&wgpu::RenderBundleDescriptor {
                    label: Some(&finish.label),
                });
                resources.encoding.bundles.insert(
                    handle,
                    Bundle {
                        native: Arc::new(bundle),
                        command_count: finish.commands.len(),
                    },
                );
            }
            "finish" => {
                let finish: Finish = decode(args)?;
                resources.reserve(handle, 0)?;
                let count = finish
                    .operations
                    .iter()
                    .try_fold(0_usize, |count, operation| {
                        let extra = match operation {
                            Operation::Render { commands, .. }
                            | Operation::Compute { commands, .. } => {
                                commands.iter().try_fold(0_usize, |count, command| {
                                    command_budget(count, pass_command_cost(&resources, command)?)
                                })?
                            }
                            _ => 0,
                        };
                        command_budget(count, extra + 1)
                    })?;
                if count > COMMAND_LIMIT {
                    return Err(GpuError::validation(
                        "WebGPU command limit reached in chuzz",
                    ));
                }
                let mut encoder = resources
                    .encoding
                    .encoders
                    .remove(&finish.encoder)
                    .ok_or_else(|| GpuError::validation("invalid or finished GPUCommandEncoder"))?;
                for operation in &finish.operations {
                    replay(&resources, &mut encoder, operation)?;
                }
                // wgpu's command buffer label is inherited from the encoder.
                // The JS GPUCommandBuffer owns the finish descriptor's label.
                let _label = finish.label;
                resources.encoding.commands.insert(handle, encoder.finish());
            }
            "submit" => {
                let handles: Vec<u64> = decode(args)?;
                if handles.len() > COMMAND_LIMIT {
                    return Err(GpuError::validation(
                        "WebGPU command buffer limit reached in chuzz",
                    ));
                }
                let mut seen = std::collections::HashSet::with_capacity(handles.len());
                for handle in &handles {
                    if !seen.insert(*handle) || !resources.encoding.commands.contains_key(handle) {
                        return Err(GpuError::validation(
                            "invalid or already submitted GPUCommandBuffer",
                        ));
                    }
                }
                let mut commands = Vec::with_capacity(handles.len());
                for handle in handles {
                    let command = resources
                        .encoding
                        .commands
                        .remove(&handle)
                        .ok_or_else(|| GpuError::validation("invalid GPUCommandBuffer"))?;
                    commands.push(command);
                }
                self.queue.submit(commands);
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

fn replay(
    resources: &Resources,
    encoder: &mut wgpu::CommandEncoder,
    operation: &Operation,
) -> Result<(), GpuError> {
    match operation {
        Operation::Render {
            label,
            color_attachments,
            depth_stencil_attachment,
            occlusion_query_set,
            commands,
        } => {
            let colors = color_attachments
                .iter()
                .map(|attachment| {
                    attachment
                        .as_ref()
                        .map(|attachment| {
                            Ok(wgpu::RenderPassColorAttachment {
                                view: &get(&resources.views, attachment.view, "GPUTextureView")?.1,
                                resolve_target: attachment
                                    .resolve_target
                                    .map(|handle| {
                                        get(&resources.views, handle, "GPUTextureView")
                                            .map(|view| view.1.as_ref())
                                    })
                                    .transpose()?,
                                depth_slice: attachment.depth_slice,
                                ops: operations(
                                    &attachment.load_op,
                                    &attachment.store_op,
                                    color(attachment.clear_value),
                                )?,
                            })
                        })
                        .transpose()
                })
                .collect::<Result<Vec<_>, GpuError>>()?;
            let depth = depth_stencil_attachment
                .as_ref()
                .map(|attachment| -> Result<_, GpuError> {
                    Ok(wgpu::RenderPassDepthStencilAttachment {
                        view: &get(&resources.views, attachment.view, "GPUTextureView")?.1,
                        depth_ops: attachment_ops(
                            attachment.depth_load_op.as_deref(),
                            attachment.depth_store_op.as_deref(),
                            attachment.depth_clear_value,
                            attachment.depth_read_only,
                        )?,
                        stencil_ops: attachment_ops(
                            attachment.stencil_load_op.as_deref(),
                            attachment.stencil_store_op.as_deref(),
                            attachment.stencil_clear_value,
                            attachment.stencil_read_only,
                        )?,
                    })
                })
                .transpose()?;
            let queries = occlusion_query_set
                .map(|handle| {
                    get(&resources.queries, handle, "GPUQuerySet").map(|(set, _)| set.as_ref())
                })
                .transpose()?;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                color_attachments: &colors,
                depth_stencil_attachment: depth,
                occlusion_query_set: queries,
                ..Default::default()
            });
            for command in commands {
                match command {
                    PassCommand::Pipeline(handle) => {
                        pass.set_pipeline(get(
                            &resources.pipelines.render,
                            *handle,
                            "GPURenderPipeline",
                        )?);
                    }
                    PassCommand::BindGroup {
                        index,
                        group,
                        offsets,
                    } => {
                        let group = group
                            .map(|handle| {
                                get(&resources.pipelines.groups, handle, "GPUBindGroup")
                                    .map(Arc::as_ref)
                            })
                            .transpose()?;
                        pass.set_bind_group(*index, group, offsets);
                    }
                    PassCommand::VertexBuffer {
                        slot,
                        buffer,
                        offset,
                        size,
                    } => {
                        pass.set_vertex_buffer(
                            *slot,
                            buffer_slice(resources, *buffer, *offset, *size)?,
                        );
                    }
                    PassCommand::IndexBuffer {
                        buffer,
                        format,
                        offset,
                        size,
                    } => {
                        pass.set_index_buffer(
                            buffer_slice(resources, *buffer, *offset, *size)?,
                            *format,
                        );
                    }
                    PassCommand::Draw {
                        count,
                        instances,
                        first,
                        first_instance,
                    } => {
                        pass.draw(
                            draw_range(*first, *count)?,
                            draw_range(*first_instance, *instances)?,
                        );
                    }
                    PassCommand::DrawIndexed {
                        count,
                        instances,
                        first,
                        base_vertex,
                        first_instance,
                    } => {
                        pass.draw_indexed(
                            draw_range(*first, *count)?,
                            *base_vertex,
                            draw_range(*first_instance, *instances)?,
                        );
                    }
                    PassCommand::DrawIndirect {
                        buffer,
                        offset,
                        indexed,
                    } => {
                        buffer_range(
                            resources,
                            *buffer,
                            *offset,
                            Some(if *indexed { 20 } else { 16 }),
                        )?;
                        let buffer = &get(&resources.buffers, *buffer, "GPUBuffer")?.buffer;
                        if *indexed {
                            pass.draw_indexed_indirect(buffer, *offset);
                        } else {
                            pass.draw_indirect(buffer, *offset);
                        }
                    }
                    PassCommand::ExecuteBundles(handles) => {
                        let bundles = handles
                            .iter()
                            .map(|handle| {
                                get(&resources.encoding.bundles, *handle, "GPURenderBundle")
                                    .map(|bundle| bundle.native.as_ref())
                            })
                            .collect::<Result<Vec<_>, _>>()?;
                        // wgpu also resets pipeline, bindings and vertex/index
                        // state after executing bundles, including an empty list.
                        pass.execute_bundles(bundles);
                    }
                    PassCommand::BeginOcclusionQuery(index) => pass.begin_occlusion_query(*index),
                    PassCommand::EndOcclusionQuery => pass.end_occlusion_query(),
                    PassCommand::Viewport(value) => {
                        pass.set_viewport(
                            value[0], value[1], value[2], value[3], value[4], value[5],
                        );
                    }
                    PassCommand::Scissor(value) => {
                        pass.set_scissor_rect(value[0], value[1], value[2], value[3]);
                    }
                    PassCommand::BlendConstant(value) => pass.set_blend_constant(color(*value)),
                    PassCommand::StencilReference(value) => pass.set_stencil_reference(*value),
                    PassCommand::Dispatch(_) | PassCommand::DispatchIndirect { .. } => {
                        return Err(GpuError::validation(
                            "dispatchWorkgroups requires a compute pass",
                        ));
                    }
                }
            }
        }
        Operation::Compute { label, commands } => {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(label),
                timestamp_writes: None,
            });
            for command in commands {
                match command {
                    PassCommand::Pipeline(handle) => {
                        pass.set_pipeline(get(
                            &resources.pipelines.compute,
                            *handle,
                            "GPUComputePipeline",
                        )?);
                    }
                    PassCommand::BindGroup {
                        index,
                        group,
                        offsets,
                    } => {
                        let group = group
                            .map(|handle| {
                                get(&resources.pipelines.groups, handle, "GPUBindGroup")
                                    .map(Arc::as_ref)
                            })
                            .transpose()?;
                        pass.set_bind_group(*index, group, offsets);
                    }
                    PassCommand::Dispatch(value) => {
                        pass.dispatch_workgroups(value[0], value[1], value[2])
                    }
                    PassCommand::DispatchIndirect { buffer, offset } => {
                        buffer_range(resources, *buffer, *offset, Some(12))?;
                        pass.dispatch_workgroups_indirect(
                            &get(&resources.buffers, *buffer, "GPUBuffer")?.buffer,
                            *offset,
                        );
                    }
                    _ => {
                        return Err(GpuError::validation(
                            "render command requires a render pass",
                        ));
                    }
                }
            }
        }
        Operation::ResolveQuerySet {
            query,
            first,
            count,
            destination,
            offset,
        } => {
            let (query, query_count) = get(&resources.queries, *query, "GPUQuerySet")?;
            let end = first
                .checked_add(*count)
                .filter(|end| end <= query_count)
                .ok_or_else(|| GpuError::validation("query range is out of bounds"))?;
            buffer_range(
                resources,
                *destination,
                *offset,
                Some(u64::from(*count) * 8),
            )?;
            encoder.resolve_query_set(
                query,
                *first..end,
                &get(&resources.buffers, *destination, "GPUBuffer")?.buffer,
                *offset,
            );
        }
        Operation::ClearBuffer {
            buffer,
            offset,
            size,
        } => {
            buffer_range(resources, *buffer, *offset, *size)?;
            encoder.clear_buffer(
                &get(&resources.buffers, *buffer, "GPUBuffer")?.buffer,
                *offset,
                *size,
            );
        }
        Operation::CopyBufferToBuffer {
            source,
            source_offset,
            destination,
            destination_offset,
            size,
        } => {
            buffer_range(resources, *source, *source_offset, Some(*size))?;
            buffer_range(resources, *destination, *destination_offset, Some(*size))?;
            encoder.copy_buffer_to_buffer(
                &get(&resources.buffers, *source, "GPUBuffer")?.buffer,
                *source_offset,
                &get(&resources.buffers, *destination, "GPUBuffer")?.buffer,
                *destination_offset,
                *size,
            );
        }
        Operation::CopyBufferToTexture {
            source,
            destination,
            size,
        } => {
            encoder.copy_buffer_to_texture(
                buffer_copy(resources, source)?,
                texture_copy(resources, destination)?,
                extent(*size),
            );
        }
        Operation::CopyTextureToBuffer {
            source,
            destination,
            size,
        } => {
            encoder.copy_texture_to_buffer(
                texture_copy(resources, source)?,
                buffer_copy(resources, destination)?,
                extent(*size),
            );
        }
        Operation::CopyTextureToTexture {
            source,
            destination,
            size,
        } => {
            encoder.copy_texture_to_texture(
                texture_copy(resources, source)?,
                texture_copy(resources, destination)?,
                extent(*size),
            );
        }
    }
    Ok(())
}

fn command_budget(count: usize, extra: usize) -> Result<usize, GpuError> {
    count
        .checked_add(extra)
        .filter(|count| *count <= COMMAND_LIMIT)
        .ok_or_else(|| GpuError::validation("WebGPU command limit reached in chuzz"))
}

fn pass_command_cost(resources: &Resources, command: &PassCommand) -> Result<usize, GpuError> {
    let mut count = 1;
    if let PassCommand::ExecuteBundles(handles) = command {
        for handle in handles {
            let bundle = get(&resources.encoding.bundles, *handle, "GPURenderBundle")?;
            // Count each reference even when its bundle has no commands.
            count = command_budget(count, bundle.command_count + 1)?;
        }
    }
    Ok(count)
}

fn replay_bundle<'a>(
    resources: &'a Resources,
    encoder: &mut wgpu::RenderBundleEncoder<'a>,
    commands: &[PassCommand],
) -> Result<(), GpuError> {
    for command in commands {
        match command {
            PassCommand::Pipeline(handle) => {
                encoder.set_pipeline(get(
                    &resources.pipelines.render,
                    *handle,
                    "GPURenderPipeline",
                )?);
            }
            PassCommand::BindGroup {
                index,
                group,
                offsets,
            } => {
                let group = group
                    .map(|handle| {
                        get(&resources.pipelines.groups, handle, "GPUBindGroup").map(Arc::as_ref)
                    })
                    .transpose()?;
                encoder.set_bind_group(*index, group, offsets);
            }
            PassCommand::VertexBuffer {
                slot,
                buffer,
                offset,
                size,
            } => {
                encoder.set_vertex_buffer(*slot, buffer_slice(resources, *buffer, *offset, *size)?);
            }
            PassCommand::IndexBuffer {
                buffer,
                format,
                offset,
                size,
            } => {
                encoder
                    .set_index_buffer(buffer_slice(resources, *buffer, *offset, *size)?, *format);
            }
            PassCommand::Draw {
                count,
                instances,
                first,
                first_instance,
            } => {
                encoder.draw(
                    draw_range(*first, *count)?,
                    draw_range(*first_instance, *instances)?,
                );
            }
            PassCommand::DrawIndexed {
                count,
                instances,
                first,
                base_vertex,
                first_instance,
            } => {
                encoder.draw_indexed(
                    draw_range(*first, *count)?,
                    *base_vertex,
                    draw_range(*first_instance, *instances)?,
                );
            }
            PassCommand::DrawIndirect {
                buffer,
                offset,
                indexed,
            } => {
                buffer_range(
                    resources,
                    *buffer,
                    *offset,
                    Some(if *indexed { 20 } else { 16 }),
                )?;
                let buffer = &get(&resources.buffers, *buffer, "GPUBuffer")?.buffer;
                if *indexed {
                    encoder.draw_indexed_indirect(buffer, *offset);
                } else {
                    encoder.draw_indirect(buffer, *offset);
                }
            }
            _ => {
                return Err(GpuError::validation(
                    "render pass command is not valid in GPURenderBundleEncoder",
                ));
            }
        }
    }
    Ok(())
}
