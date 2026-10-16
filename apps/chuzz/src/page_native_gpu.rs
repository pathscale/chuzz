use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

use crate::page_native_bridge::ChuzzBridge;
use crate::script_host::{HostBuffer, HostObject, HostValue};
use blitz_script::native::{Error, HostJob, Result, Value};

const GPU_STATE_KEY: &str = "gpu.realm";
const GPU_OBJECT_LIMIT: u64 = 16_384;
const GPU_BYTE_LIMIT: usize = 64 * 1024 * 1024;
const GPU_REQUEST_LIMIT: usize = 4096;
const GPU_COMMAND_LIMIT: usize = 8192;

#[derive(Clone)]
struct GpuPending {
    promise: Value,
    device: u64,
}

struct GpuRealm {
    next_object: u64,
    next_request: u64,
    pending: BTreeMap<u64, GpuPending>,
    canvases: BTreeMap<u64, Value>,
    jobs: Rc<RefCell<VecDeque<HostJob>>>,
}

impl Default for GpuRealm {
    fn default() -> Self {
        Self {
            next_object: 1,
            next_request: 1,
            pending: BTreeMap::new(),
            canvases: BTreeMap::new(),
            jobs: Rc::new(RefCell::new(VecDeque::new())),
        }
    }
}

#[derive(Clone)]
struct GpuCanvas {
    id: u64,
    width: u32,
    height: u32,
}

fn native(name: &str) -> Value {
    Value::native(name)
}

fn put_method(target: &Value, key: &str, name: &str) {
    target.put(key, native(name));
}

fn object(kind: &str) -> Value {
    Value::object(kind)
}

fn record(fields: impl IntoIterator<Item = (String, Value)>) -> Value {
    let value = object("Record");
    for (key, item) in fields {
        value.put(&key, item);
    }
    value
}

fn set_u64(target: &Value, key: &str, value: u64) {
    target.put(key, Value::number(value as f64));
}

fn number(value: &Value, fallback: f64) -> f64 {
    if matches!(value, Value::Undefined) {
        fallback
    } else {
        value.number_value()
    }
}

fn safe_uint(value: &Value, fallback: u64, label: &str) -> Result<u64> {
    let number = number(value, fallback as f64);
    if !number.is_finite()
        || number < 0.0
        || number.fract() != 0.0
        || number > 9_007_199_254_740_991.0
    {
        return Err(Error::type_error(format!("invalid {label}")));
    }
    Ok(number as u64)
}

fn values_to_host(value: &Value) -> Result<HostValue> {
    if let Some(object) = value.as_object() {
        let borrowed = object.borrow();
        if let Some(root) = borrowed
            .payload
            .as_ref()
            .and_then(|payload| payload.downcast_ref::<HostObject>())
        {
            return Ok(HostValue::Object(root.clone()));
        }
        if let Some(buffer) = borrowed
            .payload
            .as_ref()
            .and_then(|payload| payload.downcast_ref::<Rc<RefCell<HostBuffer>>>())
        {
            let bytes = buffer
                .borrow()
                .as_bytes()
                .map_err(|_| Error::type_error("detached GPU buffer"))?
                .to_vec();
            return Ok(HostValue::Buffer(HostBuffer::new(bytes)));
        }
        if borrowed.kind.starts_with("GPU")
            && let Some(property) = borrowed.props.get("__gpuId")
        {
            return Ok(HostValue::Number(property.value.number_value()));
        }
        if let Some(payload) = borrowed.payload.as_ref()
            && let Some(buffer) = payload.downcast_ref::<HostBuffer>()
        {
            let bytes = buffer
                .as_bytes()
                .map_err(|_| Error::type_error("detached GPU buffer"))?;
            return Ok(HostValue::Buffer(HostBuffer::new(bytes.to_vec())));
        }
        if matches!(
            borrowed.kind.as_str(),
            "Uint8Array"
                | "Uint8ClampedArray"
                | "Int8Array"
                | "Uint16Array"
                | "Int16Array"
                | "Uint32Array"
                | "Int32Array"
                | "Float32Array"
                | "Float64Array"
        ) {
            let kind = borrowed.kind.clone();
            let items = borrowed.indexed.clone();
            drop(borrowed);
            let mut bytes = Vec::new();
            for item in items {
                let n = item.number_value();
                match kind.as_str() {
                    "Uint8Array" | "Uint8ClampedArray" | "Int8Array" => {
                        bytes.push(if n.is_finite() {
                            n.trunc().rem_euclid(256.0) as u8
                        } else {
                            0
                        })
                    }
                    "Uint16Array" => bytes.extend(
                        (if n.is_finite() {
                            n.trunc().rem_euclid(65_536.0) as u16
                        } else {
                            0
                        })
                        .to_le_bytes(),
                    ),
                    "Int16Array" => {
                        bytes.extend((if n.is_finite() { n as i16 } else { 0 }).to_le_bytes())
                    }
                    "Uint32Array" => bytes.extend(
                        (if n.is_finite() {
                            n.trunc().rem_euclid(4_294_967_296.0) as u32
                        } else {
                            0
                        })
                        .to_le_bytes(),
                    ),
                    "Int32Array" => {
                        bytes.extend((if n.is_finite() { n as i32 } else { 0 }).to_le_bytes())
                    }
                    "Float32Array" => bytes.extend((n as f32).to_le_bytes()),
                    "Float64Array" => bytes.extend(n.to_le_bytes()),
                    _ => {}
                }
                if bytes.len() > GPU_BYTE_LIMIT {
                    return Err(Error::type_error("WebGPU transfer limit reached"));
                }
            }
            return Ok(HostValue::Buffer(HostBuffer::new(bytes)));
        }
        if borrowed.kind == "Array" || borrowed.kind == "NodeList" {
            let indexed = borrowed.indexed.clone();
            drop(borrowed);
            return indexed
                .iter()
                .map(values_to_host)
                .collect::<Result<Vec<_>>>()
                .map(HostValue::Array);
        }
        let props = borrowed
            .props
            .iter()
            .map(|(key, prop)| (key.clone(), prop.value.clone(), prop.enumerable))
            .collect::<Vec<_>>();
        let mut fields = BTreeMap::new();
        for (key, item, enumerable) in props {
            if !enumerable || matches!(item, Value::Undefined) {
                continue;
            }
            fields.insert(key, values_to_host(&item)?);
        }
        return Ok(HostValue::Record(fields));
    }
    match value {
        Value::Undefined => Ok(HostValue::Undefined),
        Value::Null => Ok(HostValue::Null),
        Value::Bool(value) => Ok(HostValue::Boolean(*value)),
        Value::Number(value) => Ok(HostValue::Number(*value)),
        Value::String(value) => Ok(HostValue::Utf16(Rc::clone(value))),
        Value::Object(_) => Err(Error::type_error("unsupported GPU host value")),
    }
}

fn host_to_value(value: HostValue) -> Value {
    match value {
        HostValue::Undefined => Value::Undefined,
        HostValue::Null => Value::Null,
        HostValue::Boolean(value) => Value::Bool(value),
        HostValue::Number(value) => Value::Number(value),
        HostValue::Utf16(value) => Value::String(value),
        HostValue::String(value) => Value::string(value.as_ref()),
        HostValue::Buffer(value) => {
            let output = object("Uint8Array");
            if let Some(storage) = output.as_object() {
                let Ok(bytes) = value.as_bytes() else {
                    return Value::error("TypeError", "host supplied a detached GPU buffer");
                };
                storage.borrow_mut().indexed = bytes
                    .iter()
                    .map(|byte| Value::Number(f64::from(*byte)))
                    .collect();
                output.put("length", Value::number(bytes.len() as f64));
                storage.borrow_mut().payload = Some(Rc::new(value));
            }
            output
        }
        HostValue::Object(value) => {
            let output = object("HostObject");
            if let Some(storage) = output.as_object() {
                storage.borrow_mut().payload = Some(Rc::new(value));
            }
            output
        }
        HostValue::Array(values) => Value::array(values.into_iter().map(host_to_value).collect()),
        HostValue::Record(values) => {
            let output = object("Record");
            for (key, value) in values {
                output.put(&key, host_to_value(value));
            }
            output
        }
    }
}

fn promise() -> Value {
    Value::pending_promise()
}

fn gpu_id(value: &Value) -> Result<u64> {
    if !value.kind().starts_with("GPU") {
        return Err(Error::type_error("invalid WebGPU receiver"));
    }
    safe_uint(&value.get("__gpuId"), 0, "WebGPU object")
}

fn object_with_id(kind: &str, id: u64) -> Value {
    let value = object(kind);
    set_u64(&value, "__gpuId", id);
    value
}

fn make_gpu_queue(device: &Value, id: u64) -> Value {
    let queue = object_with_id("GPUQueue", id);
    queue.put_hidden("__gpuDevice", device.clone());
    methods(
        &queue,
        &[
            ("submit", "GPUQueue.submit"),
            ("writeBuffer", "GPUQueue.writeBuffer"),
            ("writeTexture", "GPUQueue.writeTexture"),
            (
                "copyExternalImageToTexture",
                "GPUQueue.copyExternalImageToTexture",
            ),
            ("onSubmittedWorkDone", "GPUQueue.onSubmittedWorkDone"),
        ],
    );
    queue
}

fn handle_value(value: &Value) -> Result<u64> {
    safe_uint(&value.get("__gpuId"), 0, "WebGPU object")
}

fn payload_fields(value: &Value) -> Result<Value> {
    if matches!(value, Value::Undefined | Value::Null) {
        return Ok(object("Record"));
    }
    if value.as_object().is_none() {
        return Err(Error::type_error("WebGPU descriptor must be an object"));
    }
    Ok(value.clone())
}

fn consts(global: &Value, name: &str, names: &[(&str, u32)]) {
    let value = object(name);
    for (key, bits) in names {
        set_u64(&value, key, u64::from(*bits));
    }
    global.put(name, value);
}

fn methods(target: &Value, entries: &[(&str, &str)]) {
    for (key, name) in entries {
        put_method(target, key, name);
    }
}

impl ChuzzBridge {
    pub(super) fn apply_native_frame(
        &mut self,
        doc: &mut blitz_dom::BaseDocument,
        frame: crate::script_host::HostFrame,
    ) -> Result<bool> {
        use crate::script_host::HostFrameSource;
        if !matches!(
            frame.source,
            HostFrameSource::WebGpu | HostFrameSource::Canvas
        ) {
            return Ok(false);
        }
        let context = self
            .gpu_realm()?
            .borrow()
            .canvases
            .get(&frame.target)
            .cloned();
        let Some(context) = context else {
            return Ok(false);
        };
        if frame.source == HostFrameSource::WebGpu
            && context.get("__generation").number_value() as u64 != frame.generation
        {
            return Ok(false);
        }
        let Some(node) = context.get("canvas").node_id() else {
            return Ok(false);
        };
        if doc.get_node(node).is_none() {
            return Ok(false);
        }
        doc.snapshot_node_and(node, |node| {
            if let Some(element) = node.element_data_mut() {
                element.special_data = blitz_dom::node::SpecialElementData::Image(Box::new(
                    frame
                        .image
                        .map_or(blitz_dom::node::ImageData::None, |image| {
                            blitz_dom::node::ImageData::Raster((*image).clone())
                        }),
                ));
            }
        });
        Ok(true)
    }
    pub(super) fn install_gpu(&mut self, global: &Value) -> Result<()> {
        let realm = Rc::new(RefCell::new(GpuRealm::default()));
        let holder = object("GpuRealmState");
        if let Some(storage) = holder.as_object() {
            storage.borrow_mut().payload = Some(realm.clone());
        }
        self.state.insert(GPU_STATE_KEY.to_owned(), holder);

        let jobs = Rc::clone(&realm.borrow().jobs);
        let callback = self
            .host
            .retain_callback(move |_host, args| {
                let args = args
                    .iter()
                    .map(|item| match item {
                        HostValue::String(value) => Value::string(value.as_ref()),
                        HostValue::Utf16(value) => Value::String(Rc::clone(value)),
                        other => host_to_value_clone(other),
                    })
                    .collect();
                jobs.borrow_mut().push_back(HostJob {
                    callback: native("GPU.__deliver"),
                    this: Value::Undefined,
                    args,
                });
                Ok(HostValue::Undefined)
            })
            .map_err(|error| Error::type_error(error.to_string()))?;
        let root = HostValue::Object(HostObject::new(callback));
        self.host
            .invoke_intrinsic("__chuzzGpuSubscribe", &[root])
            .map_err(|error| Error::type_error(error.to_string()))?;

        let gpu = object("GPU");
        methods(
            &gpu,
            &[
                ("requestAdapter", "GPU.requestAdapter"),
                ("getPreferredCanvasFormat", "GPU.getPreferredCanvasFormat"),
            ],
        );
        global.put("GPU", gpu.clone());
        global.put_hidden("__gpuNativeSurface", Value::Bool(true));
        consts(
            global,
            "GPUBufferUsage",
            &[
                ("MAP_READ", 1),
                ("MAP_WRITE", 2),
                ("COPY_SRC", 4),
                ("COPY_DST", 8),
                ("INDEX", 16),
                ("VERTEX", 32),
                ("UNIFORM", 64),
                ("STORAGE", 128),
                ("INDIRECT", 256),
                ("QUERY_RESOLVE", 512),
            ],
        );
        consts(
            global,
            "GPUTextureUsage",
            &[
                ("COPY_SRC", 1),
                ("COPY_DST", 2),
                ("TEXTURE_BINDING", 4),
                ("STORAGE_BINDING", 8),
                ("RENDER_ATTACHMENT", 16),
            ],
        );
        consts(
            global,
            "GPUShaderStage",
            &[("VERTEX", 1), ("FRAGMENT", 2), ("COMPUTE", 4)],
        );
        consts(global, "GPUMapMode", &[("READ", 1), ("WRITE", 2)]);
        consts(
            global,
            "GPUColorWrite",
            &[
                ("RED", 1),
                ("GREEN", 2),
                ("BLUE", 4),
                ("ALPHA", 8),
                ("ALL", 15),
            ],
        );
        consts(
            global,
            "GPUTextureAspect",
            &[("ALL", 0), ("STENCIL_ONLY", 1), ("DEPTH_ONLY", 2)],
        );
        global.put("GPUCanvasContext", Value::native("GPU.illegalConstructor"));
        global.put("createImageBitmap", native("GPU.createImageBitmap"));

        let navigator = global.get("navigator");
        if navigator.as_object().is_some() {
            navigator.put("gpu", gpu);
        }
        Ok(())
    }

    fn gpu_realm(&self) -> Result<Rc<RefCell<GpuRealm>>> {
        let holder = self
            .state
            .get(GPU_STATE_KEY)
            .cloned()
            .ok_or_else(|| Error::type_error("WebGPU bridge is not installed"))?;
        let object = holder
            .as_object()
            .ok_or_else(|| Error::type_error("invalid WebGPU bridge state"))?;
        let payload = object
            .borrow()
            .payload
            .clone()
            .ok_or_else(|| Error::type_error("missing WebGPU bridge state"))?;
        Rc::downcast::<RefCell<GpuRealm>>(payload)
            .map_err(|_| Error::type_error("invalid WebGPU bridge state"))
    }

    pub(super) fn gpu_pending(&self) -> bool {
        self.gpu_realm().is_ok_and(|realm| {
            !realm.borrow().pending.is_empty() || !realm.borrow().jobs.borrow().is_empty()
        })
    }

    pub(super) fn gpu_task_count(&self) -> u64 {
        self.gpu_realm().map_or(0, |realm| {
            let realm = realm.borrow();
            (realm.pending.len() + realm.jobs.borrow().len()) as u64
        })
    }

    fn allocate_object(&mut self) -> Result<u64> {
        let realm = self.gpu_realm()?;
        let mut realm = realm.borrow_mut();
        let id = realm.next_object;
        if id == 0 || id > GPU_OBJECT_LIMIT {
            return Err(Error::type_error("WebGPU object limit reached"));
        }
        realm.next_object += 1;
        Ok(id)
    }

    fn reserve_pending(&mut self, device: u64) -> Result<(u64, Value)> {
        let realm = self.gpu_realm()?;
        let mut realm = realm.borrow_mut();
        if realm.pending.len() >= GPU_REQUEST_LIMIT {
            return Err(Error::type_error("WebGPU request limit reached"));
        }
        let id = realm.next_request;
        if id == 0 || id > 9_007_199_254_740_991 {
            return Err(Error::type_error("WebGPU request identifiers exhausted"));
        }
        realm.next_request += 1;
        let promise = promise();
        realm.pending.insert(
            id,
            GpuPending {
                promise: promise.clone(),
                device,
            },
        );
        Ok((id, promise))
    }

    fn send_json(&mut self, request: Value, uploads: Vec<Value>) -> Result<()> {
        let request = values_to_host(&request)?;
        let uploads = uploads
            .into_iter()
            .map(|value| values_to_host(&value))
            .collect::<Result<Vec<_>>>()?;
        self.host
            .invoke_intrinsic("__chuzzGpuSend", &[request, HostValue::Array(uploads)])
            .map_err(|error| Error::type_error(error.to_string()))?;
        Ok(())
    }

    fn submit(
        &mut self,
        operation: &str,
        device: u64,
        command: Value,
        fire: bool,
        uploads: Vec<u64>,
    ) -> Result<Value> {
        let (id, promise) = if fire {
            (self.next_fire_id()?, Value::Undefined)
        } else {
            self.reserve_pending(device)?
        };
        let request = object("Record");
        request.put("webgpu", Value::string("command"));
        request.put("id", Value::number(id as f64));
        request.put("device", Value::number(device as f64));
        request.put("fire", Value::Bool(fire));
        request.put("command", command);
        let _ = operation;
        self.send_json(
            request,
            uploads
                .into_iter()
                .map(|id| Value::number(id as f64))
                .collect(),
        )?;
        Ok(promise)
    }

    fn next_fire_id(&mut self) -> Result<u64> {
        let realm = self.gpu_realm()?;
        let mut realm = realm.borrow_mut();
        let id = realm.next_request;
        if id == 0 || id > 9_007_199_254_740_991 {
            return Err(Error::type_error("WebGPU request identifiers exhausted"));
        }
        realm.next_request += 1;
        Ok(id)
    }

    pub(super) fn gpu_take_jobs(&mut self) -> Result<Vec<HostJob>> {
        let realm = self.gpu_realm()?;
        Ok(realm.borrow_mut().jobs.borrow_mut().drain(..).collect())
    }

    fn command(
        &mut self,
        device: u64,
        op: &str,
        handle: u64,
        args: Value,
        fire: bool,
    ) -> Result<Value> {
        let command = record([
            ("op".to_owned(), Value::string(op)),
            ("handle".to_owned(), Value::number(handle as f64)),
            ("args".to_owned(), args),
        ]);
        self.submit("command", device, command, fire, Vec::new())
    }

    fn send_adapter(&mut self, options: Value) -> Result<Value> {
        let (id, promise) = self.reserve_pending(0)?;
        let request = record([
            ("webgpu".to_owned(), Value::string("requestAdapter")),
            ("id".to_owned(), Value::number(id as f64)),
            ("options".to_owned(), options),
        ]);
        self.send_json(request, Vec::new())?;
        Ok(promise)
    }

    fn send_device(&mut self, adapter: u64, options: Value) -> Result<Value> {
        let handle = self.allocate_object()?;
        let (id, promise) = self.reserve_pending(0)?;
        let request = record([
            ("webgpu".to_owned(), Value::string("requestDevice")),
            ("id".to_owned(), Value::number(id as f64)),
            ("adapter".to_owned(), Value::number(adapter as f64)),
            ("options".to_owned(), options),
        ]);
        promise.put_hidden("__gpuDeviceHandle", Value::number(handle as f64));
        self.send_json(request, Vec::new())?;
        Ok(promise)
    }

    fn upload(&mut self, value: &Value) -> Result<u64> {
        let bytes = bytes_from_value(value)?;
        if bytes.len() > GPU_BYTE_LIMIT {
            return Err(Error::type_error("WebGPU transfer limit reached"));
        }
        let length = bytes.len();
        let id = self
            .host
            .invoke_intrinsic(
                "__chuzzGpuPutBytes",
                &[
                    HostValue::Buffer(HostBuffer::new(bytes)),
                    HostValue::Number(0.0),
                    HostValue::Number(length as f64),
                ],
            )
            .map_err(|error| Error::type_error(error.to_string()))?;
        match id {
            HostValue::Number(id) if id.is_finite() && id > 0.0 => Ok(id as u64),
            _ => Err(Error::type_error("invalid WebGPU transfer handle")),
        }
    }

    fn deliver(&mut self, args: Vec<Value>) -> Result<Value> {
        let Some(kind) = args.first().map(Value::text) else {
            return Ok(Value::Undefined);
        };
        if kind != "reply" {
            return Ok(Value::Undefined);
        }
        let Some(id) = args.get(1) else {
            return Ok(Value::Undefined);
        };
        let id = safe_uint(id, 0, "WebGPU reply id")?;
        let payload = args.get(2).cloned().unwrap_or(Value::Undefined);
        let realm = self.gpu_realm()?;
        let pending = realm.borrow_mut().pending.remove(&id);
        let Some(pending) = pending else {
            return Ok(Value::Undefined);
        };
        if payload.get("ok").truthy() {
            let mut value = payload.get("value");
            if !matches!(pending.promise.get("__gpuDeviceHandle"), Value::Undefined) {
                let id = safe_uint(&value.get("handle"), 0, "GPUDevice")?;
                value = self.make_device(id, value)?;
            } else if pending.promise.get("__gpuImageBitmap").truthy() {
                let bitmap = object("ImageBitmap");
                bitmap.put("width", value.get("width"));
                bitmap.put("height", value.get("height"));
                let image = value.get("data");
                let pixels = if matches!(image, Value::Null | Value::Undefined) {
                    Value::Null
                } else {
                    image.get("data")
                };
                bitmap.put("data", pixels);
                bitmap.put_hidden("__bitmapId", value.get("handle"));
                bitmap.put_hidden("__closed", Value::Bool(false));
                put_method(&bitmap, "close", "GPUImageBitmap.close");
                value = bitmap;
            } else if !matches!(pending.promise.get("__gpuMapTarget"), Value::Undefined) {
                let buffer = pending.promise.get("__gpuMapTarget");
                buffer.put_hidden("__mapped", value);
                buffer.put_hidden("__mapState", Value::string("mapped"));
                value = Value::Undefined;
            } else if value.kind() == "Record" && !matches!(value.get("handle"), Value::Undefined) {
                let kind = if !matches!(value.get("features"), Value::Undefined)
                    && !matches!(value.get("isFallbackAdapter"), Value::Undefined)
                {
                    "GPUAdapter"
                } else {
                    "GPUObject"
                };
                let id = safe_uint(&value.get("handle"), 0, "WebGPU object")?;
                value = self.make_from_reply(kind, id, value)?;
            }
            self.resolve_promise(pending.promise, value, false)?;
        } else {
            let error = Value::error("OperationError", payload.get("error").text());
            error.put("kind", payload.get("kind"));
            error.put("device", Value::number(pending.device as f64));
            self.resolve_promise(pending.promise, error, true)?;
        }
        Ok(Value::Undefined)
    }

    fn make_from_reply(&mut self, kind: &str, id: u64, reply: Value) -> Result<Value> {
        let value = object_with_id(kind, id);
        if kind == "GPUAdapter" {
            put_method(&value, "requestDevice", "GPUAdapter.requestDevice");
        }
        for key in [
            "label",
            "features",
            "limits",
            "info",
            "adapterInfo",
            "isFallbackAdapter",
            "description",
        ] {
            let field = reply.get(key);
            if !matches!(field, Value::Undefined) {
                if key == "features" {
                    let set = Value::array(field.items());
                    set.put("size", Value::number(field.items().len() as f64));
                    put_method(&set, "has", "GPUFeatures.has");
                    put_method(&set, "keys", "GPUFeatures.keys");
                    value.put(key, set);
                } else {
                    value.put(key, field);
                }
            }
        }
        if kind == "GPUDevice" {
            value.put("queue", make_gpu_queue(&value, id));
            methods(
                &value,
                &[
                    ("createBuffer", "GPUDevice.createBuffer"),
                    ("createTexture", "GPUDevice.createTexture"),
                    ("createSampler", "GPUDevice.createSampler"),
                    ("createShaderModule", "GPUDevice.createShaderModule"),
                    ("createBindGroupLayout", "GPUDevice.createBindGroupLayout"),
                    ("createPipelineLayout", "GPUDevice.createPipelineLayout"),
                    ("createBindGroup", "GPUDevice.createBindGroup"),
                    ("createRenderPipeline", "GPUDevice.createRenderPipeline"),
                    ("createCommandEncoder", "GPUDevice.createCommandEncoder"),
                    ("pushErrorScope", "GPUDevice.pushErrorScope"),
                    ("popErrorScope", "GPUDevice.popErrorScope"),
                    ("destroy", "GPUDevice.destroy"),
                ],
            );
            for key in ["features", "limits"] {
                if let Some(obj) = value.get(key).as_object() {
                    let _ = obj;
                }
            }
        }
        Ok(value)
    }

    fn make_device(&mut self, id: u64, reply: Value) -> Result<Value> {
        let value = self.make_from_reply("GPUDevice", id, reply)?;
        value.put_hidden("__lost", promise());
        Ok(value)
    }

    fn make_resource(&mut self, kind: &str, device: u64, descriptor: Value) -> Result<Value> {
        let id = self.allocate_object()?;
        let resource = object_with_id(kind, id);
        resource.put_hidden("__deviceId", Value::number(device as f64));
        resource.put("label", descriptor.get("label"));
        match kind {
            "GPUBuffer" => {
                resource.put("size", descriptor.get("size"));
                resource.put("usage", descriptor.get("usage"));
                methods(
                    &resource,
                    &[
                        ("mapAsync", "GPUBuffer.mapAsync"),
                        ("getMappedRange", "GPUBuffer.getMappedRange"),
                        ("unmap", "GPUBuffer.unmap"),
                        ("destroy", "GPUBuffer.destroy"),
                    ],
                );
            }
            "GPUTexture" => {
                resource.put("format", descriptor.get("format"));
                resource.put("usage", descriptor.get("usage"));
                methods(
                    &resource,
                    &[
                        ("createView", "GPUTexture.createView"),
                        ("destroy", "GPUTexture.destroy"),
                    ],
                );
            }
            "GPUShaderModule" => methods(
                &resource,
                &[("getCompilationInfo", "GPUShaderModule.getCompilationInfo")],
            ),
            "GPUCommandEncoder" => methods(
                &resource,
                &[
                    ("beginRenderPass", "GPUCommandEncoder.beginRenderPass"),
                    (
                        "copyBufferToTexture",
                        "GPUCommandEncoder.copyBufferToTexture",
                    ),
                    (
                        "copyTextureToBuffer",
                        "GPUCommandEncoder.copyTextureToBuffer",
                    ),
                    (
                        "copyTextureToTexture",
                        "GPUCommandEncoder.copyTextureToTexture",
                    ),
                    ("finish", "GPUCommandEncoder.finish"),
                ],
            ),
            "GPURenderPassEncoder" => methods(
                &resource,
                &[
                    ("setPipeline", "GPURenderPass.setPipeline"),
                    ("setBindGroup", "GPURenderPass.setBindGroup"),
                    ("setVertexBuffer", "GPURenderPass.setVertexBuffer"),
                    ("setIndexBuffer", "GPURenderPass.setIndexBuffer"),
                    ("draw", "GPURenderPass.draw"),
                    ("drawIndexed", "GPURenderPass.drawIndexed"),
                    ("end", "GPURenderPass.end"),
                ],
            ),
            "GPURenderPipeline" => put_method(
                &resource,
                "getBindGroupLayout",
                "GPUObject.getBindGroupLayout",
            ),
            _ => {}
        }
        if kind == "GPUCommandEncoder" {
            resource.put_hidden("__operations", Value::array(Vec::new()));
        }
        let args = normalize_descriptor(kind, &descriptor)?;
        self.command(
            device,
            &format!("create{}", kind.trim_start_matches("GPU")),
            id,
            args,
            true,
        )?;
        Ok(resource)
    }

    fn canvas_for(&mut self, doc: &mut blitz_dom::BaseDocument, node: &Value) -> Result<GpuCanvas> {
        let node_id = node
            .node_id()
            .ok_or_else(|| Error::type_error("canvas receiver is not a live node"))?;
        let dom_node = doc
            .get_node(node_id)
            .ok_or_else(|| Error::type_error("canvas node is detached"))?;
        if !dom_node
            .data
            .is_element_with_tag_name(&blitz_dom::local_name!("canvas"))
        {
            return Err(Error::type_error("getContext receiver is not a canvas"));
        }
        let id = if matches!(node.get("__gpuCanvasId"), Value::Undefined) {
            let id = self.allocate_object()?;
            set_u64(node, "__gpuCanvasId", id);
            id
        } else {
            safe_uint(&node.get("__gpuCanvasId"), 0, "canvas")?
        };
        let width = dom_node
            .attr(blitz_dom::local_name!("width"))
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(300);
        let height = dom_node
            .attr(blitz_dom::local_name!("height"))
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(150);
        let _ = node_id;
        Ok(GpuCanvas { id, width, height })
    }

    fn canvas_context(
        &mut self,
        doc: &mut blitz_dom::BaseDocument,
        node: Value,
        kind: &str,
    ) -> Result<Value> {
        let canvas = self.canvas_for(doc, &node)?;
        if let Some(context) = self.gpu_realm()?.borrow().canvases.get(&canvas.id).cloned() {
            return Ok(
                if (kind == "webgpu" && context.kind() == "GPUCanvasContext")
                    || (kind == "2d" && context.kind() == "CanvasRenderingContext2D")
                {
                    context
                } else {
                    Value::Null
                },
            );
        }
        match kind {
            "webgpu" => {
                let context = object_with_id("GPUCanvasContext", canvas.id);
                context.put("canvas", node);
                context.put_hidden("__width", Value::number(canvas.width as f64));
                context.put_hidden("__height", Value::number(canvas.height as f64));
                methods(
                    &context,
                    &[
                        ("configure", "GPUCanvasContext.configure"),
                        ("unconfigure", "GPUCanvasContext.unconfigure"),
                        ("getCurrentTexture", "GPUCanvasContext.getCurrentTexture"),
                        ("getConfiguration", "GPUCanvasContext.getConfiguration"),
                    ],
                );
                self.gpu_realm()?
                    .borrow_mut()
                    .canvases
                    .insert(canvas.id, context.clone());
                Ok(context)
            }
            "2d" => {
                let id = canvas.id;
                let context = self.canvas2d_context(canvas, node)?;
                self.gpu_realm()?
                    .borrow_mut()
                    .canvases
                    .insert(id, context.clone());
                Ok(context)
            }
            _ => Ok(Value::Null),
        }
    }

    fn canvas2d_context(&mut self, canvas: GpuCanvas, node: Value) -> Result<Value> {
        let context = object_with_id("CanvasRenderingContext2D", canvas.id);
        context.put("canvas", node);
        context.put("fillStyle", Value::string("#000000"));
        context.put("strokeStyle", Value::string("#000000"));
        context.put("globalAlpha", Value::number(1.0));
        context.put("globalCompositeOperation", Value::string("source-over"));
        let state = record([
            (
                "matrix".into(),
                Value::array(
                    [1., 0., 0., 1., 0., 0.]
                        .into_iter()
                        .map(Value::number)
                        .collect(),
                ),
            ),
            ("alpha".into(), Value::number(1.0)),
            ("composite".into(), Value::string("source-over")),
            ("width".into(), Value::number(1.0)),
            ("cap".into(), Value::string("butt")),
            ("join".into(), Value::string("miter")),
            ("miter".into(), Value::number(10.0)),
            ("dash".into(), Value::array(Vec::new())),
            ("dashOffset".into(), Value::number(0.0)),
            ("font".into(), Value::string("10px sans-serif")),
            ("align".into(), Value::string("start")),
            ("baseline".into(), Value::string("alphabetic")),
            ("direction".into(), Value::string("inherit")),
            ("smoothing".into(), Value::Bool(true)),
            ("shadowBlur".into(), Value::number(0.0)),
            ("shadowColor".into(), Value::string("rgba(0,0,0,0)")),
            ("shadowOffsetX".into(), Value::number(0.0)),
            ("shadowOffsetY".into(), Value::number(0.0)),
            ("filter".into(), Value::array(Vec::new())),
        ]);
        context.put_hidden("__width", Value::number(canvas.width as f64));
        context.put_hidden("__height", Value::number(canvas.height as f64));
        context.put_hidden("__commands", Value::array(Vec::new()));
        context.put_hidden("__drawState", state);
        methods(
            &context,
            &[
                ("clearRect", "Canvas2D.clearRect"),
                ("fillRect", "Canvas2D.fillRect"),
                ("strokeRect", "Canvas2D.strokeRect"),
                ("beginPath", "Canvas2D.beginPath"),
                ("closePath", "Canvas2D.closePath"),
                ("moveTo", "Canvas2D.moveTo"),
                ("lineTo", "Canvas2D.lineTo"),
                ("rect", "Canvas2D.rect"),
                ("arc", "Canvas2D.arc"),
                ("fill", "Canvas2D.fill"),
                ("stroke", "Canvas2D.stroke"),
                ("save", "Canvas2D.save"),
                ("restore", "Canvas2D.restore"),
                ("translate", "Canvas2D.translate"),
                ("scale", "Canvas2D.scale"),
                ("rotate", "Canvas2D.rotate"),
                ("setTransform", "Canvas2D.setTransform"),
                ("resetTransform", "Canvas2D.resetTransform"),
                ("fillText", "Canvas2D.fillText"),
                ("strokeText", "Canvas2D.strokeText"),
                ("drawImage", "Canvas2D.drawImage"),
                ("getImageData", "Canvas2D.getImageData"),
                ("putImageData", "Canvas2D.putImageData"),
                ("__submit", "Canvas2D.submit"),
            ],
        );
        Ok(context)
    }

    fn create_image_bitmap(&mut self, source: Value, options: Value) -> Result<Value> {
        let encoded = source.kind() == "Blob";
        let (width, height, pixels) = if encoded {
            (0, 0, Value::array(source.items()))
        } else if source.kind() == "ImageData" {
            (
                safe_uint(&source.get("width"), 0, "ImageData width")?,
                safe_uint(&source.get("height"), 0, "ImageData height")?,
                source.get("data"),
            )
        } else if source.kind() == "ImageBitmap" {
            (
                safe_uint(&source.get("width"), 0, "ImageBitmap width")?,
                safe_uint(&source.get("height"), 0, "ImageBitmap height")?,
                source.get("data"),
            )
        } else {
            return Err(Error::type_error(
                "createImageBitmap source must expose decoded pixel data",
            ));
        };
        let expected = width
            .checked_mul(height)
            .and_then(|size| size.checked_mul(4))
            .ok_or_else(|| Error::type_error("ImageBitmap dimensions overflow"))?
            as usize;
        let bytes = bytes_from_value(&pixels)?;
        if (!encoded && bytes.len() != expected) || bytes.len() > GPU_BYTE_LIMIT {
            return Err(Error::type_error(
                "ImageBitmap source data has an invalid length",
            ));
        }
        let token = self.upload(&pixels)?;
        let id = self.allocate_object()?;
        let (request_id, promise) = self.reserve_pending(0)?;
        promise.put_hidden("__gpuImageBitmap", Value::Bool(true));
        let source = if encoded {
            record([
                ("kind".into(), Value::string("encoded")),
                ("data".into(), Value::number(token as f64)),
            ])
        } else {
            record([
                ("kind".into(), Value::string("pixels")),
                ("width".into(), Value::number(width as f64)),
                ("height".into(), Value::number(height as f64)),
                ("data".into(), Value::number(token as f64)),
            ])
        };
        let options = record([
            ("source".into(), source),
            ("readPixels".into(), Value::Bool(true)),
            (
                "flipY".into(),
                Value::Bool(options.get("imageOrientation").text() == "flipY"),
            ),
            (
                "resizeWidth".into(),
                field_or(&options, "resizeWidth", Value::Null),
            ),
            (
                "resizeHeight".into(),
                field_or(&options, "resizeHeight", Value::Null),
            ),
            (
                "resizeQuality".into(),
                field_or(&options, "resizeQuality", Value::string("low")),
            ),
            ("crop".into(), Value::Null),
        ]);
        let command = record([
            ("op".into(), Value::string("createImageBitmap")),
            ("handle".into(), Value::number(id as f64)),
            ("args".into(), options),
        ]);
        let request = record([
            ("webgpu".into(), Value::string("command")),
            ("id".into(), Value::number(request_id as f64)),
            ("device".into(), Value::number(0.0)),
            ("command".into(), command),
        ]);
        self.send_json(request, vec![Value::number(token as f64)])?;
        Ok(promise)
    }

    fn canvas_flush(
        &mut self,
        context: &Value,
        action: &str,
        n: Option<Vec<Value>>,
    ) -> Result<Value> {
        let state = context.get("__drawState");
        let commands = context.get("__commands").items();
        let id = handle_value(context)?;
        let width = safe_uint(&context.get("__width"), 300, "canvas width")? as u32;
        let height = safe_uint(&context.get("__height"), 150, "canvas height")? as u32;
        let request = record([
            ("action".into(), Value::string(action)),
            ("id".into(), Value::number(id as f64)),
            ("width".into(), Value::number(width as f64)),
            ("height".into(), Value::number(height as f64)),
            ("commands".into(), Value::array(commands)),
        ]);
        if let Some(n) = n {
            request.put("n", Value::array(n));
        }
        let result = self
            .host
            .invoke_intrinsic(
                "__chuzzCanvas2D",
                &[values_to_host(&request)?, HostValue::Array(Vec::new())],
            )
            .map_err(|error| Error::type_error(error.to_string()))?;
        context.put_hidden("__commands", Value::array(Vec::new()));
        context.put_hidden("__drawState", state);
        Ok(host_to_value(result))
    }

    fn canvas_record(&mut self, context: &Value, op: &str, args: &[Value]) -> Result<Value> {
        let n = Value::array(
            args.iter()
                .take(8)
                .map(|value| Value::number(value.number_value()))
                .collect(),
        );
        let command = record([("op".into(), Value::string(op)), ("n".into(), n)]);
        command.put("state", context.get("__drawState"));
        command.put(
            "brush",
            record([
                ("kind".into(), Value::string("solid")),
                ("color".into(), context.get("fillStyle")),
            ]),
        );
        let mut commands = context.get("__commands").items();
        if commands.len() >= GPU_COMMAND_LIMIT {
            return Err(Error::type_error("canvas command limit reached"));
        }
        commands.push(command);
        context.put_hidden("__commands", Value::array(commands));
        Ok(Value::Undefined)
    }

    pub(super) fn gpu_get(
        &mut self,
        doc: &mut blitz_dom::BaseDocument,
        receiver: &Value,
        key: &str,
    ) -> Result<Option<Value>> {
        if receiver.kind() == "GPUDevice" && key == "lost" {
            return Ok(Some(receiver.get("__lost")));
        }
        if receiver.kind() == "Node"
            && key == "getContext"
            && let Some(id) = receiver.node_id()
            && doc.get_node(id).is_some_and(|node| {
                node.data
                    .is_element_with_tag_name(&blitz_dom::local_name!("canvas"))
            })
        {
            return Ok(Some(native("GPU.Canvas.getContext")));
        }
        if receiver.kind() == "GPUDevice"
            && key == "queue"
            && matches!(receiver.get(key), Value::Undefined)
        {
            let queue = make_gpu_queue(receiver, gpu_id(receiver)?);
            receiver.put(key, queue.clone());
            return Ok(Some(queue));
        }
        Ok(None)
    }

    pub(super) fn gpu_set(
        &mut self,
        _doc: &mut blitz_dom::BaseDocument,
        receiver: &Value,
        key: &str,
        value: Value,
    ) -> Result<bool> {
        if receiver.kind().starts_with("GPU") && key == "label" {
            receiver.put(key, value.clone());
            if let Ok(id) = gpu_id(receiver) {
                let device = safe_uint(&receiver.get("__deviceId"), id, "GPUDevice")?;
                let _ = self.command(
                    device,
                    "setLabel",
                    id,
                    record([("label".into(), value)]),
                    true,
                )?;
            }
            return Ok(true);
        }
        Ok(false)
    }

    pub(super) fn gpu_call(
        &mut self,
        doc: &mut blitz_dom::BaseDocument,
        name: &str,
        this: Value,
        args: Vec<Value>,
        construct: bool,
    ) -> Result<Option<Value>> {
        if name == "GPU.__deliver" {
            return self.deliver(args).map(Some);
        }
        if name == "GPU.illegalConstructor" {
            return Err(Error::type_error("Illegal constructor"));
        }
        if name == "GPU.Canvas.getContext" {
            let kind = args
                .first()
                .map(|value| value.text())
                .unwrap_or_default()
                .to_ascii_lowercase();
            return self.canvas_context(doc, this, &kind).map(Some);
        }
        let result = match name {
            "GPU.requestAdapter" => {
                let options = payload_fields(args.first().unwrap_or(&Value::Undefined))?;
                Some(self.send_adapter(options)?)
            }
            "GPU.getPreferredCanvasFormat" => Some(Value::string("rgba8unorm")),
            "GPU.createImageBitmap" => {
                let source = args.first().cloned().unwrap_or(Value::Undefined);
                let options = args.get(1).cloned().unwrap_or_else(|| object("Record"));
                Some(self.create_image_bitmap(source, options)?)
            }
            "GPUImageBitmap.close" => {
                this.put_hidden("__closed", Value::Bool(true));
                this.put("data", Value::Null);
                Some(Value::Undefined)
            }
            "GPUObject.getBindGroupLayout" => {
                let device = safe_uint(&this.get("__deviceId"), 0, "GPUDevice")?;
                let id = self.allocate_object()?;
                self.command(
                    device,
                    "getBindGroupLayout",
                    id,
                    record([
                        ("pipeline".into(), Value::number(gpu_id(&this)? as f64)),
                        ("kind".into(), Value::string("render")),
                        (
                            "index".into(),
                            args.first().cloned().unwrap_or(Value::Number(0.0)),
                        ),
                    ]),
                    true,
                )?;
                let layout = object_with_id("GPUBindGroupLayout", id);
                layout.put_hidden("__deviceId", Value::number(device as f64));
                Some(layout)
            }
            "GPUAdapter.requestDevice" => {
                let adapter = gpu_id(&this)?;
                let options = payload_fields(args.first().unwrap_or(&Value::Undefined))?;
                Some(self.send_device(adapter, options)?)
            }
            "GPUFeatures.has" => {
                Some(Value::Bool(this.items().iter().any(|item| {
                    item.text() == args.first().map(Value::text).unwrap_or_default()
                })))
            }
            "GPUFeatures.keys" => Some(Value::array(this.items())),
            "GPUDevice.createBuffer"
            | "GPUDevice.createTexture"
            | "GPUDevice.createSampler"
            | "GPUDevice.createShaderModule"
            | "GPUDevice.createBindGroupLayout"
            | "GPUDevice.createPipelineLayout"
            | "GPUDevice.createBindGroup"
            | "GPUDevice.createRenderPipeline" => {
                let device = gpu_id(&this)?;
                let descriptor = payload_fields(args.first().unwrap_or(&Value::Undefined))?;
                let kind = match name {
                    "GPUDevice.createBuffer" => "GPUBuffer",
                    "GPUDevice.createTexture" => "GPUTexture",
                    "GPUDevice.createSampler" => "GPUSampler",
                    "GPUDevice.createShaderModule" => "GPUShaderModule",
                    "GPUDevice.createBindGroupLayout" => "GPUBindGroupLayout",
                    "GPUDevice.createPipelineLayout" => "GPUPipelineLayout",
                    "GPUDevice.createBindGroup" => "GPUBindGroup",
                    _ => "GPURenderPipeline",
                };
                Some(self.make_resource(kind, device, descriptor)?)
            }
            "GPUDevice.createCommandEncoder" => Some(self.make_resource(
                "GPUCommandEncoder",
                gpu_id(&this)?,
                payload_fields(args.first().unwrap_or(&Value::Undefined))?,
            )?),
            "GPUCommandEncoder.beginRenderPass" => {
                let device = safe_uint(&this.get("__deviceId"), 0, "GPUDevice")?;
                let input = payload_fields(args.first().unwrap_or(&Value::Undefined))?;
                let descriptor = record([
                    (
                        "label".into(),
                        if matches!(input.get("label"), Value::Undefined) {
                            Value::string("")
                        } else {
                            input.get("label")
                        },
                    ),
                    (
                        "colorAttachments".into(),
                        Value::array(
                            input
                                .get("colorAttachments")
                                .items()
                                .into_iter()
                                .map(|attachment| {
                                    if matches!(attachment, Value::Null) {
                                        return Ok(Value::Null);
                                    }
                                    let color =
                                        field_or(&attachment, "clearValue", object("Record"));
                                    Ok(record([
                                        (
                                            "view".into(),
                                            Value::number(gpu_id(&attachment.get("view"))? as f64),
                                        ),
                                        (
                                            "resolveTarget".into(),
                                            if matches!(
                                                attachment.get("resolveTarget"),
                                                Value::Undefined | Value::Null
                                            ) {
                                                Value::Null
                                            } else {
                                                Value::number(gpu_id(
                                                    &attachment.get("resolveTarget"),
                                                )?
                                                    as f64)
                                            },
                                        ),
                                        (
                                            "depthSlice".into(),
                                            field_or(&attachment, "depthSlice", Value::Null),
                                        ),
                                        (
                                            "clearValue".into(),
                                            if color.kind() == "Array" {
                                                color
                                            } else {
                                                Value::array(
                                                    ["r", "g", "b", "a"]
                                                        .into_iter()
                                                        .map(|key| {
                                                            field_or(
                                                                &color,
                                                                key,
                                                                Value::Number(0.0),
                                                            )
                                                        })
                                                        .collect(),
                                                )
                                            },
                                        ),
                                        (
                                            "loadOp".into(),
                                            field_or(&attachment, "loadOp", Value::string("load")),
                                        ),
                                        (
                                            "storeOp".into(),
                                            field_or(
                                                &attachment,
                                                "storeOp",
                                                Value::string("store"),
                                            ),
                                        ),
                                    ]))
                                })
                                .collect::<Result<Vec<_>>>()?,
                        ),
                    ),
                    (
                        "depthStencilAttachment".into(),
                        field_or(&input, "depthStencilAttachment", Value::Null),
                    ),
                    (
                        "occlusionQuerySet".into(),
                        field_or(&input, "occlusionQuerySet", Value::Null),
                    ),
                    ("commands".into(), Value::array(Vec::new())),
                ]);
                let pass = object_with_id("GPURenderPassEncoder", self.allocate_object()?);
                pass.put_hidden("__deviceId", Value::number(device as f64));
                pass.put_hidden("__encoder", this.clone());
                pass.put_hidden("__commands", Value::array(Vec::new()));
                methods(
                    &pass,
                    &[
                        ("setPipeline", "GPURenderPass.setPipeline"),
                        ("setBindGroup", "GPURenderPass.setBindGroup"),
                        ("setVertexBuffer", "GPURenderPass.setVertexBuffer"),
                        ("setIndexBuffer", "GPURenderPass.setIndexBuffer"),
                        ("draw", "GPURenderPass.draw"),
                        ("drawIndexed", "GPURenderPass.drawIndexed"),
                        ("end", "GPURenderPass.end"),
                    ],
                );
                let mut operations = this.get("__operations").items();
                operations.push(record([
                    ("op".into(), Value::string("render")),
                    ("args".into(), descriptor),
                ]));
                this.put_hidden("__operations", Value::array(operations));
                pass.put(
                    "__operationIndex",
                    Value::number((this.get("__operations").items().len() - 1) as f64),
                );
                Some(pass)
            }
            "GPUCommandEncoder.copyBufferToTexture"
            | "GPUCommandEncoder.copyTextureToBuffer"
            | "GPUCommandEncoder.copyTextureToTexture" => {
                let op = name.trim_start_matches("GPUCommandEncoder.");
                let source = args.first().cloned().unwrap_or(Value::Undefined);
                let destination = args.get(1).cloned().unwrap_or(Value::Undefined);
                let operation = record([
                    ("op".into(), Value::string(op)),
                    (
                        "args".into(),
                        record([
                            (
                                "source".into(),
                                if op == "copyBufferToTexture" {
                                    buffer_copy(&source)?
                                } else {
                                    texture_copy(&source)?
                                },
                            ),
                            (
                                "destination".into(),
                                if op == "copyTextureToBuffer" {
                                    buffer_copy(&destination)?
                                } else {
                                    texture_copy(&destination)?
                                },
                            ),
                            (
                                "size".into(),
                                extent(
                                    args.get(2).unwrap_or(&Value::Undefined),
                                    [1.0, 1.0, 1.0],
                                    ["width", "height", "depthOrArrayLayers"],
                                ),
                            ),
                        ]),
                    ),
                ]);
                let mut operations = this.get("__operations").items();
                if operations.len() >= GPU_COMMAND_LIMIT {
                    return Err(Error::type_error("WebGPU command limit reached"));
                }
                operations.push(operation);
                this.put_hidden("__operations", Value::array(operations));
                Some(Value::Undefined)
            }
            "GPUCommandEncoder.finish" => {
                let device = safe_uint(&this.get("__deviceId"), 0, "GPUDevice")?;
                let id = self.allocate_object()?;
                let command_buffer = object_with_id("GPUCommandBuffer", id);
                let args = record([
                    ("encoder".into(), Value::number(gpu_id(&this)? as f64)),
                    (
                        "label".into(),
                        args.first()
                            .map(|value| value.get("label"))
                            .unwrap_or(Value::string("")),
                    ),
                    ("operations".into(), this.get("__operations")),
                ]);
                self.command(device, "finish", id, args, true)?;
                Some(command_buffer)
            }
            "GPURenderPass.setPipeline"
            | "GPURenderPass.setBindGroup"
            | "GPURenderPass.setVertexBuffer"
            | "GPURenderPass.setIndexBuffer"
            | "GPURenderPass.draw"
            | "GPURenderPass.drawIndexed" => {
                let op = match name {
                    "GPURenderPass.setPipeline" => "pipeline",
                    "GPURenderPass.setBindGroup" => "bindGroup",
                    "GPURenderPass.setVertexBuffer" => "vertexBuffer",
                    "GPURenderPass.setIndexBuffer" => "indexBuffer",
                    "GPURenderPass.draw" => "draw",
                    _ => "drawIndexed",
                };
                let values = args
                    .iter()
                    .map(|value| {
                        if value.kind().starts_with("GPU") {
                            Ok(Value::number(gpu_id(value)? as f64))
                        } else {
                            Ok(value.clone())
                        }
                    })
                    .collect::<Result<Vec<_>>>()?;
                let mut commands = this.get("__commands").items();
                if commands.len() >= GPU_COMMAND_LIMIT {
                    return Err(Error::type_error("WebGPU command limit reached"));
                }
                let command_args = match op {
                    "pipeline" => Value::number(gpu_id(
                        args.first()
                            .ok_or_else(|| Error::type_error("pipeline is required"))?,
                    )? as f64),
                    "bindGroup" => record([
                        (
                            "index".into(),
                            values.first().cloned().unwrap_or(Value::Number(0.0)),
                        ),
                        (
                            "group".into(),
                            values.get(1).cloned().unwrap_or(Value::Null),
                        ),
                        (
                            "offsets".into(),
                            values
                                .get(2)
                                .cloned()
                                .unwrap_or_else(|| Value::array(Vec::new())),
                        ),
                    ]),
                    "vertexBuffer" => record([
                        (
                            "slot".into(),
                            values.first().cloned().unwrap_or(Value::Number(0.0)),
                        ),
                        (
                            "buffer".into(),
                            Value::number(gpu_id(
                                args.get(1)
                                    .ok_or_else(|| Error::type_error("buffer is required"))?,
                            )? as f64),
                        ),
                        (
                            "offset".into(),
                            values.get(2).cloned().unwrap_or(Value::Number(0.0)),
                        ),
                        ("size".into(), values.get(3).cloned().unwrap_or(Value::Null)),
                    ]),
                    "indexBuffer" => record([
                        (
                            "buffer".into(),
                            Value::number(gpu_id(
                                args.first()
                                    .ok_or_else(|| Error::type_error("buffer is required"))?,
                            )? as f64),
                        ),
                        (
                            "format".into(),
                            values.get(1).cloned().unwrap_or(Value::string("uint16")),
                        ),
                        (
                            "offset".into(),
                            values.get(2).cloned().unwrap_or(Value::Number(0.0)),
                        ),
                        ("size".into(), values.get(3).cloned().unwrap_or(Value::Null)),
                    ]),
                    "draw" => record([
                        (
                            "count".into(),
                            values.first().cloned().unwrap_or(Value::Number(0.0)),
                        ),
                        (
                            "instances".into(),
                            values.get(1).cloned().unwrap_or(Value::Number(1.0)),
                        ),
                        (
                            "first".into(),
                            values.get(2).cloned().unwrap_or(Value::Number(0.0)),
                        ),
                        (
                            "firstInstance".into(),
                            values.get(3).cloned().unwrap_or(Value::Number(0.0)),
                        ),
                    ]),
                    _ => record([
                        (
                            "count".into(),
                            values.first().cloned().unwrap_or(Value::Number(0.0)),
                        ),
                        (
                            "instances".into(),
                            values.get(1).cloned().unwrap_or(Value::Number(1.0)),
                        ),
                        (
                            "first".into(),
                            values.get(2).cloned().unwrap_or(Value::Number(0.0)),
                        ),
                        (
                            "baseVertex".into(),
                            values.get(3).cloned().unwrap_or(Value::Number(0.0)),
                        ),
                        (
                            "firstInstance".into(),
                            values.get(4).cloned().unwrap_or(Value::Number(0.0)),
                        ),
                    ]),
                };
                commands.push(record([
                    ("op".into(), Value::string(op)),
                    ("args".into(), command_args),
                ]));
                this.put_hidden("__commands", Value::array(commands));
                Some(Value::Undefined)
            }
            "GPURenderPass.end" => {
                let encoder = this.get("__encoder");
                let index = safe_uint(&this.get("__operationIndex"), 0, "render pass")? as usize;
                let mut operations = encoder.get("__operations").items();
                if let Some(operation) = operations.get_mut(index) {
                    let args = operation.get("args");
                    args.put("commands", this.get("__commands"));
                    operation.put("args", args);
                }
                encoder.put_hidden("__operations", Value::array(operations));
                Some(Value::Undefined)
            }
            "GPUBuffer.mapAsync" => {
                let handle = gpu_id(&this)?;
                let device = safe_uint(&this.get("__deviceId"), 0, "GPUDevice")?;
                let result = self.command(
                    device,
                    "mapRead",
                    handle,
                    record([
                        (
                            "mode".into(),
                            args.first().cloned().unwrap_or(Value::Number(1.0)),
                        ),
                        (
                            "offset".into(),
                            args.get(1).cloned().unwrap_or(Value::Number(0.0)),
                        ),
                        (
                            "size".into(),
                            args.get(2).cloned().unwrap_or(this.get("size")),
                        ),
                    ]),
                    false,
                )?;
                result.put_hidden("__gpuMapTarget", this.clone());
                this.put_hidden("__mapState", Value::string("pending"));
                Some(result)
            }
            "GPUBuffer.getMappedRange" => {
                if this.get("__mapState").text() != "mapped" {
                    return Err(Error::type_error("GPUBuffer is not mapped"));
                }
                let mapped = this.get("__mapped");
                let offset = safe_uint(
                    args.first().unwrap_or(&Value::Undefined),
                    0,
                    "mapped range offset",
                )?;
                let size = safe_uint(
                    args.get(1).unwrap_or(&Value::Undefined),
                    number_len(&mapped) as u64,
                    "mapped range size",
                )?;
                let bytes = bytes_from_value(&mapped)?;
                let end = offset
                    .checked_add(size)
                    .filter(|end| *end <= bytes.len() as u64)
                    .ok_or_else(|| Error::type_error("mapped range is out of bounds"))?;
                let output = object("ArrayBuffer");
                if let Some(storage) = output.as_object() {
                    storage.borrow_mut().payload = Some(Rc::new(HostBuffer::new(
                        bytes[offset as usize..end as usize].to_vec(),
                    )));
                }
                Some(output)
            }
            "GPUBuffer.unmap" => {
                let device = safe_uint(&this.get("__deviceId"), 0, "GPUDevice")?;
                this.put_hidden("__mapState", Value::string("unmapped"));
                this.put_hidden("__mapped", Value::Undefined);
                Some(self.command(device, "unmap", gpu_id(&this)?, object("Record"), true)?)
            }
            "GPUBuffer.destroy" => {
                let device = safe_uint(&this.get("__deviceId"), 0, "GPUDevice")?;
                Some(self.command(
                    device,
                    "destroyBuffer",
                    gpu_id(&this)?,
                    object("Record"),
                    true,
                )?)
            }
            "GPUTexture.createView" => {
                let device = safe_uint(&this.get("__deviceId"), 0, "GPUDevice")?;
                let descriptor = payload_fields(args.first().unwrap_or(&Value::Undefined))?;
                let id = self.allocate_object()?;
                let view = object_with_id("GPUTextureView", id);
                view.put_hidden("__deviceId", Value::number(device as f64));
                view.put_hidden("__textureId", Value::number(gpu_id(&this)? as f64));
                self.command(
                    device,
                    "createView",
                    id,
                    record([
                        ("texture".into(), Value::number(gpu_id(&this)? as f64)),
                        ("descriptor".into(), descriptor),
                    ]),
                    true,
                )?;
                Some(view)
            }
            "GPUTexture.destroy" => {
                let device = safe_uint(&this.get("__deviceId"), 0, "GPUDevice")?;
                Some(self.command(
                    device,
                    "destroyTexture",
                    gpu_id(&this)?,
                    object("Record"),
                    true,
                )?)
            }
            "GPUCanvasContext.configure" => {
                let device = handle_value(
                    &args
                        .first()
                        .map(|arg| arg.get("device"))
                        .unwrap_or(Value::Undefined),
                )?;
                let input = payload_fields(args.first().unwrap_or(&Value::Undefined))?;
                let canvas = this.get("canvas");
                let surface = self.canvas_for(doc, &canvas)?;
                let width = u64::from(surface.width);
                let height = u64::from(surface.height);
                let generation = safe_uint(&this.get("__generation"), 0, "canvas generation")? + 1;
                let format = if matches!(input.get("format"), Value::Undefined) {
                    Value::string("rgba8unorm")
                } else {
                    input.get("format")
                };
                let usage = if matches!(input.get("usage"), Value::Undefined) {
                    Value::number(16.0)
                } else {
                    input.get("usage")
                };
                let alpha = if matches!(input.get("alphaMode"), Value::Undefined) {
                    Value::string("premultiplied")
                } else {
                    input.get("alphaMode")
                };
                let views = if matches!(input.get("viewFormats"), Value::Undefined) {
                    Value::array(Vec::new())
                } else {
                    input.get("viewFormats")
                };
                let descriptor = record([
                    ("generation".into(), Value::number(generation as f64)),
                    ("format".into(), format),
                    ("usage".into(), usage),
                    ("alphaMode".into(), alpha),
                    ("viewFormats".into(), views),
                    ("width".into(), Value::number(width as f64)),
                    ("height".into(), Value::number(height as f64)),
                ]);
                self.command(device, "configure", gpu_id(&this)?, descriptor, true)?;
                this.put_hidden("__generation", Value::number(generation as f64));
                this.put_hidden("__configuration", input);
                this.put_hidden("__deviceId", Value::number(device as f64));
                if let Some(node) = canvas.node_id().and_then(|node| doc.get_node(node))
                    && let Some(id) = node.attr(blitz_dom::local_name!("id"))
                {
                    self.command(
                        device,
                        "setCanvasId",
                        gpu_id(&this)?,
                        Value::string(format!("canvas:#{id}")),
                        true,
                    )?;
                }
                Some(Value::Undefined)
            }
            "GPUCanvasContext.unconfigure" => {
                let device = safe_uint(&this.get("__deviceId"), 0, "GPUDevice")?;
                self.command(
                    device,
                    "unconfigure",
                    gpu_id(&this)?,
                    object("Record"),
                    true,
                )?;
                this.put_hidden("__configuration", Value::Null);
                Some(Value::Undefined)
            }
            "GPUCanvasContext.getConfiguration" => Some(this.get("__configuration")),
            "GPUCanvasContext.getCurrentTexture" => {
                let device = safe_uint(&this.get("__deviceId"), 0, "GPUDevice")?;
                let canvas = this.get("canvas");
                let surface = self.canvas_for(doc, &canvas)?;
                let width = u64::from(surface.width);
                let height = u64::from(surface.height);
                let texture = object_with_id("GPUTexture", self.allocate_object()?);
                texture.put_hidden("__deviceId", Value::number(device as f64));
                texture.put_hidden("__canvasId", Value::number(gpu_id(&this)? as f64));
                texture.put("width", Value::number(width as f64));
                texture.put("height", Value::number(height as f64));
                methods(
                    &texture,
                    &[
                        ("createView", "GPUTexture.createView"),
                        ("destroy", "GPUTexture.destroy"),
                    ],
                );
                self.command(
                    device,
                    "currentTexture",
                    gpu_id(&texture)?,
                    record([
                        ("canvas".into(), Value::number(gpu_id(&this)? as f64)),
                        ("generation".into(), this.get("__generation")),
                        ("width".into(), Value::number(width as f64)),
                        ("height".into(), Value::number(height as f64)),
                    ]),
                    true,
                )?;
                this.put_hidden("__currentTexture", texture.clone());
                Some(texture)
            }
            "GPUQueue.writeBuffer" => {
                let queue_device = this.get("__gpuDevice");
                let device = gpu_id(&queue_device)?;
                let buffer = args.first().ok_or_else(|| {
                    Error::type_error("GPUQueue.writeBuffer requires a GPUBuffer")
                })?;
                let data = args
                    .get(2)
                    .or_else(|| args.get(1))
                    .ok_or_else(|| Error::type_error("GPUQueue.writeBuffer requires data"))?;
                let offset =
                    safe_uint(args.get(1).unwrap_or(&Value::Undefined), 0, "buffer offset")?;
                let token = self.upload(data)?;
                let args = record([
                    ("offset".into(), Value::number(offset as f64)),
                    ("data".into(), Value::number(token as f64)),
                ]);
                let command = record([
                    ("op".into(), Value::string("writeBuffer")),
                    ("handle".into(), Value::number(gpu_id(buffer)? as f64)),
                    ("args".into(), args),
                ]);
                Some(self.submit("command", device, command, true, vec![token])?)
            }
            "GPUQueue.writeTexture" => {
                let device = gpu_id(&this.get("__gpuDevice"))?;
                let destination = args.first().cloned().unwrap_or(Value::Undefined);
                let data = args.get(1).cloned().unwrap_or(Value::Undefined);
                let layout = args.get(2).cloned().unwrap_or_else(|| object("Record"));
                let size = args
                    .get(3)
                    .cloned()
                    .unwrap_or_else(|| Value::array(Vec::new()));
                let token = self.upload(&data)?;
                let texture =
                    safe_uint(&destination.get("texture").get("__gpuId"), 0, "GPUTexture")?;
                let options = record([
                    ("texture".into(), Value::number(texture as f64)),
                    (
                        "mipLevel".into(),
                        field_or(&destination, "mipLevel", Value::Number(0.0)),
                    ),
                    (
                        "origin".into(),
                        extent(&destination.get("origin"), [0.0, 0.0, 0.0], ["x", "y", "z"]),
                    ),
                    (
                        "aspect".into(),
                        field_or(&destination, "aspect", Value::string("all")),
                    ),
                    ("data".into(), Value::number(token as f64)),
                    (
                        "offset".into(),
                        field_or(&layout, "offset", Value::Number(0.0)),
                    ),
                    (
                        "bytesPerRow".into(),
                        field_or(&layout, "bytesPerRow", Value::Null),
                    ),
                    (
                        "rowsPerImage".into(),
                        field_or(&layout, "rowsPerImage", Value::Null),
                    ),
                    (
                        "size".into(),
                        extent(
                            &size,
                            [1.0, 1.0, 1.0],
                            ["width", "height", "depthOrArrayLayers"],
                        ),
                    ),
                ]);
                let command = record([
                    ("op".into(), Value::string("writeTexture")),
                    ("handle".into(), Value::number(0.0)),
                    ("args".into(), options),
                ]);
                Some(self.submit("command", device, command, true, vec![token])?)
            }
            "GPUQueue.submit" => {
                let device = gpu_id(&this.get("__gpuDevice"))?;
                let handles = args
                    .first()
                    .map(Value::items)
                    .unwrap_or_default()
                    .iter()
                    .map(gpu_id)
                    .collect::<Result<Vec<_>>>()?;
                let handles = Value::array(
                    handles
                        .into_iter()
                        .map(|handle| Value::number(handle as f64))
                        .collect(),
                );
                let result = self.command(device, "submit", 0, handles, true)?;
                let contexts = self
                    .gpu_realm()?
                    .borrow()
                    .canvases
                    .values()
                    .cloned()
                    .collect::<Vec<_>>();
                for context in contexts {
                    let current_texture = context.get("__currentTexture");
                    if safe_uint(&context.get("__deviceId"), 0, "GPUDevice")? == device
                        && !matches!(
                            context.get("__configuration"),
                            Value::Undefined | Value::Null
                        )
                        && current_texture.kind() == "GPUTexture"
                    {
                        self.command(
                            device,
                            "present",
                            gpu_id(&current_texture)?,
                            record([
                                ("canvas".into(), Value::number(gpu_id(&context)? as f64)),
                                ("generation".into(), context.get("__generation")),
                            ]),
                            true,
                        )?;
                        context.put_hidden("__currentTexture", Value::Undefined);
                    }
                }
                Some(result)
            }
            "GPUQueue.onSubmittedWorkDone" => {
                let device = gpu_id(&this.get("__gpuDevice"))?;
                Some(self.command(device, "onSubmittedWorkDone", 0, object("Record"), false)?)
            }
            "GPUQueue.copyExternalImageToTexture" => {
                let device = gpu_id(&this.get("__gpuDevice"))?;
                let source = args.first().cloned().unwrap_or(Value::Undefined);
                let destination = args.get(1).cloned().unwrap_or(Value::Undefined);
                let size = args.get(2).cloned().unwrap_or(Value::Undefined);
                let image = source.get("source");
                if image.kind() != "ImageBitmap" || image.get("__closed").truthy() {
                    return Err(Error::type_error(
                        "external copy requires a live ImageBitmap",
                    ));
                }
                let descriptor = record([
                    (
                        "source".into(),
                        record([
                            ("kind".into(), Value::string("bitmap")),
                            ("handle".into(), image.get("__bitmapId")),
                        ]),
                    ),
                    (
                        "sourceOrigin".into(),
                        Value::array(
                            extent(&source.get("origin"), [0.0, 0.0, 0.0], ["x", "y", "z"])
                                .items()
                                .into_iter()
                                .take(2)
                                .collect(),
                        ),
                    ),
                    (
                        "flipY".into(),
                        field_or(&source, "flipY", Value::Bool(false)),
                    ),
                    (
                        "texture".into(),
                        Value::number(gpu_id(&destination.get("texture"))? as f64),
                    ),
                    (
                        "mipLevel".into(),
                        field_or(&destination, "mipLevel", Value::Number(0.0)),
                    ),
                    (
                        "origin".into(),
                        extent(&destination.get("origin"), [0.0, 0.0, 0.0], ["x", "y", "z"]),
                    ),
                    (
                        "aspect".into(),
                        field_or(&destination, "aspect", Value::string("all")),
                    ),
                    (
                        "premultipliedAlpha".into(),
                        field_or(&destination, "premultipliedAlpha", Value::Bool(false)),
                    ),
                    (
                        "colorSpace".into(),
                        field_or(&destination, "colorSpace", Value::string("srgb")),
                    ),
                    (
                        "size".into(),
                        extent(
                            &size,
                            [1.0, 1.0, 1.0],
                            ["width", "height", "depthOrArrayLayers"],
                        ),
                    ),
                ]);
                Some(self.command(device, "copyExternalImageToTexture", 0, descriptor, true)?)
            }
            "GPUDevice.destroy" => {
                let device = gpu_id(&this)?;
                Some(self.command(device, "destroy", 0, object("Record"), true)?)
            }
            "GPUDevice.pushErrorScope" => {
                let device = gpu_id(&this)?;
                Some(self.command(
                    device,
                    "pushErrorScope",
                    0,
                    record([(
                        "filter".into(),
                        args.first().cloned().unwrap_or(Value::Undefined),
                    )]),
                    true,
                )?)
            }
            "GPUDevice.popErrorScope" => {
                let device = gpu_id(&this)?;
                Some(self.command(device, "popErrorScope", 0, object("Record"), false)?)
            }
            "Canvas2D.submit" => Some(self.canvas_flush(&this, "paint", None)?),
            "Canvas2D.clearRect"
            | "Canvas2D.fillRect"
            | "Canvas2D.strokeRect"
            | "Canvas2D.beginPath"
            | "Canvas2D.closePath"
            | "Canvas2D.moveTo"
            | "Canvas2D.lineTo"
            | "Canvas2D.rect"
            | "Canvas2D.arc"
            | "Canvas2D.fill"
            | "Canvas2D.stroke"
            | "Canvas2D.fillText"
            | "Canvas2D.strokeText" => {
                let op = name.trim_start_matches("Canvas2D.");
                Some(self.canvas_record(&this, op, &args)?)
            }
            "Canvas2D.getImageData" => {
                let n = args.iter().take(4).cloned().collect::<Vec<_>>();
                if n.len() != 4 {
                    return Err(Error::type_error("getImageData requires four coordinates"));
                }
                let pixels = self.canvas_flush(&this, "read", Some(n.clone()))?;
                let image = object("ImageData");
                image.put("width", n.get(2).cloned().unwrap_or(Value::Number(0.0)));
                image.put("height", n.get(3).cloned().unwrap_or(Value::Number(0.0)));
                image.put("data", pixels);
                Some(image)
            }
            "Canvas2D.drawImage" => {
                let source = args
                    .first()
                    .cloned()
                    .ok_or_else(|| Error::type_error("drawImage requires a source"))?;
                let descriptor = if source.kind() == "ImageBitmap" {
                    if source.get("__closed").truthy() {
                        return Err(Error::type_error("ImageBitmap has been closed"));
                    }
                    record([
                        ("kind".into(), Value::string("bitmap")),
                        ("handle".into(), source.get("__bitmapId")),
                    ])
                } else if source.node_id().is_some() {
                    let surface = self.canvas_for(doc, &source)?;
                    let context = self
                        .gpu_realm()?
                        .borrow()
                        .canvases
                        .get(&surface.id)
                        .cloned()
                        .ok_or_else(|| Error::type_error("source canvas has no context"))?;
                    record([
                        ("kind".into(), Value::string("canvas")),
                        ("canvas".into(), Value::number(surface.id as f64)),
                        ("generation".into(), context.get("__generation")),
                        ("width".into(), Value::number(surface.width as f64)),
                        ("height".into(), Value::number(surface.height as f64)),
                    ])
                } else {
                    return Err(Error::type_error(
                        "drawImage source is not a supported native image",
                    ));
                };
                let source_id = self.allocate_object()?;
                let serial = self.next_fire_id()?;
                let request = record([
                    ("action".into(), Value::string("source")),
                    ("id".into(), Value::number(source_id as f64)),
                    ("serial".into(), Value::number(serial as f64)),
                    ("source".into(), descriptor),
                ]);
                let dimensions = host_to_value(
                    self.host
                        .invoke_intrinsic(
                            "__chuzzCanvas2D",
                            &[values_to_host(&request)?, HostValue::Array(Vec::new())],
                        )
                        .map_err(|error| Error::type_error(error.to_string()))?,
                );
                let dimensions = dimensions.items();
                let width = dimensions.first().cloned().unwrap_or(Value::Number(0.0));
                let height = dimensions.get(1).cloned().unwrap_or(Value::Number(0.0));
                let n = match args.len() {
                    3 => vec![
                        Value::Number(0.0),
                        Value::Number(0.0),
                        width.clone(),
                        height.clone(),
                        args[1].clone(),
                        args[2].clone(),
                        width.clone(),
                        height.clone(),
                    ],
                    5 => vec![
                        Value::Number(0.0),
                        Value::Number(0.0),
                        width.clone(),
                        height.clone(),
                        args[1].clone(),
                        args[2].clone(),
                        args[3].clone(),
                        args[4].clone(),
                    ],
                    9 => args[1..].to_vec(),
                    _ => {
                        return Err(Error::type_error(
                            "drawImage requires three, five, or nine arguments",
                        ));
                    }
                };
                let image = record([
                    ("index".into(), Value::Number(0.0)),
                    ("width".into(), width.clone()),
                    ("height".into(), height.clone()),
                    ("offset".into(), Value::Number(0.0)),
                    (
                        "length".into(),
                        Value::Number(width.number_value() * height.number_value() * 4.0),
                    ),
                    ("native".into(), Value::number(source_id as f64)),
                ]);
                let command = record([
                    ("op".into(), Value::string("drawImage")),
                    ("n".into(), Value::array(n)),
                    ("state".into(), this.get("__drawState")),
                    (
                        "brush".into(),
                        record([
                            ("kind".into(), Value::string("image")),
                            ("image".into(), image),
                        ]),
                    ),
                ]);
                let mut commands = this.get("__commands").items();
                commands.push(command);
                this.put_hidden("__commands", Value::array(commands));
                Some(Value::Undefined)
            }
            "Canvas2D.putImageData" => {
                let op = name.trim_start_matches("Canvas2D.");
                Some(self.canvas_record(&this, op, &args)?)
            }
            _ => None,
        };
        let _ = construct;
        Ok(result)
    }
}

fn number_len(value: &Value) -> usize {
    let Some(object) = value.as_object() else {
        return 0;
    };
    let borrowed = object.borrow();
    let Some(payload) = borrowed.payload.as_ref() else {
        return borrowed.indexed.len();
    };
    if let Some(buffer) = payload.downcast_ref::<Rc<RefCell<HostBuffer>>>() {
        return buffer.borrow().as_bytes().map_or(0, |bytes| bytes.len());
    }
    if let Some(buffer) = payload.downcast_ref::<RefCell<HostBuffer>>() {
        return buffer.borrow().as_bytes().map_or(0, |bytes| bytes.len());
    }
    if let Some(buffer) = payload.downcast_ref::<HostBuffer>() {
        return buffer.as_bytes().map_or(0, |bytes| bytes.len());
    }
    borrowed.indexed.len()
}

fn bytes_from_value(value: &Value) -> Result<Vec<u8>> {
    if let Some(object) = value.as_object() {
        let borrowed = object.borrow();
        if let Some(payload) = borrowed.payload.as_ref() {
            if let Some(buffer) = payload.downcast_ref::<Rc<RefCell<HostBuffer>>>() {
                return buffer
                    .borrow()
                    .as_bytes()
                    .map(|bytes| bytes.to_vec())
                    .map_err(|_| Error::type_error("detached WebGPU bytes"));
            }
            if let Some(buffer) = payload.downcast_ref::<RefCell<HostBuffer>>() {
                return buffer
                    .borrow()
                    .as_bytes()
                    .map(|bytes| bytes.to_vec())
                    .map_err(|_| Error::type_error("detached WebGPU bytes"));
            }
            if let Some(buffer) = payload.downcast_ref::<HostBuffer>() {
                return buffer
                    .as_bytes()
                    .map(|bytes| bytes.to_vec())
                    .map_err(|_| Error::type_error("detached WebGPU bytes"));
            }
        }
        if borrowed.indexed.len() > GPU_BYTE_LIMIT {
            return Err(Error::type_error("WebGPU transfer limit reached"));
        }
        let mut bytes = Vec::with_capacity(borrowed.indexed.len());
        for item in &borrowed.indexed {
            let value = item.number_value();
            if !value.is_finite() {
                return Err(Error::type_error("invalid WebGPU byte"));
            }
            bytes.push(value.trunc().clamp(0.0, 255.0) as u8);
        }
        return Ok(bytes);
    }
    Err(Error::type_error("expected an ArrayBuffer or typed array"))
}

fn host_to_value_clone(value: &HostValue) -> Value {
    match value {
        HostValue::Undefined => Value::Undefined,
        HostValue::Null => Value::Null,
        HostValue::Boolean(value) => Value::Bool(*value),
        HostValue::Number(value) => Value::Number(*value),
        HostValue::String(value) => Value::string(value.as_ref()),
        HostValue::Utf16(value) => Value::String(Rc::clone(value)),
        HostValue::Buffer(buffer) => {
            let result = object("Uint8Array");
            if let Some(storage) = result.as_object() {
                storage.borrow_mut().indexed = buffer.as_bytes().map_or_else(
                    |_| Vec::new(),
                    |bytes| {
                        bytes
                            .iter()
                            .map(|byte| Value::Number(f64::from(*byte)))
                            .collect()
                    },
                );
            }
            result
        }
        HostValue::Object(value) => {
            let result = object("HostObject");
            if let Some(storage) = result.as_object() {
                storage.borrow_mut().payload = Some(Rc::new(value.clone()));
            }
            result
        }
        HostValue::Array(values) => Value::array(values.iter().map(host_to_value_clone).collect()),
        HostValue::Record(values) => {
            let result = object("Record");
            for (key, value) in values {
                result.put(key, host_to_value_clone(value));
            }
            result
        }
    }
}

fn field_or(value: &Value, key: &str, default: Value) -> Value {
    let field = value.get(key);
    if matches!(field, Value::Undefined) {
        default
    } else {
        field
    }
}
fn extent(value: &Value, defaults: [f64; 3], keys: [&str; 3]) -> Value {
    let items = value.items();
    Value::array(
        keys.into_iter()
            .enumerate()
            .map(|(index, key)| {
                if value.kind() == "Array" {
                    items
                        .get(index)
                        .cloned()
                        .unwrap_or(Value::Number(defaults[index]))
                } else {
                    field_or(value, key, Value::Number(defaults[index]))
                }
            })
            .collect(),
    )
}
fn gpu_stage(value: &Value, fragment: bool) -> Result<Value> {
    let stage = record([
        (
            "module".into(),
            Value::number(gpu_id(&value.get("module"))? as f64),
        ),
        (
            "entryPoint".into(),
            field_or(value, "entryPoint", Value::Null),
        ),
        (
            "constants".into(),
            field_or(value, "constants", object("Record")),
        ),
    ]);
    if fragment {
        let targets = value
            .get("targets")
            .items()
            .into_iter()
            .map(|target| {
                if matches!(target, Value::Null) {
                    Value::Null
                } else {
                    record([
                        ("format".into(), target.get("format")),
                        ("blend".into(), field_or(&target, "blend", Value::Null)),
                        (
                            "writeMask".into(),
                            field_or(&target, "writeMask", Value::Number(15.0)),
                        ),
                    ])
                }
            })
            .collect();
        stage.put("targets", Value::array(targets));
    } else {
        stage.put(
            "buffers",
            field_or(value, "buffers", Value::array(Vec::new())),
        );
    }
    Ok(stage)
}
fn normalize_descriptor(kind: &str, input: &Value) -> Result<Value> {
    let label = field_or(input, "label", Value::string(""));
    let value = match kind {
        "GPUBuffer" => record([
            ("label".into(), label),
            ("size".into(), input.get("size")),
            ("usage".into(), input.get("usage")),
            (
                "mappedAtCreation".into(),
                field_or(input, "mappedAtCreation", Value::Bool(false)),
            ),
        ]),
        "GPUTexture" => record([
            ("label".into(), label),
            (
                "size".into(),
                extent(
                    &input.get("size"),
                    [1.0, 1.0, 1.0],
                    ["width", "height", "depthOrArrayLayers"],
                ),
            ),
            (
                "mipLevelCount".into(),
                field_or(input, "mipLevelCount", Value::Number(1.0)),
            ),
            (
                "sampleCount".into(),
                field_or(input, "sampleCount", Value::Number(1.0)),
            ),
            (
                "dimension".into(),
                field_or(input, "dimension", Value::string("2d")),
            ),
            ("format".into(), input.get("format")),
            ("usage".into(), input.get("usage")),
            (
                "viewFormats".into(),
                field_or(input, "viewFormats", Value::array(Vec::new())),
            ),
        ]),
        "GPUShaderModule" => record([("label".into(), label), ("code".into(), input.get("code"))]),
        "GPUCommandEncoder" => record([("label".into(), label)]),
        "GPURenderPipeline" => {
            let primitive = field_or(input, "primitive", object("Record"));
            let multisample = field_or(input, "multisample", object("Record"));
            let layout = input.get("layout");
            let layout = if matches!(layout, Value::Undefined) || layout.text() == "auto" {
                Value::Null
            } else {
                Value::number(gpu_id(&layout)? as f64)
            };
            let fragment = input.get("fragment");
            let fragment = if matches!(fragment, Value::Undefined | Value::Null) {
                Value::Null
            } else {
                gpu_stage(&fragment, true)?
            };
            record([
                ("label".into(), label),
                ("layout".into(), layout),
                ("vertex".into(), gpu_stage(&input.get("vertex"), false)?),
                ("fragment".into(), fragment),
                (
                    "primitive".into(),
                    record([
                        (
                            "topology".into(),
                            field_or(&primitive, "topology", Value::string("triangle-list")),
                        ),
                        (
                            "stripIndexFormat".into(),
                            field_or(&primitive, "stripIndexFormat", Value::Null),
                        ),
                        (
                            "frontFace".into(),
                            field_or(&primitive, "frontFace", Value::string("ccw")),
                        ),
                        (
                            "cullMode".into(),
                            field_or(&primitive, "cullMode", Value::Null),
                        ),
                        (
                            "unclippedDepth".into(),
                            field_or(&primitive, "unclippedDepth", Value::Bool(false)),
                        ),
                    ]),
                ),
                (
                    "depthStencil".into(),
                    field_or(input, "depthStencil", Value::Null),
                ),
                (
                    "multisample".into(),
                    record([
                        (
                            "count".into(),
                            field_or(&multisample, "count", Value::Number(1.0)),
                        ),
                        (
                            "mask".into(),
                            field_or(&multisample, "mask", Value::Number(4294967295.0)),
                        ),
                        (
                            "alphaToCoverageEnabled".into(),
                            field_or(&multisample, "alphaToCoverageEnabled", Value::Bool(false)),
                        ),
                    ]),
                ),
            ])
        }
        "GPUBindGroup" => {
            let entries = input
                .get("entries")
                .items()
                .into_iter()
                .map(|entry| {
                    let resource = entry.get("resource");
                    let binding = if resource.kind() == "GPUSampler" {
                        record([
                            ("kind".into(), Value::string("sampler")),
                            ("handle".into(), Value::number(gpu_id(&resource)? as f64)),
                        ])
                    } else if resource.kind() == "GPUTextureView" {
                        record([
                            ("kind".into(), Value::string("view")),
                            ("handle".into(), Value::number(gpu_id(&resource)? as f64)),
                        ])
                    } else {
                        record([
                            ("kind".into(), Value::string("buffer")),
                            (
                                "handle".into(),
                                Value::number(gpu_id(&resource.get("buffer"))? as f64),
                            ),
                            (
                                "offset".into(),
                                field_or(&resource, "offset", Value::Number(0.0)),
                            ),
                            ("size".into(), field_or(&resource, "size", Value::Null)),
                        ])
                    };
                    Ok(record([
                        ("binding".into(), entry.get("binding")),
                        ("resource".into(), binding),
                    ]))
                })
                .collect::<Result<Vec<_>>>()?;
            record([
                ("label".into(), label),
                (
                    "layout".into(),
                    Value::number(gpu_id(&input.get("layout"))? as f64),
                ),
                ("entries".into(), Value::array(entries)),
            ])
        }
        _ => input.clone(),
    };
    Ok(value)
}
fn texture_copy(value: &Value) -> Result<Value> {
    Ok(record([
        (
            "texture".into(),
            Value::number(gpu_id(&value.get("texture"))? as f64),
        ),
        (
            "mipLevel".into(),
            field_or(value, "mipLevel", Value::Number(0.0)),
        ),
        (
            "origin".into(),
            extent(&value.get("origin"), [0.0, 0.0, 0.0], ["x", "y", "z"]),
        ),
        (
            "aspect".into(),
            field_or(value, "aspect", Value::string("all")),
        ),
    ]))
}
fn buffer_copy(value: &Value) -> Result<Value> {
    Ok(record([
        (
            "buffer".into(),
            Value::number(gpu_id(&value.get("buffer"))? as f64),
        ),
        (
            "offset".into(),
            field_or(value, "offset", Value::Number(0.0)),
        ),
        (
            "bytesPerRow".into(),
            field_or(value, "bytesPerRow", Value::Null),
        ),
        (
            "rowsPerImage".into(),
            field_or(value, "rowsPerImage", Value::Null),
        ),
    ]))
}
