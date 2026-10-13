use super::resources::{ALLOCATION_LIMIT, checked, decode};
use super::*;

pub(super) struct Entry {
    pub texture: Arc<wgpu::Texture>,
    pub bytes: u64,
    pub canvas: Option<(u64, u64)>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Options {
    pub label: String,
    pub size: [u32; 3],
    pub mip_level_count: u32,
    pub sample_count: u32,
    pub dimension: String,
    pub format: String,
    pub usage: u32,
    pub view_formats: Vec<String>,
}

#[derive(serde::Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct ViewOptions {
    label: String,
    format: Option<String>,
    dimension: Option<String>,
    aspect: String,
    base_mip_level: u32,
    mip_level_count: Option<u32>,
    base_array_layer: u32,
    array_layer_count: Option<u32>,
}

impl Default for ViewOptions {
    fn default() -> Self {
        Self {
            label: String::new(),
            format: None,
            dimension: None,
            aspect: "all".to_owned(),
            base_mip_level: 0,
            mip_level_count: None,
            base_array_layer: 0,
            array_layer_count: None,
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct View {
    texture: u64,
    descriptor: ViewOptions,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Write {
    texture: u64,
    mip_level: u32,
    origin: [u32; 3],
    aspect: String,
    data: u64,
    offset: u64,
    bytes_per_row: Option<u32>,
    rows_per_image: Option<u32>,
    size: [u32; 3],
}

#[derive(serde::Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct SamplerOptions {
    label: String,
    address_mode_u: String,
    address_mode_v: String,
    address_mode_w: String,
    mag_filter: String,
    min_filter: String,
    mipmap_filter: String,
    lod_min_clamp: f32,
    lod_max_clamp: f32,
    compare: Option<String>,
    max_anisotropy: u16,
}

impl Default for SamplerOptions {
    fn default() -> Self {
        Self {
            label: String::new(),
            address_mode_u: "clamp-to-edge".to_owned(),
            address_mode_v: "clamp-to-edge".to_owned(),
            address_mode_w: "clamp-to-edge".to_owned(),
            mag_filter: "nearest".to_owned(),
            min_filter: "min_filter".replace("min_filter", "nearest"),
            mipmap_filter: "nearest".to_owned(),
            lod_min_clamp: 0.0,
            lod_max_clamp: 32.0,
            compare: None,
            max_anisotropy: 1,
        }
    }
}

pub(super) fn format(name: &str) -> Result<wgpu::TextureFormat, GpuError> {
    // Reject native-only texture families even if wgpu can deserialize them.
    if matches!(
        name,
        "r64uint"
            | "nv12"
            | "p010"
            | "r16unorm"
            | "r16snorm"
            | "rg16unorm"
            | "rg16snorm"
            | "rgba16unorm"
            | "rgba16snorm"
    ) || name.contains("astc") && !name.ends_with("-unorm") && !name.ends_with("-unorm-srgb")
    {
        return Err(GpuError::validation(format!(
            "not implemented in chuzz: GPUTextureFormat.{name}"
        )));
    }
    decode(Value::String(name.to_owned()))
}

fn dimension(name: &str) -> Result<wgpu::TextureDimension, GpuError> {
    match name {
        "1d" => Ok(wgpu::TextureDimension::D1),
        "2d" => Ok(wgpu::TextureDimension::D2),
        "3d" => Ok(wgpu::TextureDimension::D3),
        _ => Err(GpuError::validation("invalid GPUTextureDimension")),
    }
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

fn aspect(name: &str) -> Result<wgpu::TextureAspect, GpuError> {
    match name {
        "all" => Ok(wgpu::TextureAspect::All),
        "depth-only" => Ok(wgpu::TextureAspect::DepthOnly),
        "stencil-only" => Ok(wgpu::TextureAspect::StencilOnly),
        _ => Err(GpuError::validation("invalid GPUTextureAspect")),
    }
}

fn address(name: &str) -> Result<wgpu::AddressMode, GpuError> {
    match name {
        "clamp-to-edge" => Ok(wgpu::AddressMode::ClampToEdge),
        "repeat" => Ok(wgpu::AddressMode::Repeat),
        "mirror-repeat" => Ok(wgpu::AddressMode::MirrorRepeat),
        _ => Err(GpuError::validation("invalid GPUAddressMode")),
    }
}

fn filter(name: &str) -> Result<wgpu::FilterMode, GpuError> {
    match name {
        "nearest" => Ok(wgpu::FilterMode::Nearest),
        "linear" => Ok(wgpu::FilterMode::Linear),
        _ => Err(GpuError::validation("invalid GPUFilterMode")),
    }
}

fn mipmap_filter(name: &str) -> Result<wgpu::MipmapFilterMode, GpuError> {
    match name {
        "nearest" => Ok(wgpu::MipmapFilterMode::Nearest),
        "linear" => Ok(wgpu::MipmapFilterMode::Linear),
        _ => Err(GpuError::validation("invalid GPUMipmapFilterMode")),
    }
}

fn compare(name: &str) -> Result<wgpu::CompareFunction, GpuError> {
    match name {
        "never" => Ok(wgpu::CompareFunction::Never),
        "less" => Ok(wgpu::CompareFunction::Less),
        "equal" => Ok(wgpu::CompareFunction::Equal),
        "less-equal" => Ok(wgpu::CompareFunction::LessEqual),
        "greater" => Ok(wgpu::CompareFunction::Greater),
        "not-equal" => Ok(wgpu::CompareFunction::NotEqual),
        "greater-equal" => Ok(wgpu::CompareFunction::GreaterEqual),
        "always" => Ok(wgpu::CompareFunction::Always),
        _ => Err(GpuError::validation("invalid GPUCompareFunction")),
    }
}

fn allocation(options: &Options, format: wgpu::TextureFormat) -> Result<u64, GpuError> {
    if options.size.contains(&0)
        || options.mip_level_count == 0
        || options.mip_level_count > 32
        || !matches!(options.sample_count, 1 | 4)
    {
        return Err(GpuError::validation(
            "invalid GPUTexture size, mips or samples",
        ));
    }
    let (block_width, block_height) = format.block_dimensions();
    // Copy footprints are undefined for depth24plus and combined depth/stencil.
    // Use wgpu's memory estimate for one block, then retain checked arithmetic
    // for dimensions, layers, samples and mips. This is a budget estimate, not
    // a measurement of the driver's allocation.
    let block_bytes = format.theoretical_memory_footprint(wgpu::Extent3d {
        width: block_width,
        height: block_height,
        depth_or_array_layers: 1,
    });
    let mut total = 0_u64;
    for level in 0..options.mip_level_count {
        let width = (options.size[0] >> level).max(1).div_ceil(block_width);
        let height = (options.size[1] >> level).max(1).div_ceil(block_height);
        let depth = if options.dimension == "3d" {
            (options.size[2] >> level).max(1)
        } else {
            options.size[2]
        };
        let bytes = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|bytes| bytes.checked_mul(u64::from(depth)))
            .and_then(|bytes| bytes.checked_mul(block_bytes))
            .and_then(|bytes| bytes.checked_mul(u64::from(options.sample_count)))
            .ok_or_else(|| GpuError::validation("GPUTexture allocation overflow"))?;
        total = total
            .checked_add(bytes)
            .ok_or_else(|| GpuError::validation("GPUTexture allocation overflow"))?;
    }
    Ok(total)
}

impl DeviceConnection {
    pub(super) async fn create_texture(
        &self,
        handle: u64,
        args: Value,
        canvas: Option<(u64, u64)>,
    ) -> Result<Value, GpuError> {
        self.create_texture_options(handle, decode(args)?, canvas)
            .await
    }

    pub(super) async fn create_texture_options(
        &self,
        handle: u64,
        options: Options,
        canvas: Option<(u64, u64)>,
    ) -> Result<Value, GpuError> {
        let format = format(&options.format)?;
        let dimension = dimension(&options.dimension)?;
        if options.usage == 0 || options.usage & !31 != 0 {
            return Err(GpuError::validation("invalid GPUTextureUsage"));
        }
        let usage = wgpu::TextureUsages::from_bits(options.usage)
            .ok_or_else(|| GpuError::validation("invalid GPUTextureUsage"))?;
        let view_formats = options
            .view_formats
            .iter()
            .map(|name| self::format(name))
            .collect::<Result<Vec<_>, _>>()?;
        let bytes = allocation(&options, format)?;
        self.resources().reserve(handle, bytes)?;
        let texture = checked(&self.device, || {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(&options.label),
                size: wgpu::Extent3d {
                    width: options.size[0],
                    height: options.size[1],
                    depth_or_array_layers: options.size[2],
                },
                mip_level_count: options.mip_level_count,
                sample_count: options.sample_count,
                dimension,
                format,
                usage,
                view_formats: &view_formats,
            })
        })
        .await?;
        self.resources().textures.insert(
            handle,
            Entry {
                texture: Arc::new(texture),
                bytes,
                canvas,
            },
        );
        Ok(Value::Null)
    }

    pub(super) async fn create_view(&self, handle: u64, args: Value) -> Result<Value, GpuError> {
        let view: View = decode(args)?;
        let texture = {
            let resources = self.resources();
            resources.reserve(handle, 0)?;
            Arc::clone(
                &resources
                    .textures
                    .get(&view.texture)
                    .ok_or_else(|| GpuError::validation("invalid or destroyed GPUTexture"))?
                    .texture,
            )
        };
        let options = view.descriptor;
        let descriptor = wgpu::TextureViewDescriptor {
            label: Some(&options.label),
            format: options.format.as_deref().map(format).transpose()?,
            dimension: options
                .dimension
                .as_deref()
                .map(view_dimension)
                .transpose()?,
            aspect: aspect(&options.aspect)?,
            base_mip_level: options.base_mip_level,
            mip_level_count: options.mip_level_count,
            base_array_layer: options.base_array_layer,
            array_layer_count: options.array_layer_count,
            ..Default::default()
        };
        let native = checked(&self.device, || texture.create_view(&descriptor)).await?;
        self.resources()
            .views
            .insert(handle, (view.texture, Arc::new(native)));
        Ok(Value::Null)
    }

    pub(super) async fn write_texture(&self, args: Value) -> Result<Value, GpuError> {
        let write: Write = decode(args)?;
        let data = self.input_bytes(write.data)?;
        if data.len() as u64 > ALLOCATION_LIMIT {
            return Err(GpuError::validation(
                "WebGPU transfer limit reached in chuzz",
            ));
        }
        let texture = {
            let resources = self.resources();
            Arc::clone(
                &resources
                    .textures
                    .get(&write.texture)
                    .ok_or_else(|| GpuError::validation("invalid or destroyed GPUTexture"))?
                    .texture,
            )
        };
        if !texture.usage().contains(wgpu::TextureUsages::COPY_DST) || texture.sample_count() != 1 {
            return Err(GpuError::validation(
                "GPUQueue.writeTexture requires a single-sample COPY_DST texture",
            ));
        }
        let aspect = aspect(&write.aspect)?;
        checked(&self.device, || {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: write.mip_level,
                    origin: wgpu::Origin3d {
                        x: write.origin[0],
                        y: write.origin[1],
                        z: write.origin[2],
                    },
                    aspect,
                },
                &data,
                wgpu::TexelCopyBufferLayout {
                    offset: write.offset,
                    bytes_per_row: write.bytes_per_row,
                    rows_per_image: write.rows_per_image,
                },
                wgpu::Extent3d {
                    width: write.size[0],
                    height: write.size[1],
                    depth_or_array_layers: write.size[2],
                },
            );
            self.submit_gpu([]);
        })
        .await?;
        Ok(Value::Null)
    }

    pub(super) async fn create_sampler(&self, handle: u64, args: Value) -> Result<Value, GpuError> {
        let options: SamplerOptions = decode(args)?;
        self.resources().reserve(handle, 0)?;
        let descriptor = wgpu::SamplerDescriptor {
            label: Some(&options.label),
            address_mode_u: address(&options.address_mode_u)?,
            address_mode_v: address(&options.address_mode_v)?,
            address_mode_w: address(&options.address_mode_w)?,
            mag_filter: filter(&options.mag_filter)?,
            min_filter: filter(&options.min_filter)?,
            mipmap_filter: mipmap_filter(&options.mipmap_filter)?,
            lod_min_clamp: options.lod_min_clamp,
            lod_max_clamp: options.lod_max_clamp,
            compare: options.compare.as_deref().map(compare).transpose()?,
            anisotropy_clamp: options.max_anisotropy,
            ..Default::default()
        };
        let sampler = checked(&self.device, || self.device.create_sampler(&descriptor)).await?;
        self.resources().samplers.insert(handle, Arc::new(sampler));
        Ok(Value::Null)
    }
}
