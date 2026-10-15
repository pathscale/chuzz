//! Native byte snapshots. JSON contains handles and descriptors only.

use blitz_dom::node::RasterImageData;

use super::resources::{ALLOCATION_LIMIT, RESOURCE_LIMIT};
use super::*;

#[path = "binary_script.rs"]
mod script;

const TABLE_BYTES_LIMIT: u64 = 512 * 1024 * 1024;
pub(super) const MAX_ID: u64 = 9_007_199_254_740_991;

pub(super) enum Block {
    Buffer(Arc<Vec<u8>>),
    Image(Arc<RasterImageData>),
}

impl Block {
    fn data(&self) -> &[u8] {
        match self {
            Self::Buffer(bytes) => bytes.as_slice(),
            Self::Image(image) => image.data.data(),
        }
    }
}

struct Entry {
    block: Block,
    input: bool,
    claimed: bool,
}

#[derive(Default)]
struct Entries {
    next_id: u64,
    allocated: u64,
    closed: bool,
    values: HashMap<u64, Entry>,
    active: HashMap<u64, u64>,
}

#[derive(Default)]
pub(super) struct Table(Mutex<Entries>);

impl Table {
    fn reserve(entries: &mut Entries, size: usize) -> Result<u64, GpuError> {
        if entries.closed {
            return Err(GpuError::internal("byte document was destroyed"));
        }
        if size as u64 > ALLOCATION_LIMIT
            || entries.allocated.saturating_add(size as u64) > TABLE_BYTES_LIMIT
            || entries.values.len() + entries.active.len() >= RESOURCE_LIMIT
            || entries.next_id >= MAX_ID
        {
            return Err(GpuError::out_of_memory(
                "WebGPU transfer limit reached in chuzz",
            ));
        }
        entries.next_id += 1;
        entries.allocated += size as u64;
        Ok(entries.next_id)
    }

    pub(super) fn upload(&self, bytes: &[u8]) -> Result<u64, GpuError> {
        let mut entries = lock(&self.0);
        let id = Self::reserve(&mut entries, bytes.len())?;
        entries.values.insert(
            id,
            Entry {
                block: Block::Buffer(Arc::new(bytes.to_vec())),
                input: true,
                claimed: false,
            },
        );
        Ok(id)
    }

    pub(super) fn insert(&self, block: Block, input: bool) -> Result<u64, GpuError> {
        let mut entries = lock(&self.0);
        let id = Self::reserve(&mut entries, block.data().len())?;
        entries.values.insert(
            id,
            Entry {
                block,
                input,
                claimed: false,
            },
        );
        Ok(id)
    }

    fn remove(entries: &mut Entries, id: u64) -> Option<Entry> {
        let entry = entries.values.remove(&id)?;
        entries.allocated -= entry.block.data().len() as u64;
        Some(entry)
    }

    pub(super) fn discard(&self, id: u64) {
        let mut entries = lock(&self.0);
        Self::remove(&mut entries, id);
        if let Some(size) = entries.active.remove(&id) {
            entries.allocated -= size;
        }
    }

    pub(super) fn release_staged(&self, id: u64) {
        let mut entries = lock(&self.0);
        if entries.values.get(&id).is_some_and(|entry| !entry.claimed) {
            Self::remove(&mut entries, id);
        }
    }

    pub(super) fn claim(self: &Arc<Self>, command: &Command) -> Result<Lease, GpuError> {
        let mut ids = Vec::new();
        let mut collect = |value: Option<&Value>| {
            if let Some(id) = value.and_then(Value::as_u64) {
                ids.push(id);
            }
        };
        match command.op.as_str() {
            "writeBuffer" | "writeTexture" => collect(command.args.get("data")),
            "unmap" => {
                if let Some(ranges) = command.args.get("ranges").and_then(Value::as_array) {
                    for range in ranges {
                        collect(range.get("data"));
                    }
                }
            }
            "createImageBitmap" | "copyExternalImageToTexture" | "paintImage" => {
                if let Some(source) = command.args.get("source")
                    && matches!(
                        source.get("kind").and_then(Value::as_str),
                        Some("pixels" | "encoded")
                    )
                {
                    collect(source.get("data"));
                }
            }
            _ => {}
        }
        let mut entries = lock(&self.0);
        let mut unique = std::collections::HashSet::new();
        let valid = ids.iter().all(|id| {
            unique.insert(*id)
                && entries
                    .values
                    .get(id)
                    .is_some_and(|entry| entry.input && !entry.claimed)
        });
        if !valid {
            for id in &ids {
                if entries
                    .values
                    .get(id)
                    .is_some_and(|entry| entry.input && !entry.claimed)
                {
                    Self::remove(&mut entries, *id);
                }
            }
            return Err(GpuError::validation("invalid or reused WebGPU byte handle"));
        }
        for id in &ids {
            if let Some(entry) = entries.values.get_mut(id) {
                entry.claimed = true;
            }
        }
        Ok(Lease {
            table: Arc::downgrade(self),
            ids,
        })
    }

    pub(super) fn take_input(&self, id: u64) -> Result<Arc<Vec<u8>>, GpuError> {
        let mut entries = lock(&self.0);
        if !entries
            .values
            .get(&id)
            .is_some_and(|entry| entry.input && entry.claimed)
        {
            return Err(GpuError::validation(
                "invalid or consumed WebGPU byte handle",
            ));
        }
        let entry = entries
            .values
            .remove(&id)
            .ok_or_else(|| GpuError::validation("invalid WebGPU upload"))?;
        let size = entry.block.data().len() as u64;
        let Block::Buffer(bytes) = entry.block else {
            entries.allocated -= size;
            return Err(GpuError::validation("invalid WebGPU upload"));
        };
        // Move the Arc out of the table. Retain only its budget charge until
        // the command lease drops, including driver waits and cancellation.
        entries.active.insert(id, size);
        Ok(bytes)
    }

    pub(super) fn take_output(&self, id: u64) -> Result<Block, GpuError> {
        let mut entries = lock(&self.0);
        if !entries.values.get(&id).is_some_and(|entry| !entry.input) {
            return Err(GpuError::validation("invalid or consumed WebGPU readback"));
        }
        Self::remove(&mut entries, id)
            .map(|entry| entry.block)
            .ok_or_else(|| GpuError::validation("invalid WebGPU readback"))
    }

    fn close(&self) {
        let mut entries = lock(&self.0);
        entries.closed = true;
        entries.values.clear();
        entries.active.clear();
        entries.allocated = 0;
    }
}

#[derive(Default)]
pub(super) struct Lease {
    table: Weak<Table>,
    ids: Vec<u64>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(table) = self.table.upgrade() {
            for id in &self.ids {
                table.discard(*id);
            }
        }
    }
}

impl Bridge {
    pub(super) fn byte_table(&self) -> Arc<Table> {
        Arc::clone(&self.state().bytes)
    }

    pub(crate) fn install_bytes(&self, document: &mut blitz_script::ScriptDocument) {
        script::install(self, document);
    }

    pub(super) fn export_bytes(&self, block: Block) -> Result<Value, GpuError> {
        self.byte_table()
            .insert(block, false)
            .map(|id| json!({"byteId": id}))
    }

    pub(super) fn release_reply(&self, value: &Value) {
        let id = value
            .get("byteId")
            .and_then(Value::as_u64)
            .or_else(|| value.get("data")?.get("byteId")?.as_u64());
        if let Some(id) = id {
            self.byte_table().discard(id);
        }
    }
}

impl DeviceConnection {
    pub(super) fn input_bytes(&self, id: u64) -> Result<Arc<Vec<u8>>, GpuError> {
        let owner = self
            .owner
            .upgrade()
            .ok_or_else(|| GpuError::internal("byte document was destroyed"))?;
        Bridge(owner).byte_table().take_input(id)
    }

    pub(super) fn export_bytes(&self, block: Block) -> Result<Value, GpuError> {
        let owner = self
            .owner
            .upgrade()
            .ok_or_else(|| GpuError::internal("byte document was destroyed"))?;
        Bridge(owner).export_bytes(block)
    }
}
