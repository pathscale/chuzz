//! Native byte snapshots. JSON contains handles and descriptors only.

use blitz_dom::node::RasterImageData;
use blitz_script::boa_engine::object::builtins::{AlignedVec, JsArray, JsArrayBuffer};
use blitz_script::boa_engine::{
    Context, JsNativeError, JsResult, JsString, JsValue, NativeFunction,
};

use super::resources::{ALLOCATION_LIMIT, RESOURCE_LIMIT};
use super::*;

const TABLE_BYTES_LIMIT: u64 = 512 * 1024 * 1024;
const MAX_ID: u64 = 9_007_199_254_740_991;

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
            return Err(GpuError {
                kind: "out-of-memory",
                message: "WebGPU transfer limit reached in chuzz".to_owned(),
            });
        }
        entries.next_id += 1;
        entries.allocated += size as u64;
        Ok(entries.next_id)
    }

    fn upload(&self, bytes: &[u8]) -> Result<u64, GpuError> {
        let mut entries = lock(&self.0);
        let id = Self::reserve(&mut entries, bytes.len())?;
        // Snapshot only the selected view. The page retains its allocation.
        entries.values.insert(id, Entry {
            block: Block::Buffer(Arc::new(bytes.to_vec())),
            input: true,
            claimed: false,
        });
        Ok(id)
    }

    pub(super) fn insert(&self, block: Block, input: bool) -> Result<u64, GpuError> {
        let mut entries = lock(&self.0);
        let id = Self::reserve(&mut entries, block.data().len())?;
        entries.values.insert(id, Entry {
            block,
            input,
            claimed: false,
        });
        Ok(id)
    }

    fn remove(entries: &mut Entries, id: u64) -> Option<Entry> {
        let entry = entries.values.remove(&id)?;
        entries.allocated -= entry.block.data().len() as u64;
        Some(entry)
    }

    fn discard(&self, id: u64) {
        let mut entries = lock(&self.0);
        Self::remove(&mut entries, id);
        if let Some(size) = entries.active.remove(&id) {
            entries.allocated -= size;
        }
    }

    fn release_staged(&self, id: u64) {
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
                    && matches!(source.get("kind").and_then(Value::as_str), Some("pixels" | "encoded"))
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
                && entries.values.get(id).is_some_and(|entry| entry.input && !entry.claimed)
        });
        if !valid {
            for id in &ids {
                if entries.values.get(id).is_some_and(|entry| entry.input && !entry.claimed) {
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
        Ok(Lease { table: Arc::downgrade(self), ids })
    }

    pub(super) fn take_input(&self, id: u64) -> Result<Arc<Vec<u8>>, GpuError> {
        let mut entries = lock(&self.0);
        if !entries.values.get(&id).is_some_and(|entry| entry.input && entry.claimed) {
            return Err(GpuError::validation("invalid or consumed WebGPU byte handle"));
        }
        let entry = entries.values.remove(&id)
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

    fn take_output(&self, id: u64) -> Result<Block, GpuError> {
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

struct NativeHost {
    bridge: Bridge,
    table: Arc<Table>,
}

impl Drop for NativeHost {
    fn drop(&mut self) {
        // Worker clones of Bridge must not prolong a departed page's bytes.
        self.table.close();
    }
}

fn native_error(error: GpuError) -> blitz_script::boa_engine::JsError {
    JsNativeError::typ().with_message(error.message).into()
}

fn table(context: &Context) -> JsResult<Arc<Table>> {
    context.get_data::<NativeHost>()
        .map(|host| Arc::clone(&host.table))
        .ok_or_else(|| JsNativeError::typ().with_message("WebGPU byte host is unavailable").into())
}

fn integer(args: &[JsValue], index: usize, context: &mut Context) -> JsResult<u64> {
    let value = args.get(index).cloned().unwrap_or_default().to_number(context)?;
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 || value > MAX_ID as f64 {
        return Err(JsNativeError::typ().with_message("invalid WebGPU byte range").into());
    }
    Ok(value as u64)
}

fn put(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let table = table(context)?;
    let offset = integer(args, 1, context)?;
    let length = integer(args, 2, context)?;
    let object = args.first().and_then(JsValue::as_object)
        .ok_or_else(|| JsNativeError::typ().with_message("expected an ArrayBuffer"))?;
    let buffer = JsArrayBuffer::from_object(object)?;
    let data = buffer.data()
        .ok_or_else(|| JsNativeError::typ().with_message("detached WebGPU upload"))?;
    let end = offset.checked_add(length)
        .filter(|end| *end <= data.len() as u64)
        .ok_or_else(|| JsNativeError::typ().with_message("WebGPU data range is out of bounds"))?;
    let id = table.upload(&data[offset as usize..end as usize]).map_err(native_error)?;
    Ok(JsValue::from(id as f64))
}

fn take(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let table = table(context)?;
    let id = integer(args, 0, context)?;
    let block = table.take_output(id).map_err(native_error)?;
    let bytes = AlignedVec::from_iter(0, block.data().iter().copied());
    Ok(JsArrayBuffer::from_byte_block(bytes, context)?.into())
}

fn release(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let table = table(context)?;
    let id = integer(args, 0, context)?;
    table.release_staged(id);
    Ok(JsValue::undefined())
}

fn send(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let bridge = context.get_data::<NativeHost>()
        .map(|host| host.bridge.clone())
        .ok_or_else(|| JsNativeError::typ().with_message("WebGPU byte host is unavailable"))?;
    let table = table(context)?;
    let object = args.get(1).and_then(JsValue::as_object)
        .ok_or_else(|| JsNativeError::typ().with_message("expected byte handles"))?;
    let uploads = JsArray::from_object(object)?;
    let count = uploads.length(context)?;
    if count > RESOURCE_LIMIT as u64 {
        return Err(JsNativeError::typ().with_message("too many byte handles").into());
    }
    let mut ids = Vec::with_capacity(count as usize);
    for index in 0..count {
        let value = uploads.get(index, context)?;
        ids.push(integer(&[value], 0, context)?);
    }
    let message = args.first().cloned().unwrap_or_default()
        .to_string(context)?.to_std_string_escaped();
    let handled = bridge.handle(&message);
    // Unknown descriptor fields and rejected admission cannot leave uploads
    // behind. Claimed handles are released by their command lease instead.
    for id in ids {
        table.release_staged(id);
    }
    if !handled {
        return Err(JsNativeError::typ().with_message("invalid WebGPU command").into());
    }
    Ok(JsValue::undefined())
}

impl Bridge {
    pub(super) fn byte_table(&self) -> Arc<Table> {
        Arc::clone(&self.state().bytes)
    }

    pub(crate) fn install_bytes(&self, document: &mut blitz_script::ScriptDocument) {
        document.with_js_context(|context| {
            context.insert_data(NativeHost {
                bridge: self.clone(),
                table: self.byte_table(),
            });
            type Callback = fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>;
            let functions: [(&str, usize, Callback); 4] = [
                ("__chuzzGpuPutBytes", 3, put),
                ("__chuzzGpuTakeBytes", 1, take),
                ("__chuzzGpuReleaseBytes", 1, release),
                ("__chuzzGpuSend", 2, send),
            ];
            for (name, length, function) in functions {
                context.register_global_builtin_callable(
                    JsString::from(name),
                    length,
                    NativeFunction::from_fn_ptr(function),
                ).expect("WebGPU byte bridge");
            }
        });
    }

    pub(super) fn export_bytes(&self, block: Block) -> Result<Value, GpuError> {
        self.byte_table().insert(block, false).map(|id| json!({"byteId": id}))
    }

    pub(super) fn release_reply(&self, value: &Value) {
        let id = value.get("byteId").and_then(Value::as_u64)
            .or_else(|| value.get("data")?.get("byteId")?.as_u64());
        if let Some(id) = id {
            self.byte_table().discard(id);
        }
    }
}

impl DeviceConnection {
    pub(super) fn input_bytes(&self, id: u64) -> Result<Arc<Vec<u8>>, GpuError> {
        let owner = self.owner.upgrade()
            .ok_or_else(|| GpuError::internal("byte document was destroyed"))?;
        Bridge(owner).byte_table().take_input(id)
    }

    pub(super) fn export_bytes(&self, block: Block) -> Result<Value, GpuError> {
        let owner = self.owner.upgrade()
            .ok_or_else(|| GpuError::internal("byte document was destroyed"))?;
        Bridge(owner).export_bytes(block)
    }
}

