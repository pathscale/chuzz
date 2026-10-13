use std::collections::BTreeMap;
use std::sync::atomic::AtomicU64;

use super::*;

const SNAPSHOT_BYTES_LIMIT: u64 = 512 * 1024 * 1024;

#[derive(Default)]
pub(super) struct Tracking {
    serial: u64,
    completed: Arc<AtomicU64>,
    bytes: Arc<AtomicU64>,
    shaders: HashMap<u64, Arc<wgpu::naga::Module>>,
    layouts: HashMap<u64, Arc<Vec<u32>>>,
    pipeline_layouts: HashMap<u64, Vec<Option<u64>>>,
    pipeline_groups: HashMap<u64, HashMap<u32, Arc<Vec<u32>>>>,
    pub groups: HashMap<u64, Arc<Vec<u64>>>,
    pub pending_writes: Option<(u64, Vec<u64>)>,
}

fn number(value: &Value, name: &str) -> u64 {
    value.get(name).and_then(Value::as_u64).unwrap_or_default()
}

impl Tracking {
    pub fn observe(&mut self, command: &Command) {
        let args = &command.args;
        match command.op.as_str() {
            "createShaderModule" => {
                if let Some(code) = args.get("code").and_then(Value::as_str)
                    && let Ok(module) = wgpu::naga::front::wgsl::parse_str(code)
                {
                    self.shaders.insert(command.handle, Arc::new(module));
                }
            }
            "createBindGroupLayout" => {
                let bindings = args
                    .get("entries")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|entry| {
                        entry
                            .get("buffer")
                            .and_then(|buffer| buffer.get("type"))
                            .and_then(Value::as_str)
                            == Some("storage")
                    })
                    .map(|entry| number(entry, "binding") as u32)
                    .collect();
                self.layouts.insert(command.handle, Arc::new(bindings));
            }
            "createPipelineLayout" => {
                let layouts = args
                    .get("bindGroupLayouts")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(Value::as_u64)
                    .collect();
                self.pipeline_layouts.insert(command.handle, layouts);
            }
            "createRenderPipeline"
            | "createRenderPipelineAsync"
            | "createComputePipeline"
            | "createComputePipelineAsync" => {
                let mut groups = HashMap::<u32, Arc<Vec<u32>>>::new();
                if let Some(layout) = args.get("layout").and_then(Value::as_u64) {
                    if let Some(layouts) = self.pipeline_layouts.get(&layout) {
                        for (index, layout) in layouts.iter().enumerate() {
                            if let Some(bindings) = layout.and_then(|id| self.layouts.get(&id)) {
                                groups.insert(index as u32, Arc::clone(bindings));
                            }
                        }
                    }
                } else {
                    for (name, kind) in [
                        ("vertex", wgpu::naga::ShaderStage::Vertex),
                        ("fragment", wgpu::naga::ShaderStage::Fragment),
                        ("compute", wgpu::naga::ShaderStage::Compute),
                    ] {
                        let Some(stage) = args.get(name) else {
                            continue;
                        };
                        let Some(module) = self.shaders.get(&number(stage, "module")) else {
                            continue;
                        };
                        let selected = stage.get("entryPoint").and_then(Value::as_str);
                        let mut entries =
                            module.entry_points.iter().enumerate().filter(|(_, entry)| {
                                entry.stage == kind
                                    && selected.is_none_or(|name| entry.name == name)
                            });
                        let Some((index, _)) = entries.next() else {
                            continue;
                        };
                        if entries.next().is_some() {
                            continue;
                        }
                        let mut validator = wgpu::naga::valid::Validator::new(
                            wgpu::naga::valid::ValidationFlags::all(),
                            wgpu::naga::valid::Capabilities::all(),
                        );
                        let Ok(info) = validator.validate(module) else {
                            continue;
                        };
                        let entry = info.get_entry_point(index);
                        for (handle, global) in module.global_variables.iter() {
                            let wgpu::naga::AddressSpace::Storage { access } = global.space else {
                                continue;
                            };
                            if !access.contains(wgpu::naga::StorageAccess::STORE)
                                || entry[handle].is_empty()
                            {
                                continue;
                            }
                            if let Some(binding) = &global.binding {
                                let bindings = groups
                                    .entry(binding.group)
                                    .or_insert_with(|| Arc::new(Vec::new()));
                                if !bindings.contains(&binding.binding) {
                                    Arc::make_mut(bindings).push(binding.binding);
                                }
                            }
                        }
                    }
                }
                self.pipeline_groups.insert(command.handle, groups);
            }
            "getBindGroupLayout" => {
                let bindings = self
                    .pipeline_groups
                    .get(&number(args, "pipeline"))
                    .and_then(|groups| groups.get(&(number(args, "index") as u32)))
                    .cloned()
                    .unwrap_or_else(|| Arc::new(Vec::new()));
                self.layouts.insert(command.handle, bindings);
            }
            "createBindGroup" => {
                let writable = self.layouts.get(&number(args, "layout"));
                let buffers = args
                    .get("entries")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|entry| {
                        writable.is_some_and(|bindings| {
                            bindings.contains(&(number(entry, "binding") as u32))
                        })
                    })
                    .filter_map(|entry| {
                        let resource = entry.get("resource")?;
                        (resource.get("kind").and_then(Value::as_str) == Some("buffer"))
                            .then(|| number(resource, "handle"))
                    })
                    .collect();
                self.groups.insert(command.handle, Arc::new(buffers));
            }
            _ => {}
        }
    }
}

/// A completed copy stays mapped and immutable until its final reader drops it.
pub(super) struct Snapshot {
    pub submit: u64,
    pub written_by_write_buffer: bool,
    pub staging: Option<Arc<wgpu::Buffer>>,
    pub bytes: u64,
    budget: Arc<AtomicU64>,
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        if let Some(staging) = &self.staging {
            staging.unmap();
        }
        self.budget.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

#[derive(Default)]
pub(super) struct History {
    pub newest: AtomicU64,
    pub written_by_write_buffer: AtomicBool,
    pub pending: AtomicU64,
    pub completed: Mutex<Option<Arc<Snapshot>>>,
    pub error: Mutex<Option<String>>,
}

impl Bridge {
    pub(super) fn inspection_active(&self) -> bool {
        self.state().inspection.active()
    }
}

impl DeviceConnection {
    /// All native submits pass here, including queue-write flushes and host copies.
    /// The off path performs only the existing lease check and native submit.
    #[inline]
    pub(super) fn submit_gpu(
        &self,
        commands: impl IntoIterator<Item = wgpu::CommandBuffer>,
    ) -> u64 {
        if !self.inspection_active() {
            self.queue.submit(commands);
            return 0;
        }
        let (serial, completed) = {
            let mut inspection = lock(&self.inspection);
            let tracking = inspection
                .buffer_tracking
                .get_or_insert_with(Tracking::default);
            tracking.serial += 1;
            (tracking.serial, Arc::clone(&tracking.completed))
        };
        self.queue.submit(commands);
        let wake = self.owner.upgrade().map(|owner| lock(&owner).wake.clone());
        self.queue.on_submitted_work_done(move || {
            completed.fetch_max(serial, Ordering::AcqRel);
            if let Some(wake) = wake {
                wake.wake();
            }
        });
        serial
    }

    pub(super) fn checkpoint_buffers(
        &self,
        resources: &Resources,
        handles: &[u64],
        submit: u64,
        written_by_write_buffer: bool,
    ) {
        if !self.inspection_active() {
            return;
        }
        for handle in handles {
            if let Some(entry) = resources.buffers.get(handle)
                && let Some(history) = &entry.inspection
            {
                history
                    .written_by_write_buffer
                    .store(written_by_write_buffer, Ordering::Relaxed);
                history.newest.store(submit, Ordering::Release);
                self.checkpoint_buffer(entry, Arc::clone(history), submit, written_by_write_buffer);
            }
        }
    }

    pub(super) fn checkpoint_buffer(
        &self,
        entry: &buffer::Entry,
        history: Arc<History>,
        submit: u64,
        written_by_write_buffer: bool,
    ) {
        if !self.inspection_active() || entry.mapped.is_some() {
            return;
        }
        let budget = {
            let mut inspection = lock(&self.inspection);
            Arc::clone(
                &inspection
                    .buffer_tracking
                    .get_or_insert_with(Tracking::default)
                    .bytes,
            )
        };
        // WebGPU only permits copies of a multiple of four bytes. An odd tail
        // cannot appear in an aligned request, but size still reports that tail.
        let bytes = entry.size / 4 * 4;
        if budget
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes)
                    .filter(|total| *total <= SNAPSHOT_BYTES_LIMIT)
            })
            .is_err()
        {
            *lock(&history.error) = Some("WebGPU buffer snapshot memory limit reached".to_owned());
            return;
        }
        let wake = self.owner.upgrade().map(|owner| lock(&owner).wake.clone());
        if bytes == 0 {
            *lock(&history.completed) = Some(Arc::new(Snapshot {
                submit,
                written_by_write_buffer,
                staging: None,
                bytes,
                budget,
            }));
            if let Some(wake) = wake {
                wake.wake();
            }
            return;
        }
        let staging = Arc::new(self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chuzz buffer inspection"),
            size: bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        }));
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("chuzz buffer inspection"),
            });
        encoder.copy_buffer_to_buffer(&entry.buffer, 0, &staging, 0, bytes);
        // This is a separate submit ordered after the writer. The copy runs
        // after that write completes on the GPU and before the next page write.
        self.submit_gpu([encoder.finish()]);
        let snapshot = Arc::new(Snapshot {
            submit,
            written_by_write_buffer,
            staging: Some(Arc::clone(&staging)),
            bytes,
            budget,
        });
        history.pending.fetch_add(1, Ordering::AcqRel);
        staging.map_async(wgpu::MapMode::Read, 0..bytes, move |result| {
            match result {
                Ok(()) => {
                    let mut completed = lock(&history.completed);
                    if completed.as_ref().is_none_or(|old| old.submit <= submit) {
                        *completed = Some(snapshot);
                        *lock(&history.error) = None;
                    }
                }
                Err(error) => *lock(&history.error) = Some(error.to_string()),
            }
            history.pending.fetch_sub(1, Ordering::AcqRel);
            // Device servicing remains in the existing host readback path.
            // Neither the page thread nor the ordered command worker waits.
            if let Some(wake) = wake {
                wake.wake();
            }
        });
        // Ask the host's existing device servicing hook to run once.
        if let Some(owner) = self.owner.upgrade() {
            let wake = lock(&owner).wake.clone();
            wake.wake();
        }
    }
}

pub(super) fn extend_unique(destination: &mut Vec<u64>, source: impl IntoIterator<Item = u64>) {
    for handle in source {
        if !destination.contains(&handle) {
            destination.push(handle);
        }
    }
}

pub(super) type BoundGroups = BTreeMap<u32, u64>;
