use crate::page_native_bridge::ChuzzBridge;
use crate::script_host::{HostError, HostValue};
use blitz_dom::BaseDocument;
use blitz_script::native::{Error, HostJob, Result, Value};
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

const MAX_MEDIA_OBJECTS: usize = 4096;
const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
type HeaderEntries = RefCell<BTreeMap<String, String>>;

#[derive(Default)]
struct CompletionQueue(RefCell<VecDeque<(u64, Value)>>);

#[derive(Default)]
struct MediaState {
    next_id: u64,
    pending: BTreeMap<u64, Value>,
    completions: Rc<CompletionQueue>,
}

#[derive(Clone)]
struct MediaObject {
    bytes: Option<Rc<[u8]>>,
}

fn number(value: &Value) -> u64 {
    let n = value.number_value();
    if n.is_finite() && n >= 0.0 && n.fract() == 0.0 && n <= 9_007_199_254_740_991.0 {
        n as u64
    } else {
        0
    }
}

fn field(value: &Value, name: &str) -> Value {
    value.get(name)
}

fn truth(value: &Value, fallback: bool) -> bool {
    if matches!(value, Value::Undefined) {
        fallback
    } else {
        value.truthy()
    }
}

fn native(name: &str) -> Value {
    Value::native(name)
}

fn set_method(object: &Value, name: &str, native_name: &str) {
    object.put_hidden(name, native(native_name));
}

fn response(
    bytes: Vec<u8>,
    status: u16,
    status_text: String,
    headers: Value,
    url: String,
) -> Value {
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Value::error(
            "QuotaExceededError",
            "response exceeds the media body limit",
        );
    }
    let body: Rc<[u8]> = Rc::from(bytes);
    let out = Value::object("Response");
    out.put("status", Value::number(status));
    out.put("statusText", Value::string(status_text));
    out.put("ok", Value::Bool((200..300).contains(&status)));
    out.put("url", Value::string(url));
    out.put("headers", headers);
    out.put("bodyUsed", Value::Bool(false));
    out.put_hidden("__media", Value::native("Media.Response"));
    if let Some(object) = out.as_object() {
        object.borrow_mut().payload = Some(Rc::new(MediaObject {
            bytes: Some(Rc::clone(&body)),
        }));
    }
    out.put_hidden("__bytesLength", Value::number(body.len() as f64));
    set_method(&out, "arrayBuffer", "Media.Response.arrayBuffer");
    set_method(&out, "text", "Media.Response.text");
    set_method(&out, "json", "Media.Response.json");
    set_method(&out, "clone", "Media.Response.clone");
    out
}

fn headers_from(value: &Value) -> Value {
    let out = Value::object("Headers");
    let mut entries = BTreeMap::<String, String>::new();
    if let Some(existing) = headers_payload(value) {
        entries = existing.borrow().clone();
    } else if value.kind() == "Array" {
        for pair in value.items() {
            let items = pair.items();
            if items.len() >= 2 {
                let key = items[0].text().to_ascii_lowercase();
                let entry = entries.entry(key).or_default();
                if !entry.is_empty() {
                    entry.push_str(", ");
                }
                entry.push_str(&items[1].text());
            }
        }
    } else if let Some(object) = value.as_object() {
        for (key, property) in &object.borrow().props {
            if property.enumerable {
                entries.insert(key.to_ascii_lowercase(), property.value.text());
            }
        }
    }
    if let Some(object) = out.as_object() {
        object.borrow_mut().payload = Some(Rc::new(RefCell::new(entries)));
    }
    set_method(&out, "get", "Media.Headers.get");
    set_method(&out, "has", "Media.Headers.has");
    set_method(&out, "set", "Media.Headers.set");
    set_method(&out, "append", "Media.Headers.append");
    set_method(&out, "delete", "Media.Headers.delete");
    out
}

fn request_record(url: String, init: &Value, headers: &Value, body: &Value) -> Result<Value> {
    let request = Value::object("Record");
    request.put("url", Value::string(url));
    request.put(
        "method",
        if matches!(field(init, "method"), Value::Undefined) {
            Value::string("GET")
        } else {
            field(init, "method")
        },
    );
    request.put("headers", Value::array(header_pairs(headers)));
    let body_value = if matches!(body, Value::Undefined | Value::Null) {
        Value::Null
    } else {
        Value::array(blob_bytes(body)?.into_iter().map(Value::number).collect())
    };
    request.put("body", body_value);
    Ok(request)
}

fn header_pairs(headers: &Value) -> Vec<Value> {
    headers_payload(headers)
        .map(|entries| {
            entries
                .borrow()
                .iter()
                .map(|(key, value)| Value::array(vec![Value::string(key), Value::string(value)]))
                .collect()
        })
        .unwrap_or_default()
}

fn headers_payload(value: &Value) -> Option<Rc<HeaderEntries>> {
    let payload = value
        .as_object()
        .and_then(|object| object.borrow().payload.as_ref().cloned())?;
    Rc::downcast::<HeaderEntries>(payload).ok()
}

impl ChuzzBridge {
    pub(super) fn media_pending(&self) -> bool {
        self.media_state().is_ok_and(|state| {
            !state.borrow().pending.is_empty() || !state.borrow().completions.0.borrow().is_empty()
        })
    }

    pub(super) fn media_task_count(&self) -> u64 {
        self.media_state()
            .map_or(0, |state| state.borrow().pending.len() as u64)
    }
    pub(super) fn install_media(&mut self, global: &Value) -> Result<()> {
        let state = Value::object("MediaState");
        if let Some(object) = state.as_object() {
            object.borrow_mut().payload = Some(Rc::new(RefCell::new(MediaState {
                next_id: 1,
                pending: BTreeMap::new(),
                completions: Rc::new(CompletionQueue::default()),
            })));
        }
        self.state.insert("media.native-state".into(), state);
        for (global_name, intrinsic) in [
            ("fetch", "Media.fetch"),
            ("WebSocket", "Media.WebSocket"),
            ("AudioContext", "Media.AudioContext"),
            ("webkitAudioContext", "Media.AudioContext"),
            ("MediaStream", "Media.MediaStream"),
            ("MediaStreamTrack", "Media.MediaStreamTrack"),
            ("RTCPeerConnection", "Media.RTCPeerConnection"),
            ("RTCSessionDescription", "Media.RTCSessionDescription"),
            ("RTCIceCandidate", "Media.RTCIceCandidate"),
            ("MediaSource", "Media.MediaSource"),
            ("SourceBuffer", "Media.SourceBuffer"),
        ] {
            global.put(global_name, native(intrinsic));
        }
        global.put("Headers", native("Media.Headers"));
        global.put("Response", native("Media.Response"));
        global.put("Request", native("Media.Request"));
        Ok(())
    }

    pub(super) fn media_get(
        &mut self,
        _doc: &mut BaseDocument,
        receiver: &Value,
        key: &str,
    ) -> Result<Option<Value>> {
        let kind = receiver.kind();
        let method = match (kind.as_str(), key) {
            ("Response", "arrayBuffer") => Some("Media.Response.arrayBuffer"),
            ("Response", "text") => Some("Media.Response.text"),
            ("Response", "json") => Some("Media.Response.json"),
            ("Response", "clone") => Some("Media.Response.clone"),
            ("Headers", "get") => Some("Media.Headers.get"),
            ("Headers", "has") => Some("Media.Headers.has"),
            ("Headers", "set") => Some("Media.Headers.set"),
            ("Headers", "append") => Some("Media.Headers.append"),
            ("Headers", "delete") => Some("Media.Headers.delete"),
            ("WebSocket", "send") => Some("Media.WebSocket.send"),
            ("WebSocket", "close") => Some("Media.WebSocket.close"),
            ("AudioContext", "resume") => Some("Media.AudioContext.resume"),
            ("AudioContext", "suspend") => Some("Media.AudioContext.suspend"),
            ("AudioContext", "close") => Some("Media.AudioContext.close"),
            ("AudioContext", "createOscillator") => Some("Media.AudioContext.createOscillator"),
            ("AudioContext", "createGain") => Some("Media.AudioContext.createGain"),
            ("AudioContext", "createAnalyser") => Some("Media.AudioContext.createAnalyser"),
            ("MediaSource", "addSourceBuffer") => Some("Media.MediaSource.addSourceBuffer"),
            ("SourceBuffer", "appendBuffer") => Some("Media.SourceBuffer.appendBuffer"),
            ("SourceBuffer", "remove") => Some("Media.SourceBuffer.remove"),
            ("SourceBuffer", "abort") => Some("Media.SourceBuffer.abort"),
            ("RTCPeerConnection", "createOffer") => Some("Media.RTCPeerConnection.createOffer"),
            ("RTCPeerConnection", "createAnswer") => Some("Media.RTCPeerConnection.createAnswer"),
            ("RTCPeerConnection", "setLocalDescription") => {
                Some("Media.RTCPeerConnection.setLocalDescription")
            }
            ("RTCPeerConnection", "setRemoteDescription") => {
                Some("Media.RTCPeerConnection.setRemoteDescription")
            }
            ("RTCPeerConnection", "addIceCandidate") => {
                Some("Media.RTCPeerConnection.addIceCandidate")
            }
            ("RTCPeerConnection", "addTrack") => Some("Media.RTCPeerConnection.addTrack"),
            ("RTCPeerConnection", "close") => Some("Media.RTCPeerConnection.close"),
            ("RTCPeerConnection", "getStats") => Some("Media.RTCPeerConnection.getStats"),
            _ => None,
        };
        if let Some(method) = method {
            return Ok(Some(native(method)));
        }
        if kind == "WebSocket" {
            return Ok(match key {
                "readyState" | "bufferedAmount" | "extensions" | "protocol" | "url" => {
                    Some(receiver.get(key))
                }
                "binaryType" => Some(receiver.get(key)),
                _ => None,
            });
        }
        Ok(None)
    }

    pub(super) fn media_set(
        &mut self,
        _doc: &mut BaseDocument,
        receiver: &Value,
        key: &str,
        value: Value,
    ) -> Result<bool> {
        if receiver.kind() == "WebSocket" && key == "binaryType" {
            if value.text() != "blob" && value.text() != "arraybuffer" {
                return Err(Error::type_error("invalid WebSocket binaryType"));
            }
            receiver.put(key, value);
            return Ok(true);
        }
        if receiver.kind() == "AudioContext" && key == "onstatechange" {
            receiver.put(key, value);
            return Ok(true);
        }
        Ok(false)
    }

    pub(super) fn media_call(
        &mut self,
        _doc: &mut BaseDocument,
        name: &str,
        this: Value,
        args: Vec<Value>,
        _construct: bool,
    ) -> Result<Option<Value>> {
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Undefined);
        let called = match name {
            "Media.fetch" => Some(self.fetch(arg(0), arg(1))?),
            "Media.Headers" => Some(headers_from(&arg(0))),
            "Media.Response" => {
                let status = number(&field(&arg(1), "status")).clamp(0, 599) as u16;
                Some(response(
                    blob_bytes(&arg(0))?,
                    status,
                    field(&arg(1), "statusText").text(),
                    headers_from(&field(&arg(1), "headers")),
                    String::new(),
                ))
            }
            "Media.Request" => Some(make_request(arg(0), arg(1))),
            "Media.Blob" => Some(make_blob(&args)?),
            "Media.WebSocket" => Some(self.websocket(arg(0), arg(1))?),
            "Media.AudioContext" => Some(self.audio_context(&args)?),
            "Media.MediaStream" => Some(
                self.call_json_host("__chuzzMediaControl", media_command("stream.create", &args))?,
            ),
            "Media.MediaStreamTrack" | "Media.MediaSource" | "Media.SourceBuffer" => {
                return Err(Error::new(
                    "NotSupportedError",
                    "this media object requires a native producer",
                ));
            }
            "Media.RTCPeerConnection" => Some(self.peer_connection(&args)?),
            "Media.RTCSessionDescription" => Some(copy_record("RTCSessionDescription", &arg(0))),
            "Media.RTCIceCandidate" => Some(copy_record("RTCIceCandidate", &arg(0))),
            "Media.Response.arrayBuffer" => {
                Some(response_bytes(&this).unwrap_or_else(|e| Value::promise(Err(e))))
            }
            "Media.Response.text" => {
                Some(response_text(&this).unwrap_or_else(|e| Value::promise(Err(e))))
            }
            "Media.Response.json" => {
                Some(response_json(&this).unwrap_or_else(|e| Value::promise(Err(e))))
            }
            "Media.Response.clone" => Some(clone_response(&this)?),
            "Media.Blob.arrayBuffer" => {
                Some(response_bytes(&this).unwrap_or_else(|e| Value::promise(Err(e))))
            }
            "Media.Blob.text" => {
                Some(response_text(&this).unwrap_or_else(|e| Value::promise(Err(e))))
            }
            "Media.Headers.get" => Some(
                headers_payload(&this)
                    .and_then(|entries| {
                        entries
                            .borrow()
                            .get(&arg(0).text().to_ascii_lowercase())
                            .cloned()
                    })
                    .map(Value::string)
                    .unwrap_or(Value::Null),
            ),
            "Media.Headers.has" => {
                Some(Value::Bool(headers_payload(&this).is_some_and(|entries| {
                    entries
                        .borrow()
                        .contains_key(&arg(0).text().to_ascii_lowercase())
                })))
            }
            "Media.Headers.set" => {
                if let Some(entries) = headers_payload(&this) {
                    entries
                        .borrow_mut()
                        .insert(arg(0).text().to_ascii_lowercase(), arg(1).text());
                }
                Some(Value::Undefined)
            }
            "Media.Headers.append" => {
                let key = arg(0).text().to_ascii_lowercase();
                if let Some(entries) = headers_payload(&this) {
                    let mut entries = entries.borrow_mut();
                    match entries.get_mut(&key) {
                        Some(old) => {
                            old.push_str(", ");
                            old.push_str(&arg(1).text());
                        }
                        None => {
                            entries.insert(key, arg(1).text());
                        }
                    }
                }
                Some(Value::Undefined)
            }
            "Media.Headers.delete" => {
                if let Some(entries) = headers_payload(&this) {
                    entries
                        .borrow_mut()
                        .remove(&arg(0).text().to_ascii_lowercase());
                }
                Some(Value::Undefined)
            }
            "Media.WebSocket.send" => {
                let id = number(&this.get("__hostId"));
                self.invoke("__chuzzWsSend", vec![Value::number(id as f64), arg(0)])?;
                Some(Value::Undefined)
            }
            "Media.WebSocket.close" => {
                let id = number(&this.get("__hostId"));
                self.invoke(
                    "__chuzzWsClose",
                    vec![Value::number(id as f64), arg(0), arg(1)],
                )?;
                this.put("readyState", Value::number(2));
                Some(Value::Undefined)
            }
            "Media.AudioContext.resume"
            | "Media.AudioContext.suspend"
            | "Media.AudioContext.close" => Some(self.audio_command(name, &this, &args)?),
            "Media.AudioContext.createOscillator"
            | "Media.AudioContext.createGain"
            | "Media.AudioContext.createAnalyser" => Some(self.audio_command(name, &this, &args)?),
            "Media.MediaSource.addSourceBuffer"
            | "Media.SourceBuffer.appendBuffer"
            | "Media.SourceBuffer.remove"
            | "Media.SourceBuffer.abort" => Some(self.video_command(name, &this, &args)?),
            "Media.RTCPeerConnection.createOffer"
            | "Media.RTCPeerConnection.createAnswer"
            | "Media.RTCPeerConnection.setLocalDescription"
            | "Media.RTCPeerConnection.setRemoteDescription"
            | "Media.RTCPeerConnection.addIceCandidate"
            | "Media.RTCPeerConnection.addTrack"
            | "Media.RTCPeerConnection.close"
            | "Media.RTCPeerConnection.getStats" => Some(self.peer_command(name, &this, &args)?),
            _ => None,
        };
        Ok(called)
    }

    pub(super) fn resolve_promise(
        &mut self,
        promise: Value,
        value: Value,
        rejected: bool,
    ) -> Result<()> {
        self.pending.borrow_mut().push(HostJob {
            callback: Value::native("Web.HostResolvePromise"),
            this: Value::Undefined,
            args: vec![promise, value, Value::Bool(rejected)],
        });
        Ok(())
    }

    fn call_json_host(&mut self, name: &str, value: Value) -> Result<Value> {
        self.invoke(name, vec![value])
    }

    fn fetch(&mut self, input: Value, init: Value) -> Result<Value> {
        let url = if input.kind() == "Request" {
            input.get("url").text()
        } else {
            input.text()
        };
        if url.is_empty() {
            return Err(Error::type_error("fetch requires a URL"));
        }
        let method = if matches!(field(&init, "method"), Value::Undefined) {
            Value::string("GET")
        } else {
            field(&init, "method")
        };
        let headers = headers_from(&field(&init, "headers"));
        let body = field(&init, "body");
        let request = request_record(url, &init, &headers, &body)?;
        let state = self.media_state()?;
        let mut state = state.borrow_mut();
        if state.pending.len() >= MAX_MEDIA_OBJECTS {
            return Err(Error::new(
                "QuotaExceededError",
                "too many pending media requests",
            ));
        }
        let promise = Value::pending_promise();
        let request_id = state.next_id.max(1);
        state.next_id = request_id.checked_add(1).unwrap_or(1);
        state.pending.insert(request_id, promise.clone());
        let completions = Rc::clone(&state.completions);
        drop(state);
        let callback = self
            .host
            .retain_callback(move |_host, args| {
                let value = args.first().map_or(
                    Value::Undefined,
                    crate::page_native_bridge::host_callback_value,
                );
                let mut queue = completions.0.borrow_mut();
                if queue.len() < MAX_MEDIA_OBJECTS {
                    queue.push_back((request_id, value));
                }
                Ok(HostValue::Undefined)
            })
            .map_err(host_error)?;
        let callback_value = Value::native("HostCallback");
        if let Some(object) = callback_value.as_object() {
            object.borrow_mut().payload = Some(Rc::new(callback));
        }
        let result = self.invoke("__chuzzNetAsync", vec![request, callback_value]);
        if result.is_err() {
            self.media_state()?.borrow_mut().pending.remove(&request_id);
            return result.map(|_| promise);
        }
        let _ = method;
        Ok(promise)
    }

    pub(super) fn drain_media_completions(&mut self) -> Result<usize> {
        let state = self.media_state()?;
        let items = {
            let state = state.borrow();
            std::mem::take(&mut *state.completions.0.borrow_mut())
        };
        let mut count = 0;
        for (id, host_value) in items {
            let Some(promise) = self.media_state()?.borrow_mut().pending.remove(&id) else {
                continue;
            };
            match self.response_from_host(host_value) {
                Ok(response) => self.resolve_promise(promise, response, false)?,
                Err(error) => {
                    self.resolve_promise(promise, Value::error("TypeError", error.message), true)?
                }
            }
            count += 1;
        }
        Ok(count)
    }

    fn response_from_host(&mut self, value: Value) -> Result<Value> {
        if !truth(&value.get("ok"), false) {
            let message = value.get("error").text();
            return Err(Error::new(
                "TypeError",
                if message.is_empty() {
                    "network request failed".into()
                } else {
                    message
                },
            ));
        }
        let status = number(&value.get("status")).clamp(0, 599) as u16;
        let status_text = value.get("statusText").text();
        let url = value.get("url").text();
        let headers = headers_from(&value.get("headers"));
        let bytes = host_buffer_bytes(&value.get("bytes"))?;
        Ok(response(bytes, status, status_text, headers, url))
    }

    fn websocket(&mut self, url: Value, protocols: Value) -> Result<Value> {
        let socket = Value::object("WebSocket");
        socket.put("url", url.clone());
        socket.put("readyState", Value::number(0));
        socket.put("bufferedAmount", Value::number(0));
        socket.put("binaryType", Value::string("blob"));
        socket.put("protocol", Value::string(""));
        socket.put("extensions", Value::string(""));
        let id = self.allocate_media_id()?;
        socket.put_hidden("__hostId", Value::number(id as f64));
        let protocols = if protocols.kind() == "Array" {
            protocols.items()
        } else if matches!(protocols, Value::Undefined) {
            Vec::new()
        } else {
            vec![protocols]
        };
        self.invoke(
            "__chuzzWsConnect",
            vec![Value::number(id as f64), url, Value::array(protocols)],
        )?;
        socket.put_hidden("__protocols", Value::array(Vec::new()));
        Ok(socket)
    }

    fn audio_context(&mut self, args: &[Value]) -> Result<Value> {
        let context = Value::object("AudioContext");
        let options = args.first().cloned().unwrap_or(Value::Undefined);
        let sample_rate = if matches!(options.get("sampleRate"), Value::Undefined) {
            48_000
        } else {
            number(&options.get("sampleRate")).clamp(8_000, 96_000) as u32
        };
        let create = Value::object("Record");
        create.put("op", Value::string("createContext"));
        create.put("sampleRate", Value::number(sample_rate as f64));
        let result = self.invoke(
            "__chuzzAudioCall",
            vec![Value::string(create.to_json()?.to_string())],
        )?;
        let id = number(&result.get("context"));
        context.put("state", Value::string("suspended"));
        context.put("sampleRate", Value::number(sample_rate as f64));
        context.put("currentTime", Value::number(0));
        context.put("baseLatency", Value::number(0));
        context.put_hidden("__hostId", Value::number(id as f64));
        context.put_hidden("__options", options);
        Ok(context)
    }

    fn audio_command(&mut self, name: &str, context: &Value, args: &[Value]) -> Result<Value> {
        let operation = match name {
            "Media.AudioContext.resume" => "state",
            "Media.AudioContext.suspend" => "state",
            "Media.AudioContext.close" => "state",
            "Media.AudioContext.createOscillator"
            | "Media.AudioContext.createGain"
            | "Media.AudioContext.createAnalyser" => "createNode",
            _ => return Err(Error::type_error("unsupported audio operation")),
        };
        let command = Value::object("Record");
        command.put("op", Value::string(operation));
        command.put("context", context.get("__hostId"));
        if operation == "state" {
            let state = match name {
                "Media.AudioContext.resume" => "running",
                "Media.AudioContext.suspend" => "suspended",
                _ => "closed",
            };
            command.put("state", Value::string(state));
        } else {
            let kind = match name {
                "Media.AudioContext.createOscillator" => "oscillator",
                "Media.AudioContext.createGain" => "gain",
                _ => "analyser",
            };
            command.put("kind", Value::string(kind));
        }
        if let Some(node) = args.first() {
            command.put("node", node.clone());
        }
        let result = self.invoke(
            "__chuzzAudioCall",
            vec![Value::string(command.to_json()?.to_string())],
        )?;
        if operation == "state" {
            let state = match name {
                "Media.AudioContext.resume" => "running",
                "Media.AudioContext.suspend" => "suspended",
                _ => "closed",
            };
            context.put("state", Value::string(state));
        }
        Ok(result)
    }

    fn video_command(&mut self, name: &str, this: &Value, args: &[Value]) -> Result<Value> {
        let command = Value::object("Record");
        let op = match name {
            "Media.MediaSource.addSourceBuffer" => "addSourceBuffer",
            "Media.SourceBuffer.appendBuffer" => "append",
            "Media.SourceBuffer.remove" => "remove",
            _ => "abort",
        };
        command.put("op", Value::string(op));
        command.put("id", this.get("__hostId"));
        if let Some(arg) = args.first() {
            command.put(
                if op == "append" { "bytes" } else { "mimeType" },
                arg.clone(),
            );
        }
        self.invoke("__chuzzMseControl", vec![command])
    }

    fn peer_connection(&mut self, args: &[Value]) -> Result<Value> {
        let peer = Value::object("RTCPeerConnection");
        let request = Value::object("Record");
        request.put("op", Value::string("create"));
        request.put(
            "configuration",
            args.first().cloned().unwrap_or(Value::Undefined),
        );
        let id = number(&self.invoke("__chuzzPeerControl", vec![request])?);
        if id == 0 {
            return Err(Error::new(
                "OperationError",
                "peer host did not create a connection",
            ));
        }
        peer.put_hidden("__hostId", Value::number(id as f64));
        peer.put("connectionState", Value::string("new"));
        peer.put("iceConnectionState", Value::string("new"));
        peer.put("signalingState", Value::string("stable"));
        peer.put("localDescription", Value::Null);
        peer.put("remoteDescription", Value::Null);
        peer.put(
            "__configuration",
            args.first().cloned().unwrap_or(Value::Undefined),
        );
        Ok(peer)
    }

    fn peer_command(&mut self, name: &str, peer: &Value, args: &[Value]) -> Result<Value> {
        let method = name
            .strip_prefix("Media.RTCPeerConnection.")
            .unwrap_or(name);
        let op = match method {
            "close" => "close",
            "getStats" => "inspect",
            _ => {
                return Err(Error::new(
                    "NotSupportedError",
                    "the native peer host does not implement this WebRTC operation",
                ));
            }
        };
        let request = Value::object("Record");
        request.put("op", Value::string(op));
        request.put("peer", peer.get("__hostId"));
        if let Some(arg) = args.first() {
            request.put("value", arg.clone());
        }
        if let Some(arg) = args.get(1) {
            request.put("track", arg.clone());
        }
        let result = self.invoke("__chuzzPeerControl", vec![request])?;
        if op == "close" {
            peer.put("connectionState", Value::string("closed"));
        }
        Ok(result)
    }

    fn allocate_media_id(&mut self) -> Result<u64> {
        let state = self.media_state()?;
        let mut state = state.borrow_mut();
        if state.pending.len() >= MAX_MEDIA_OBJECTS {
            return Err(Error::new(
                "QuotaExceededError",
                "media object limit reached",
            ));
        }
        let id = state.next_id.max(1);
        state.next_id = id.checked_add(1).unwrap_or(1);
        Ok(id)
    }

    fn media_state(&self) -> Result<Rc<RefCell<MediaState>>> {
        self.state
            .get("media.native-state")
            .and_then(|value| value.as_object())
            .and_then(|object| object.borrow().payload.as_ref().cloned())
            .and_then(|payload| Rc::downcast::<RefCell<MediaState>>(payload).ok())
            .ok_or_else(|| Error::new("InvalidStateError", "media host state is unavailable"))
    }
}

fn host_error(error: HostError) -> Error {
    Error::new(error.name, error.message)
}

fn make_request(input: Value, init: Value) -> Value {
    let request = Value::object("Request");
    request.put(
        "url",
        if input.kind() == "Request" {
            input.get("url")
        } else {
            input.clone()
        },
    );
    for key in [
        "method",
        "headers",
        "body",
        "mode",
        "credentials",
        "cache",
        "redirect",
        "referrer",
        "integrity",
        "keepalive",
    ] {
        let candidate = init.get(key);
        if matches!(candidate, Value::Undefined) && input.kind() == "Request" {
            request.put(key, input.get(key));
        } else if !matches!(candidate, Value::Undefined) {
            request.put(key, candidate);
        }
    }
    request
}

fn make_blob(args: &[Value]) -> Result<Value> {
    let parts = args
        .first()
        .cloned()
        .unwrap_or(Value::array(Vec::new()))
        .items();
    let mut bytes = Vec::new();
    for part in parts {
        let chunk = blob_bytes(&part)?;
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(Error::new(
                "QuotaExceededError",
                "blob exceeds the media body limit",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    let blob = Value::object("Blob");
    blob.put("size", Value::number(bytes.len() as f64));
    blob.put(
        "type",
        Value::string(
            args.get(1)
                .map(Value::text)
                .unwrap_or_default()
                .to_ascii_lowercase(),
        ),
    );
    if let Some(object) = blob.as_object() {
        object.borrow_mut().payload = Some(Rc::new(MediaObject {
            bytes: Some(Rc::from(bytes)),
        }));
    }
    set_method(&blob, "arrayBuffer", "Media.Blob.arrayBuffer");
    set_method(&blob, "text", "Media.Blob.text");
    Ok(blob)
}

fn blob_bytes(value: &Value) -> Result<Vec<u8>> {
    let payload = value
        .as_object()
        .and_then(|object| object.borrow().payload.clone());
    if let Some(payload) = payload.as_ref()
        && let Ok(media) = Rc::downcast::<MediaObject>(Rc::clone(payload))
    {
        return Ok(media.bytes.as_deref().unwrap_or_default().to_vec());
    }
    if let Some(payload) = payload.as_ref()
        && let Ok(buffer) = Rc::downcast::<crate::script_host::HostBuffer>(Rc::clone(payload))
    {
        return buffer
            .as_bytes()
            .map(<[u8]>::to_vec)
            .map_err(|_| Error::new("TypeError", "detached host buffer"));
    }
    if let Some(payload) = payload.as_ref()
        && let Ok(bytes) = Rc::downcast::<Vec<u8>>(Rc::clone(payload))
    {
        return Ok(bytes.as_ref().clone());
    }
    if value.kind().ends_with("Array") || value.kind().ends_with("ArrayBuffer") {
        return Ok(value
            .items()
            .into_iter()
            .map(|item| number(&item).min(255) as u8)
            .collect());
    }
    Ok(value.text().into_bytes())
}

fn host_buffer_bytes(value: &Value) -> Result<Vec<u8>> {
    blob_bytes(value)
}

fn response_bytes(response: &Value) -> Result<Value> {
    if response.get("bodyUsed").truthy() {
        return Err(Error::type_error("response body has already been consumed"));
    }
    let bytes = blob_bytes(response)?;
    response.put("bodyUsed", Value::Bool(true));
    let array = Value::object("ArrayBuffer");
    array.put("byteLength", Value::number(bytes.len() as f64));
    if let Some(object) = array.as_object() {
        let mut object = object.borrow_mut();
        object.indexed = bytes
            .iter()
            .map(|byte| Value::Number(f64::from(*byte)))
            .collect();
        object.payload = Some(Rc::new(bytes));
    }
    Ok(Value::promise(Ok(array)))
}

fn response_text(response: &Value) -> Result<Value> {
    if response.get("bodyUsed").truthy() {
        return Err(Error::type_error("response body has already been consumed"));
    }
    let bytes = blob_bytes(response)?;
    response.put("bodyUsed", Value::Bool(true));
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Ok(Value::promise(Ok(Value::string(text))))
}

fn response_json(response: &Value) -> Result<Value> {
    if response.get("bodyUsed").truthy() {
        return Err(Error::type_error("response body has already been consumed"));
    }
    let bytes = blob_bytes(response)?;
    response.put("bodyUsed", Value::Bool(true));
    let text =
        String::from_utf8(bytes).map_err(|error| Error::new("SyntaxError", error.to_string()))?;
    let json = serde_json::from_str::<serde_json::Value>(&text)
        .map_err(|error| Error::new("SyntaxError", error.to_string()))?;
    Ok(Value::promise(Ok(Value::from_json(json))))
}

fn clone_response(source: &Value) -> Result<Value> {
    let bytes = blob_bytes(source)?;
    let status = number(&source.get("status")).min(599) as u16;
    Ok(response(
        bytes,
        status,
        source.get("statusText").text(),
        source.get("headers"),
        source.get("url").text(),
    ))
}

fn copy_record(kind: &str, source: &Value) -> Value {
    let result = Value::object(kind);
    if let Some(source) = source.as_object() {
        for (name, property) in &source.borrow().props {
            result.put(name, property.value.clone());
        }
    }
    result
}

fn media_command(op: &str, args: &[Value]) -> Value {
    let record = Value::object("Record");
    record.put("op", Value::string(op));
    if let Some(value) = args.first() {
        record.put("value", value.clone());
    }
    record
}
