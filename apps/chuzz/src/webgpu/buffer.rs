use std::ops::Range;

use super::resources::{ALLOCATION_LIMIT, checked, decode};
use super::*;

pub(super) struct Entry {
    pub buffer: Arc<wgpu::Buffer>,
    pub size: u64,
    pub usage: wgpu::BufferUsages,
    pub mapped: Option<Range<u64>>,
    pub writable: bool,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Options {
    label: String,
    size: u64,
    usage: u32,
    mapped_at_creation: bool,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Write {
    offset: u64,
    data: Vec<u8>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Mapping {
    offset: u64,
    size: u64,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Unmap {
    ranges: Vec<Write>,
}

fn range(offset: u64, size: u64, limit: u64) -> Result<Range<u64>, GpuError> {
    let end = offset
        .checked_add(size)
        .filter(|end| *end <= limit)
        .ok_or_else(|| GpuError::validation("GPUBuffer range is out of bounds"))?;
    Ok(offset..end)
}

struct MappingLease {
    buffer: Arc<wgpu::Buffer>,
    active: bool,
}

impl Drop for MappingLease {
    fn drop(&mut self) {
        if self.active {
            self.buffer.unmap();
        }
    }
}

/// Copy only at the IPC boundary. Cancellation, timeout and loss unmap the
/// native allocation through the lease, including cancellation by the outer
/// request deadline.
pub(super) async fn map_read(
    device: &wgpu::Device,
    buffer: Arc<wgpu::Buffer>,
    range: Range<u64>,
) -> Result<Vec<u8>, GpuError> {
    let mut lease = MappingLease {
        buffer: Arc::clone(&buffer),
        active: true,
    };
    let result = Arc::new(Mutex::new(None));
    let callback = Arc::clone(&result);
    checked(device, || {
        buffer.map_async(wgpu::MapMode::Read, range.clone(), move |value| {
            *lock(&callback) = Some(value.map_err(|error| error.to_string()));
        });
    })
    .await?;
    let completed = nagoya::timeout(DEADLINE, async {
        loop {
            device
                .poll(wgpu::PollType::Poll)
                .map_err(|error| GpuError::internal(format!("WebGPU poll failed: {error}")))?;
            let completed = lock(&result).take();
            if let Some(completed) = completed {
                return completed.map_err(GpuError::validation);
            }
            nagoya::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .map_err(|_| GpuError::internal("GPUBuffer.mapAsync timed out in chuzz"))?;
    completed?;
    let bytes = buffer.get_mapped_range(range).to_vec();
    lease.active = false;
    Ok(bytes)
}

impl DeviceConnection {
    pub(super) async fn create_buffer(&self, handle: u64, args: Value) -> Result<Value, GpuError> {
        let options: Options = decode(args)?;
        if options.usage == 0 || options.usage & !1023 != 0 {
            return Err(GpuError::validation("invalid GPUBufferUsage"));
        }
        let usage = wgpu::BufferUsages::from_bits(options.usage)
            .ok_or_else(|| GpuError::validation("invalid GPUBufferUsage"))?;
        if options.mapped_at_creation && options.size % 4 != 0 {
            return Err(GpuError::validation(
                "mappedAtCreation buffer size must be a multiple of 4",
            ));
        }
        self.resources().reserve(handle, options.size)?;
        let buffer = checked(&self.device, || {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(&options.label),
                size: options.size,
                usage,
                mapped_at_creation: options.mapped_at_creation,
            })
        })
        .await?;
        self.resources().buffers.insert(
            handle,
            Entry {
                buffer: Arc::new(buffer),
                size: options.size,
                usage,
                mapped: options.mapped_at_creation.then_some(0..options.size),
                writable: options.mapped_at_creation,
            },
        );
        Ok(Value::Null)
    }

    pub(super) async fn write_buffer(&self, handle: u64, args: Value) -> Result<Value, GpuError> {
        let write: Write = decode(args)?;
        let buffer = {
            let resources = self.resources();
            let entry = resources
                .buffers
                .get(&handle)
                .ok_or_else(|| GpuError::validation("invalid or destroyed GPUBuffer"))?;
            if entry.mapped.is_some() || !entry.usage.contains(wgpu::BufferUsages::COPY_DST) {
                return Err(GpuError::validation(
                    "GPUQueue.writeBuffer requires an unmapped COPY_DST buffer",
                ));
            }
            range(write.offset, write.data.len() as u64, entry.size)?;
            if write.offset % 4 != 0 || write.data.len() % 4 != 0 {
                return Err(GpuError::validation(
                    "GPUQueue.writeBuffer offset and size must be multiples of 4",
                ));
            }
            if write.data.len() as u64 > ALLOCATION_LIMIT {
                return Err(GpuError::validation(
                    "WebGPU transfer limit reached in chuzz",
                ));
            }
            Arc::clone(&entry.buffer)
        };
        checked(&self.device, || {
            if !write.data.is_empty() {
                self.queue.write_buffer(&buffer, write.offset, &write.data);
                // wgpu stages queue writes until submit. Flush them before a
                // following mapAsync, even though public encoding is pending.
                self.queue.submit([]);
            }
        })
        .await?;
        Ok(Value::Null)
    }

    pub(super) async fn map_buffer(&self, handle: u64, args: Value) -> Result<Value, GpuError> {
        let mapping: Mapping = decode(args)?;
        let (buffer, mapped) = {
            let resources = self.resources();
            let entry = resources
                .buffers
                .get(&handle)
                .ok_or_else(|| GpuError::validation("invalid or destroyed GPUBuffer"))?;
            if entry.mapped.is_some() || !entry.usage.contains(wgpu::BufferUsages::MAP_READ) {
                return Err(GpuError::validation(
                    "GPUBuffer.mapAsync READ requires an unmapped MAP_READ buffer",
                ));
            }
            let mapped = range(mapping.offset, mapping.size, entry.size)?;
            // wgpu's range accessors panic for empty or misaligned ranges.
            if mapping.offset % 8 != 0 || mapping.size == 0 || mapping.size % 4 != 0 {
                return Err(GpuError::validation(
                    "GPUBuffer.mapAsync requires an 8-byte offset and a nonempty 4-byte size",
                ));
            }
            (Arc::clone(&entry.buffer), mapped)
        };
        let bytes = map_read(&self.device, buffer, mapped.clone()).await?;
        if let Some(entry) = self.resources().buffers.get_mut(&handle) {
            entry.mapped = Some(mapped);
            entry.writable = false;
        }
        Ok(json!(bytes))
    }

    pub(super) async fn unmap_buffer(&self, handle: u64, args: Value) -> Result<Value, GpuError> {
        let unmap: Unmap = decode(args)?;
        let (buffer, writable, mapped) = {
            let resources = self.resources();
            let entry = resources
                .buffers
                .get(&handle)
                .ok_or_else(|| GpuError::validation("invalid or destroyed GPUBuffer"))?;
            (
                Arc::clone(&entry.buffer),
                entry.writable,
                entry.mapped.clone(),
            )
        };
        if writable {
            let mapped = mapped
                .as_ref()
                .ok_or_else(|| GpuError::validation("GPUBuffer is not mapped"))?;
            let mut ranges = Vec::new();
            let mut transferred = 0_u64;
            for write in &unmap.ranges {
                let current = range(write.offset, write.data.len() as u64, mapped.end)?;
                if current.start < mapped.start
                    || current.start % 8 != 0
                    || write.data.len() % 4 != 0
                    || ranges.iter().any(|previous: &Range<u64>| {
                        current.start < previous.end && previous.start < current.end
                    })
                {
                    return Err(GpuError::validation("invalid GPUBuffer mapped range"));
                }
                transferred = transferred.saturating_add(write.data.len() as u64);
                if transferred > ALLOCATION_LIMIT {
                    return Err(GpuError::validation(
                        "WebGPU transfer limit reached in chuzz",
                    ));
                }
                ranges.push(current);
            }
        } else if !unmap.ranges.is_empty() {
            return Err(GpuError::validation("cannot write a READ mapping"));
        }

        checked(&self.device, || {
            if writable {
                for write in unmap.ranges {
                    if !write.data.is_empty() {
                        buffer
                            .get_mapped_range_mut(
                                write.offset..write.offset + write.data.len() as u64,
                            )
                            .copy_from_slice(&write.data);
                    }
                }
            }
            buffer.unmap();
        })
        .await?;
        if let Some(entry) = self.resources().buffers.get_mut(&handle) {
            entry.mapped = None;
            entry.writable = false;
        }
        Ok(Value::Null)
    }
}
