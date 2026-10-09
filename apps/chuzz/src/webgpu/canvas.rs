use blitz_dom::Document as _;
use blitz_dom::node::{ImageData, RasterImageData, SpecialElementData};

use super::buffer::map_read;
use super::resources::{ALLOCATION_LIMIT, checked, decode};
use super::*;

#[derive(Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Configuration {
    pub generation: u64,
    pub format: String,
    pub usage: u32,
    pub alpha_mode: String,
    pub view_formats: Vec<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Current {
    canvas: u64,
    generation: u64,
    width: u32,
    height: u32,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Present {
    canvas: u64,
    generation: u64,
}

pub(super) struct Frame {
    pub canvas: u64,
    pub generation: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Option<Arc<Vec<u8>>>,
}

impl Frame {
    pub fn paint(self, document: &mut blitz_script::ScriptDocument) -> bool {
        // A configure, unconfigure or resize may have overtaken a readback.
        // Do not let its old pixels replace the current canvas generation.
        let live = document
            .eval_json(&format!(
                "globalThis.__chuzzGpuCanvasLive({}, {}, {}, {})",
                self.canvas, self.generation, self.width, self.height
            ))
            .ok()
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        if !live {
            return false;
        }
        let selector = format!("canvas[data-chuzz-webgpu='{}']", self.canvas);
        let Some(id) = document.inner().query_selector(&selector).ok().flatten() else {
            return false;
        };
        // The guard must outlive the node borrow taken from it.
        let mut doc = document.inner_mut();
        let Some(node) = doc.get_node_mut(id) else {
            return false;
        };
        let Some(element) = node.element_data_mut() else {
            return false;
        };
        element.special_data = match self.rgba {
            Some(rgba) => SpecialElementData::Image(Box::new(ImageData::Raster(
                RasterImageData::new(self.width, self.height, rgba),
            ))),
            None => SpecialElementData::None,
        };
        node.cache_mut().clear();
        if let Some(damage) = node.damage() {
            node.insert_damage(damage.union(damage.complement()));
        }
        node.set_dirty_descendants();
        node.mark_ancestors_dirty();
        true
    }
}

impl DeviceConnection {
    fn publish_frame(&self, frame: Frame) {
        if let Some(owner) = self.owner.upgrade() {
            // Keep at most the latest completed frame for each canvas.
            lock(&owner).frames.insert(frame.canvas, frame);
        }
    }

    pub(super) fn configure_canvas(&self, canvas: u64, args: Value) -> Result<Value, GpuError> {
        let configuration: Configuration = decode(args)?;
        if !matches!(configuration.format.as_str(), "rgba8unorm" | "bgra8unorm") {
            return Err(GpuError::validation(format!(
                "not implemented in chuzz: GPUCanvasConfiguration.format.{}",
                configuration.format
            )));
        }
        if !matches!(
            configuration.alpha_mode.as_str(),
            "opaque" | "premultiplied"
        ) {
            return Err(GpuError::validation("invalid GPUCanvasAlphaMode"));
        }
        if configuration.usage == 0 || configuration.usage & !31 != 0 {
            return Err(GpuError::validation("invalid GPUTextureUsage"));
        }
        for view in &configuration.view_formats {
            if view != &configuration.format && view != &format!("{}-srgb", configuration.format) {
                return Err(GpuError::validation("invalid canvas viewFormats"));
            }
        }
        {
            let mut resources = self.resources();
            if !resources.canvases.contains_key(&canvas) && resources.canvases.len() >= 64 {
                return Err(GpuError::validation("WebGPU canvas limit reached in chuzz"));
            }
            resources.release_canvas(canvas);
            resources.canvases.insert(canvas, configuration.clone());
        }
        self.publish_frame(Frame {
            canvas,
            generation: configuration.generation,
            width: 0,
            height: 0,
            rgba: None,
        });
        Ok(Value::Null)
    }

    pub(super) fn unconfigure_canvas(&self, canvas: u64, args: Value) -> Result<Value, GpuError> {
        let generation: u64 = decode(args)?;
        self.resources().release_canvas(canvas);
        self.publish_frame(Frame {
            canvas,
            generation,
            width: 0,
            height: 0,
            rgba: None,
        });
        Ok(Value::Null)
    }

    pub(super) async fn current_texture(
        &self,
        handle: u64,
        args: Value,
    ) -> Result<Value, GpuError> {
        let current: Current = decode(args)?;
        let configuration = self
            .resources()
            .canvases
            .get(&current.canvas)
            .cloned()
            .filter(|configuration| configuration.generation == current.generation)
            .ok_or_else(|| GpuError::validation("GPUCanvasContext is not configured"))?;
        self.create_texture_options(
            handle,
            texture::Options {
                label: "chuzz canvas current texture".to_owned(),
                size: [current.width, current.height, 1],
                mip_level_count: 1,
                sample_count: 1,
                dimension: "2d".to_owned(),
                format: configuration.format,
                // COPY_SRC is private implementation usage for CPU capture.
                usage: configuration.usage | wgpu::TextureUsages::COPY_SRC.bits(),
                view_formats: configuration.view_formats,
            },
            Some((current.canvas, current.generation)),
        )
        .await
    }

    pub(super) async fn present_canvas(&self, handle: u64, args: Value) -> Result<Value, GpuError> {
        let present: Present = decode(args)?;
        let (entry, configuration) = {
            let mut resources = self.resources();
            let configuration = resources
                .canvases
                .get(&present.canvas)
                .cloned()
                .filter(|configuration| configuration.generation == present.generation)
                .ok_or_else(|| GpuError::validation("GPUCanvasContext is not configured"))?;
            let entry = resources
                .remove_texture(handle)
                .filter(|entry| entry.canvas == Some((present.canvas, present.generation)))
                .ok_or_else(|| GpuError::validation("invalid canvas current texture"))?;
            (entry, configuration)
        };
        let texture = entry.texture;
        let width = texture.width();
        let height = texture.height();
        let row = width
            .checked_mul(4)
            .ok_or_else(|| GpuError::validation("canvas row overflow"))?;
        let padded =
            row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let bytes = u64::from(padded) * u64::from(height);
        if bytes == 0 || bytes > ALLOCATION_LIMIT {
            return Err(GpuError::validation(
                "WebGPU canvas readback limit reached in chuzz",
            ));
        }

        let buffer = checked(&self.device, || {
            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("chuzz canvas readback"),
                size: bytes,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("chuzz canvas readback"),
                });
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(padded),
                        rows_per_image: Some(height),
                    },
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
            self.queue.submit([encoder.finish()]);
            Arc::new(buffer)
        })
        .await?;
        let readback = map_read(&self.device, Arc::clone(&buffer), 0..bytes).await?;
        buffer.unmap();
        texture.destroy();

        let mut rgba = Vec::with_capacity(row as usize * height as usize);
        let bgra = configuration.format == "bgra8unorm";
        for source in readback.chunks_exact(padded as usize) {
            for pixel in source[..row as usize].chunks_exact(4) {
                let mut pixel = [pixel[0], pixel[1], pixel[2], pixel[3]];
                if bgra {
                    pixel.swap(0, 2);
                }
                if configuration.alpha_mode == "opaque" {
                    pixel[3] = 255;
                } else {
                    // Blitz raster images contain straight RGBA. WebGPU's
                    // premultiplied canvas storage must be converted once.
                    let alpha = u32::from(pixel[3]);
                    if alpha == 0 {
                        pixel[..3].fill(0);
                    } else if alpha < 255 {
                        for channel in &mut pixel[..3] {
                            *channel =
                                ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
                        }
                    }
                }
                rgba.extend_from_slice(&pixel);
            }
        }
        self.publish_frame(Frame {
            canvas: present.canvas,
            generation: present.generation,
            width,
            height,
            rgba: Some(Arc::new(rgba)),
        });
        Ok(Value::Null)
    }
}
