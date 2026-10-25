//! JavaScript surface layered over the owner-local Ekojit host intrinsics.
//!
//! Keep this source in the page realm: callbacks passed to the `__chuzz*`
//! functions are retained by the document host and are delivered during its
//! normal poll. No second realm or synthetic host object is created here.

use std::sync::Arc;

use blitz_script_ekojit::{EkojitDocument, PageSource};

const BOOTSTRAPS: &[(&str, &str)] = &[
    (
        "environment",
        r#"(function (g) {
        g.navigator = __chuzzBrowserEnvironment();
        g.top = g; g.parent = g;
    })(globalThis);"#,
    ),
    (
        "websocket",
        r#"(function (g) {
  'use strict';
  // Socket events are delivered by the registered page callback. Native host
  // transport currently accepts text messages; binary send is rejected clearly.
  if (typeof __chuzzWsConnect === 'function' && typeof __chuzzWsSubscribe === 'function') {
    var sockets = new Map(), nextSocket = 1;
    function WebSocket(url, protocols) {
      if (this == null || this === g) { throw new TypeError("Failed to construct 'WebSocket': use new"); }
      var rawUrl = String(url);
      if (rawUrl.indexOf("wss:") === 0) { rawUrl = "https:" + rawUrl.slice(4); }
      else if (rawUrl.indexOf("ws:") === 0) { rawUrl = "http:" + rawUrl.slice(3); }
      var normalizedUrl = new URL(rawUrl, g.location && g.location.href || undefined).href;
      this.url = normalizedUrl.indexOf("https:") === 0 ? "wss:" + normalizedUrl.slice(6) : normalizedUrl.indexOf("http:") === 0 ? "ws:" + normalizedUrl.slice(5) : normalizedUrl;
      this.protocol = ''; this.extensions = ''; this.binaryType = 'blob'; this.bufferedAmount = 0;
      this.readyState = WebSocket.CONNECTING; this.onopen = this.onmessage = this.onerror = this.onclose = null;
      this._listeners = Object.create(null); this._id = nextSocket++; sockets.set(this._id, this);
      var socket = this;
      __chuzzWsSubscribe(this._id, function (event) {
        if (event.type === 'open') { socket.protocol = event.protocol || ''; socket.readyState = 1; socket.dispatchEvent({ type: 'open' }); }
        else if (event.type === 'message') { socket.dispatchEvent({ type: 'message', data: event.data }); }
        else if (event.type === 'error') { socket.dispatchEvent({ type: 'error', message: event.message }); }
        else if (event.type === 'close') { socket.readyState = 3; socket.dispatchEvent({ type: 'close', code: event.code || 1000, reason: event.reason || '', wasClean: true }); sockets.delete(socket._id); }
      });
      __chuzzWsConnect(this._id, this.url, protocols == null ? [] : (Array.isArray(protocols) ? protocols.map(String) : [String(protocols)]));
    }
    WebSocket.CONNECTING = 0; WebSocket.OPEN = 1; WebSocket.CLOSING = 2; WebSocket.CLOSED = 3;
    WebSocket.prototype = {};
    WebSocket.prototype.constructor = WebSocket;
    WebSocket.prototype.addEventListener = function (name, callback) { (this._listeners[name] || (this._listeners[name] = [])).push(callback); };
    WebSocket.prototype.removeEventListener = function (name, callback) { var list = this._listeners[name] || []; this._listeners[name] = list.filter(function (item) { return item !== callback; }); };
    WebSocket.prototype.dispatchEvent = function (event) {
      var list = (this._listeners[event.type] || []).slice(); event.target = this;
      list.forEach(function (callback) { if (typeof callback === 'function') { callback.call(this, event); } else if (callback && callback.handleEvent) { callback.handleEvent(event); } }, this);
      var handler = this['on' + event.type]; if (typeof handler === 'function') { handler.call(this, event); }
      return true;
    };
    WebSocket.prototype.send = function (data) {
      if (this.readyState !== WebSocket.OPEN) { throw new Error('WebSocket is not open'); }
      if (typeof data !== 'string') { throw new TypeError('binary WebSocket send is not supported by this host'); }
      __chuzzWsSend(this._id, data);
    };
    WebSocket.prototype.close = function (code, reason) {
      if (this.readyState >= WebSocket.CLOSING) { return; }
      this.readyState = WebSocket.CLOSING; __chuzzWsClose(this._id, code == null ? null : code);
    };
    g.WebSocket = WebSocket;
  }
})(globalThis);"#,
    ),
    (
        "media",
        r#"(function (g) {
  'use strict';
  // The native host owns capture sources and track state. Keep the JS objects
  // as views over those host handles so stop/settings/enabled stay authoritative.
  if (typeof __chuzzMediaControl === 'function') {
    var mediaTracks = Object.create(null), mediaDeviceListeners = [];
    var mediaDevices = { ondevicechange: null };
    mediaDevices.addEventListener = function (type, callback) { if (type === 'devicechange') { mediaDeviceListeners.push(callback); } };
    mediaDevices.removeEventListener = function (type, callback) { if (type === 'devicechange') { mediaDeviceListeners = mediaDeviceListeners.filter(function (item) { return item !== callback; }); } };
    mediaDevices.dispatchEvent = function (event) { event.target = mediaDevices; mediaDeviceListeners.slice().forEach(function (callback) { callback.call(mediaDevices, event); }); if (typeof mediaDevices.ondevicechange === 'function') { mediaDevices.ondevicechange.call(mediaDevices, event); } return true; };
    function dispatchMediaUpdates(updates) {
      if (!updates) { return; }
      (updates.tracks || []).forEach(function (descriptor) {
        var track = mediaTracks[descriptor.handle];
        if (!track) { return; }
        var wasEnded = track.readyState === 'ended';
        track._enabled = descriptor.enabled; track.muted = descriptor.muted; track.readyState = descriptor.readyState;
        if (!wasEnded && track.readyState === 'ended') {
          var event = { type: 'ended', target: track };
          (track._listeners.ended || []).slice().forEach(function (callback) { callback.call(track, event); });
          if (typeof track.onended === 'function') { track.onended.call(track, event); }
        }
      });
      if (updates.devicechange) { mediaDevices.dispatchEvent({ type: 'devicechange' }); }
    }
    function mediaCall(command) {
      var result = JSON.parse(__chuzzMediaControl(JSON.stringify(command)));
      dispatchMediaUpdates(result && result.updates);
      if (!result.ok) { var detail = result.error || {}; var error = new Error(detail.message || String(detail)); error.name = detail.name || 'NotSupportedError'; if (detail.constraint) { error.constraint = detail.constraint; } throw error; }
      return result.value;
    }
    function MediaStreamTrack(descriptor) {
      this.id = descriptor.id; this.kind = descriptor.kind; this.label = descriptor.label;
      this._enabled = descriptor.enabled; this.muted = descriptor.muted;
      this.readyState = descriptor.readyState; this._handle = descriptor.handle;
      this.onended = null; this._listeners = Object.create(null);
      mediaTracks[this._handle] = this;
    }
    Object.defineProperty(MediaStreamTrack.prototype, 'enabled', { get: function () { return this._enabled; }, set: function (value) { this._enabled = !!value; if (this.readyState !== 'ended') { mediaCall({ op: 'enabled', handle: this._handle, value: this._enabled }); } } });
    MediaStreamTrack.prototype.addEventListener = function (type, callback) { (this._listeners[type] || (this._listeners[type] = [])).push(callback); };
    MediaStreamTrack.prototype.removeEventListener = function (type, callback) { var list = this._listeners[type] || []; this._listeners[type] = list.filter(function (item) { return item !== callback; }); };
    MediaStreamTrack.prototype.getSettings = function () { return mediaCall({ op: 'describe', handle: this._handle }).settings; };
    MediaStreamTrack.prototype.getConstraints = function () { return mediaCall({ op: 'describe', handle: this._handle }).constraints; };
    MediaStreamTrack.prototype.getCapabilities = function () { return mediaCall({ op: 'describe', handle: this._handle }).capabilities; };
    MediaStreamTrack.prototype.stop = function () { if (this.readyState === 'ended') { return; } mediaCall({ op: 'stop', handle: this._handle }); this.readyState = 'ended'; };
    MediaStreamTrack.prototype.applyConstraints = function (constraints) { var self = this; try { var info = mediaCall({ op: 'apply', handle: this._handle, constraints: constraints || {} }); self.enabled = info.enabled; self.muted = info.muted; self.readyState = info.readyState; return Promise.resolve(); } catch (error) { return Promise.reject(error); } };
    MediaStreamTrack.prototype.clone = function () { return new MediaStreamTrack(mediaCall({ op: 'clone', handle: this._handle })); };
    function MediaStream(tracks) { this.id = 'chuzz-stream-' + String(Date.now()); this._tracks = tracks || []; }
    MediaStream.prototype.getTracks = function () { return this._tracks.slice(); };
    MediaStream.prototype.getAudioTracks = function () { return this._tracks.filter(function (track) { return track.kind === 'audio'; }); };
    MediaStream.prototype.getVideoTracks = function () { return this._tracks.filter(function (track) { return track.kind === 'video'; }); };
    MediaStream.prototype.getTrackById = function (id) { return this._tracks.filter(function (track) { return track.id === id; })[0] || null; };
    MediaStream.prototype.addTrack = function (track) { if (this._tracks.indexOf(track) < 0) { this._tracks.push(track); } };
    MediaStream.prototype.removeTrack = function (track) { this._tracks = this._tracks.filter(function (item) { return item !== track; }); };
    mediaDevices.enumerateDevices = function () {
        try { return Promise.resolve(mediaCall({ op: 'enumerate' })); }
        catch (error) { return Promise.reject(error); }
      };
    mediaDevices.getUserMedia = function (constraints) {
        try { return Promise.resolve(new MediaStream(mediaCall({ op: 'acquire', constraints: constraints || {} }).map(function (item) { return new MediaStreamTrack(item); }))); }
        catch (error) { return Promise.reject(error); }
    };
    if (typeof __chuzzMediaSubscribe === 'function') { __chuzzMediaSubscribe(function (updates) { dispatchMediaUpdates(updates); }); }
    g.navigator.mediaDevices = mediaDevices;
    g.MediaStream = MediaStream; g.MediaStreamTrack = MediaStreamTrack;
  }
})(globalThis);"#,
    ),
    (
        "webgpu",
        r#"(function (g) {
  'use strict';
  // WebGPU transport uses the same command envelope as the native bridge.
  // Every asynchronous reply is correlated with its request id in this realm.
  if (typeof __chuzzGpuSend === 'function' && typeof __chuzzGpuSubscribe === 'function') {
    var gpuNext = 1, gpuPending = new Map(), gpuHandles = 1, gpuDevices = Object.create(null);
    function gpuSend(request, fire, uploads) {
      var id = gpuNext++, device = request.device || 0;
      request.id = id;
      if (request.webgpu === 'command') { request.fire = !!fire; }
      var promise = fire ? null : new Promise(function (resolve, reject) { gpuPending.set(id, { resolve: resolve, reject: reject }); });
      try { __chuzzGpuSend(request, uploads || []); } catch (error) { if (promise) { var pending = gpuPending.get(id); gpuPending.delete(id); pending.reject(error); } throw error; }
      return promise;
    }
    __chuzzGpuSubscribe(function (kind, id, value) {
      if (kind === 'event') {
        var device = gpuDevices[id];
        if (!device || !value) { return; }
        if (value.type === 'lost') {
          device._lostResolve({ reason: value.reason || 'unknown', message: value.message || '' });
          device._listeners.deviceLost.slice().forEach(function (callback) { callback.call(device, { type: 'deviceLost', target: device, reason: value.reason || 'unknown', message: value.message || '' }); });
        } else if (value.type === 'error') {
          var detail = value.error || {};
          device.dispatchEvent({ type: 'uncapturederror', error: { name: detail.name || 'GPUValidationError', message: detail.message || String(detail) } });
        }
        return;
      }
      if (kind !== 'reply') { return; }
      var pending = gpuPending.get(id); if (!pending) { return; }
      gpuPending.delete(id);
      if (value && value.ok) { pending.resolve(value.value); }
      else { pending.reject(new Error(value && value.error || 'WebGPU operation failed')); }
    });
    function gpuCommand(device, op, handle, args, fire, uploads) {
      return gpuSend({ webgpu: 'command', device: device, command: { op: op, handle: handle || 0, args: gpuSerialize(args || {}) } }, fire, uploads);
    }
    function gpuUpload(bytes, offset, length) {
      if (typeof __chuzzGpuPutBytes !== 'function') { throw new TypeError('WebGPU byte upload bridge is unavailable'); }
      return __chuzzGpuPutBytes(bytes.buffer, bytes.byteOffset + offset, length);
    }
    function gpuExtent(value) {
      if (Array.isArray(value)) { return [Number(value[0]), Number(value[1]), value.length > 2 ? Number(value[2]) : 1]; }
      if (value && typeof value === 'object') { return [Number(value.width), Number(value.height == null ? 1 : value.height), Number(value.depthOrArrayLayers == null ? 1 : value.depthOrArrayLayers)]; }
      throw new TypeError('invalid GPU extent');
    }
    function normalizeTextureDescriptor(descriptor) {
      if (!descriptor || typeof descriptor !== 'object' || descriptor.size == null) { return descriptor; }
      var extent = descriptor.size, size;
      if (Array.isArray(extent)) {
        if (extent.length < 1 || extent.length > 3) { return descriptor; }
        size = extent.slice();
        while (size.length < 3) { size.push(1); }
      } else if (typeof extent === 'object') {
        size = [extent.width, extent.height == null ? 1 : extent.height, extent.depthOrArrayLayers == null ? 1 : extent.depthOrArrayLayers];
      } else { return descriptor; }
      var copy = {};
      Object.keys(descriptor).forEach(function (key) { copy[key] = descriptor[key]; });
      copy.size = size;
      if (copy.label === undefined) { copy.label = ''; }
      if (copy.dimension == null) { copy.dimension = '2d'; }
      if (copy.mipLevelCount == null) { copy.mipLevelCount = 1; }
      if (copy.sampleCount == null) { copy.sampleCount = 1; }
      if (copy.viewFormats == null) { copy.viewFormats = []; }
      return copy;
    }
    function descriptorCopy(descriptor) {
      var copy = {};
      if (descriptor && typeof descriptor === 'object') {
        Object.keys(descriptor).forEach(function (key) { copy[key] = descriptor[key]; });
      }
      return copy;
    }
    function normalizeBufferDescriptor(descriptor) {
      var copy = descriptorCopy(descriptor);
      if (copy.label === undefined) { copy.label = ''; }
      if (copy.mappedAtCreation === undefined) { copy.mappedAtCreation = false; }
      return copy;
    }
    function normalizeLabelDescriptor(descriptor) {
      var copy = descriptorCopy(descriptor);
      if (copy.label === undefined) { copy.label = ''; }
      return copy;
    }
    function normalizeEncoderDescriptor(descriptor) {
      var copy = descriptorCopy(descriptor);
      if (copy.label === undefined) { copy.label = ''; }
      return copy;
    }
    function gpuOrigin(value, dimensions) {
      if (value === undefined) { return dimensions === 2 ? [0, 0] : [0, 0, 0]; }
      if (value && typeof value === 'object' && !Array.isArray(value)) {
        return dimensions === 2
          ? [value.x === undefined ? 0 : value.x, value.y === undefined ? 0 : value.y]
          : [value.x === undefined ? 0 : value.x, value.y === undefined ? 0 : value.y, value.z === undefined ? 0 : value.z];
      }
      return value;
    }
    function normalizeTextureCopy(copy) {
      var normalized = descriptorCopy(copy);
      if (normalized.mipLevel === undefined) { normalized.mipLevel = 0; }
      normalized.origin = gpuOrigin(normalized.origin, 3);
      if (normalized.aspect === undefined) { normalized.aspect = 'all'; }
      return normalized;
    }
    function normalizeBufferCopy(copy) {
      var normalized = descriptorCopy(copy);
      if (normalized.offset === undefined) { normalized.offset = 0; }
      return normalized;
    }
    function normalizePipelineStage(stage, vertex) {
      if (stage == null) { return stage; }
      var copy = descriptorCopy(stage);
      if (copy.constants == null) { copy.constants = {}; }
      if (vertex && copy.buffers == null) { copy.buffers = []; }
      if (!vertex && copy.targets == null) { copy.targets = []; }
      if (!vertex) {
        copy.targets = copy.targets.map(function (target) {
          if (target == null) { return null; }
          var normalized = descriptorCopy(target);
          if (normalized.writeMask == null) { normalized.writeMask = GPUColorWrite.ALL; }
          return normalized;
        });
      }
      return copy;
    }
    function normalizeRenderPipelineDescriptor(descriptor) {
      var copy = descriptorCopy(descriptor);
      if (copy.label === undefined) { copy.label = ''; }
      if (copy.layout === 'auto') { copy.layout = null; }
      copy.vertex = normalizePipelineStage(copy.vertex, true);
      copy.fragment = normalizePipelineStage(copy.fragment, false);
      var primitive = descriptorCopy(copy.primitive);
      if (primitive.frontFace == null) { primitive.frontFace = 'ccw'; }
      if (primitive.cullMode == null) { primitive.cullMode = null; }
      if (primitive.stripIndexFormat == null) { primitive.stripIndexFormat = null; }
      if (primitive.unclippedDepth == null) { primitive.unclippedDepth = false; }
      copy.primitive = primitive;
      var multisample = descriptorCopy(copy.multisample);
      if (multisample.count == null) { multisample.count = 1; }
      if (multisample.mask == null) { multisample.mask = 4294967295; }
      if (multisample.alphaToCoverageEnabled == null) { multisample.alphaToCoverageEnabled = false; }
      copy.multisample = multisample;
      return copy;
    }
    function normalizeBindGroupDescriptor(descriptor) {
      var copy = descriptorCopy(descriptor);
      if (copy.label == null) { copy.label = ''; }
      copy.entries = (copy.entries || []).map(function (entry) {
        var normalized = descriptorCopy(entry), resource = entry && entry.resource;
        if (resource && resource.buffer && resource.buffer.__gpuHandle != null) {
          var bufferBinding = { kind: 'buffer', handle: resource.buffer.__gpuHandle, offset: resource.offset || 0 };
          if (resource.size != null) { bufferBinding.size = resource.size; }
          normalized.resource = bufferBinding;
        } else if (resource && resource.__gpuHandle != null) {
          var kind = resource.__gpuKind === 'GPUTextureView' ? 'view' : resource.__gpuKind === 'GPUSampler' ? 'sampler' : 'buffer';
          normalized.resource = { kind: kind, handle: resource.__gpuHandle };
          if (kind === 'buffer') { normalized.resource.offset = 0; }
        }
        return normalized;
      });
      return copy;
    }
    function normalizeRenderPassDescriptor(descriptor) {
      var copy = descriptorCopy(descriptor);
      if (copy.label == null) { copy.label = ''; }
      copy.colorAttachments = (copy.colorAttachments || []).map(function (attachment) {
        if (attachment == null) { return null; }
        var normalized = descriptorCopy(attachment), clear = normalized.clearValue;
        if (clear && typeof clear === 'object' && !Array.isArray(clear)) {
          normalized.clearValue = [clear.r, clear.g, clear.b, clear.a];
        }
        return normalized;
      });
      return copy;
    }
    function gpuSerialize(value) {
      if (value == null || typeof value !== 'object') { return value; }
      if (value.__gpuHandle != null) { return value.__gpuHandle; }
      if (Array.isArray(value)) { return value.map(gpuSerialize); }
      if (__chuzzIsByteBuffer(value)) { return __chuzzBytesArray(value); }
      var result = {};
      Object.keys(value).forEach(function (key) { if (key.indexOf('__gpu') !== 0 && value[key] !== undefined) { result[key] = gpuSerialize(value[key]); } });
      return result;
    }
    function gpuObject(kind, device, id, methods) {
      var value = { __gpuKind: kind, __gpuHandle: id, __gpuDevice: device };
      Object.keys(methods).forEach(function (name) { value[name] = methods[name]; });
      return value;
    }
    function features(values) {
      var list = Array.isArray(values) ? values : [];
      var result = { has: function (name) { return list.indexOf(String(name)) >= 0; }, values: function () { return list[Symbol.iterator](); }, keys: function () { return list[Symbol.iterator](); }, entries: function () { return list.map(function (v) { return [v, v]; })[Symbol.iterator](); }, forEach: function (callback, self) { list.forEach(function (item) { callback.call(self, item, item, result); }); }, size: list.length, length: list.length };
      list.forEach(function (item, index) { result[index] = item; });
      if (typeof Symbol === 'function' && Symbol.iterator) { result[Symbol.iterator] = function () { return list[Symbol.iterator](); }; }
      return result;
    }
    function createDevice(adapter, options) {
      return gpuSend({ webgpu: 'requestDevice', adapter: adapter.__gpuHandle, options: options || {} }).then(function (reply) {
        var id = reply.handle; var device = gpuObject('GPUDevice', id, id, {}); gpuDevices[id] = device;
        device.features = features(reply.features || []); device.limits = reply.limits || {}; device.queue = makeQueue(device);
        device._listeners = { deviceLost: [], uncapturederror: [] };
        device.lost = new Promise(function (resolve) { device._lostResolve = resolve; });
        device.addEventListener = function (type, callback) { if (device._listeners[type]) { device._listeners[type].push(callback); } };
        device.removeEventListener = function (type, callback) { if (device._listeners[type]) { device._listeners[type] = device._listeners[type].filter(function (item) { return item !== callback; }); } };
        device.dispatchEvent = function (event) { var callbacks = (device._listeners[event.type] || []).slice(); event.target = device; callbacks.forEach(function (callback) { callback.call(device, event); }); var handler = device['on' + event.type]; if (typeof handler === 'function') { handler.call(device, event); } return !event.defaultPrevented; };
        device.createBuffer = function (desc) { return makeResource(device, 'GPUBuffer', 'createBuffer', normalizeBufferDescriptor(desc)); };
        device.createTexture = function (desc) { return makeResource(device, 'GPUTexture', 'createTexture', normalizeTextureDescriptor(desc)); };
        device.createShaderModule = function (desc) { return makeResource(device, 'GPUShaderModule', 'createShaderModule', normalizeLabelDescriptor(desc)); };
        device.createSampler = function (desc) { return makeResource(device, 'GPUSampler', 'createSampler', desc); };
        device.createBindGroupLayout = function (desc) { return makeResource(device, 'GPUBindGroupLayout', 'createBindGroupLayout', normalizeLabelDescriptor(desc)); };
        device.createPipelineLayout = function (desc) { return makeResource(device, 'GPUPipelineLayout', 'createPipelineLayout', normalizeLabelDescriptor(desc)); };
        device.createBindGroup = function (desc) { return makeResource(device, 'GPUBindGroup', 'createBindGroup', normalizeBindGroupDescriptor(desc)); };
        device.createRenderPipeline = function (desc) { return makeResource(device, 'GPURenderPipeline', 'createRenderPipeline', normalizeRenderPipelineDescriptor(desc)); };
        device.createCommandEncoder = function (desc) { return makeEncoder(device, desc || {}); };
        device.pushErrorScope = function (filter) { gpuCommand(id, 'pushErrorScope', 0, { filter: filter }, true); };
        device.popErrorScope = function () { return gpuCommand(id, 'popErrorScope', 0, {}, false); };
        device.destroy = function () { gpuCommand(id, 'destroy', 0, {}, true); };
        return device;
      });
    }
    function makeResource(device, kind, op, descriptor) {
      var id = gpuHandles++;
      gpuCommand(device.__gpuHandle, op, id, descriptor || {}, true);
      var resource = gpuObject(kind, device.__gpuHandle, id, {});
      resource.label = descriptor && descriptor.label || '';
      resource.destroy = function () { gpuCommand(device.__gpuHandle, kind === 'GPUBuffer' ? 'destroyBuffer' : kind === 'GPUTexture' ? 'destroyTexture' : 'destroyResource', id, {}, true); };
      if (kind === 'GPUBuffer') {
        resource.size = descriptor.size; resource.usage = descriptor.usage; resource.mapState = 'unmapped';
        resource._mappedRange = null;
        resource.mapAsync = function (mode, offset, size) {
          if (resource.mapState !== 'unmapped') { return Promise.reject(new TypeError('GPUBuffer is already mapped')); }
          resource.mapState = 'pending';
          return gpuCommand(device.__gpuHandle, 'mapRead', id, { mode: mode, offset: offset || 0, size: size == null ? descriptor.size : size }, false).then(function (bytes) {
            if (typeof bytes === 'number' && typeof __chuzzGpuTakeBytes === 'function') { bytes = __chuzzGpuTakeBytes(bytes); }
            resource._mappedRange = bytes; resource.mapState = 'mapped'; return undefined;
          }, function (error) { resource.mapState = 'unmapped'; throw error; });
        };
        resource.getMappedRange = function (offset, size) {
          if (resource.mapState !== 'mapped' || resource._mappedRange == null) { throw new TypeError('GPUBuffer is not mapped'); }
          var bytes = __chuzzCopyBytes(resource._mappedRange), start = offset || 0, end = size == null ? bytes.length : start + size;
          if (start < 0 || end < start || end > bytes.length) { throw new RangeError('mapped range is out of bounds'); }
          return bytes.slice(start, end);
        };
        resource.unmap = function () { gpuCommand(device.__gpuHandle, 'unmap', id, {}, true); resource._mappedRange = null; resource.mapState = 'unmapped'; };
      }
      if (kind === 'GPUTexture') {
        resource.createView = function (view) { var viewId = gpuHandles++; gpuCommand(device.__gpuHandle, 'createView', viewId, { texture: id, descriptor: view || {} }, true); return gpuObject('GPUTextureView', device.__gpuHandle, viewId, {}); };
      }
      if (kind === 'GPURenderPipeline') { resource.getBindGroupLayout = function (index) { var layout = gpuHandles++; gpuCommand(device.__gpuHandle, 'getBindGroupLayout', layout, { pipeline: id, kind: 'render', index: index }, true); return gpuObject('GPUBindGroupLayout', device.__gpuHandle, layout, {}); }; }
      return resource;
    }
    function makeQueue(device) {
      var queue = {};
      queue.writeBuffer = function (buffer, offset, data, dataOffset, size) {
        var bytes = __chuzzCopyBytes(data);
        var elementSize = data && data.constructor && data.constructor.name === 'DataView' ? 1 : (data && data.BYTES_PER_ELEMENT) || 1;
        var start = (dataOffset || 0) * elementSize, length = size == null ? bytes.length - start : size * elementSize;
        if (start < 0 || length < 0 || start + length > bytes.length || start % 4 || length % 4) { throw new RangeError('GPUQueue.writeBuffer data range must be in bounds and 4-byte aligned'); }
        var copy = new Uint8Array(bytes.length); copy.set(bytes);
        var upload = gpuUpload(copy, start, length);
        gpuSend({ webgpu: 'command', device: device.__gpuHandle, command: { op: 'writeBuffer', handle: buffer.__gpuHandle, args: { offset: offset, data: upload } } }, true, [upload]);
      };
      queue.writeTexture = function (destination, data, layout, size) {
        var bytes = __chuzzCopyBytes(data);
        var copy = new Uint8Array(bytes.length); copy.set(bytes);
        var upload = gpuUpload(copy, 0, copy.length);
        var target = normalizeTextureCopy(destination);
        var options = { texture: target.texture.__gpuHandle, mipLevel: target.mipLevel, origin: target.origin, aspect: target.aspect, data: upload, offset: layout && layout.offset != null ? layout.offset : 0, bytesPerRow: layout && layout.bytesPerRow, rowsPerImage: layout && layout.rowsPerImage, size: gpuExtent(size) };
        gpuSend({ webgpu: 'command', device: device.__gpuHandle, command: { op: 'writeTexture', handle: 0, args: gpuSerialize(options) } }, true, [upload]);
      };
      queue.copyExternalImageToTexture = function (source, destination, size) {
        if (!source || !destination || !destination.texture || !size) { throw new TypeError('invalid external image copy arguments'); }
        var image = source.source || source;
        if (!image.__gpuImageSource) { throw new TypeError('external image source is not a native ImageBitmap'); }
        var target = normalizeTextureCopy(destination);
        var args = {
          source: image.__gpuImageSource,
          sourceOrigin: gpuOrigin(source.origin, 2),
          flipY: !!source.flipY,
          texture: target.texture.__gpuHandle,
          mipLevel: target.mipLevel,
          origin: target.origin,
          aspect: target.aspect,
          premultipliedAlpha: !!destination.premultipliedAlpha,
          colorSpace: destination.colorSpace || 'srgb',
          size: gpuExtent(size)
        };
        gpuCommand(device.__gpuHandle, 'copyExternalImageToTexture', 0, args, true);
      };
      queue.submit = function (commands) {
        gpuCommand(device.__gpuHandle, 'submit', 0, commands.map(function (command) { return command.__gpuHandle; }), true);
        gpuCanvasContexts.forEach(function (context) {
          if (context.__chuzzDevice === device && context.__chuzzCurrentTexture) {
            gpuCommand(device.__gpuHandle, 'present', context.__chuzzCurrentTexture.__gpuHandle, { canvas: context.__chuzzCanvasHandle, generation: context.__chuzzGeneration }, true);
            context.__chuzzCurrentTexture = null;
          }
        });
      };
      queue.onSubmittedWorkDone = function () { return gpuCommand(device.__gpuHandle, 'onSubmittedWorkDone', 0, {}, false); };
      return queue;
    }
    function makeEncoder(device, descriptor) {
      var id = gpuHandles++, encoder = gpuObject('GPUCommandEncoder', device.__gpuHandle, id, {}), finished = false, operations = [];
      gpuCommand(device.__gpuHandle, 'createCommandEncoder', id, normalizeEncoderDescriptor(descriptor), true);
      function encode(op, args) { if (finished) { throw new TypeError('command encoder is finished'); } operations.push({ op: op, args: gpuSerialize(args || {}) }); }
      encoder.beginRenderPass = function (desc) {
        var render = gpuSerialize(normalizeRenderPassDescriptor(desc || {})); render.commands = []; var ended = false;
        operations.push({ op: 'render', args: render });
        var pass = gpuObject('GPURenderPassEncoder', device.__gpuHandle, gpuHandles++, {});
        function passCommand(op, values) { if (ended) { throw new TypeError("render pass has ended"); } render.commands.push({ op: op, args: gpuSerialize(values) }); }
        pass.setPipeline = function (pipeline) { passCommand('pipeline', pipeline); };
        pass.setBindGroup = function (index, group, offsets) { passCommand('bindGroup', { index: index, group: group, offsets: offsets || [] }); };
        pass.setVertexBuffer = function (slot, buffer, offset, size) { passCommand('vertexBuffer', { slot: slot, buffer: buffer, offset: offset || 0, size: size }); };
        pass.setIndexBuffer = function (buffer, format, offset, size) { passCommand('indexBuffer', { buffer: buffer, format: format, offset: offset || 0, size: size }); };
        pass.draw = function (vertexCount, instanceCount, firstVertex, firstInstance) { passCommand('draw', { count: vertexCount, instances: instanceCount == null ? 1 : instanceCount, first: firstVertex || 0, firstInstance: firstInstance || 0 }); };
        pass.drawIndexed = function (indexCount, instanceCount, firstIndex, baseVertex, firstInstance) { passCommand('drawIndexed', { count: indexCount, instances: instanceCount == null ? 1 : instanceCount, first: firstIndex || 0, baseVertex: baseVertex || 0, firstInstance: firstInstance || 0 }); };
        pass.end = function () { if (ended) { throw new TypeError("render pass has ended"); } ended = true; };
        return pass;
      };
      encoder.copyBufferToTexture = function (source, destination, size) { encode('copyBufferToTexture', { source: normalizeBufferCopy(source), destination: normalizeTextureCopy(destination), size: gpuExtent(size) }); };
      encoder.copyTextureToBuffer = function (source, destination, size) { encode('copyTextureToBuffer', { source: normalizeTextureCopy(source), destination: normalizeBufferCopy(destination), size: gpuExtent(size) }); };
      encoder.copyTextureToTexture = function (source, destination, size) { encode('copyTextureToTexture', { source: normalizeTextureCopy(source), destination: normalizeTextureCopy(destination), size: gpuExtent(size) }); };
      encoder.clearBuffer = function (buffer, offset, size) { encode('clearBuffer', { buffer: buffer, offset: offset || 0, size: size }); };
      encoder.finish = function () { finished = true; var commandId = gpuHandles++; gpuCommand(device.__gpuHandle, 'finish', commandId, { encoder: id, label: descriptor.label || '', operations: operations }, true); return gpuObject('GPUCommandBuffer', device.__gpuHandle, commandId, {}); };
      return encoder;
    }
    var gpuCanvasContexts = [];
    function canvasDimension(canvas, name, defaultValue) {
      var property = canvas && canvas[name];
      if (typeof property === 'number' && Number.isFinite(property) && property >= 0) { return Math.min(4294967295, Math.floor(property)); }
      if (canvas && typeof canvas.getAttribute === 'function') {
        var raw = canvas.getAttribute(name);
        if (raw != null) {
          raw = String(raw).replace(/^\s+|\s+$/g, '');
          if (raw === '') { return defaultValue; }
          var reflected = Number(raw);
          if (Number.isFinite(reflected) && reflected >= 0) { return Math.min(4294967295, Math.floor(reflected)); }
          return defaultValue;
        }
      }
      return defaultValue;
    }
    function canvasBufferSize(canvas, context) {
      var width = context && context.__chuzzCanvasWidth != null
        ? Number(context.__chuzzCanvasWidth) : canvasDimension(canvas, 'width', 300);
      var height = context && context.__chuzzCanvasHeight != null
        ? Number(context.__chuzzCanvasHeight) : canvasDimension(canvas, 'height', 150);
      return [width, height];
    }
    function installCanvasContext(context) {
      if (!context || context.__chuzzGpuWrapped) { return context; }
      var canvas = context.canvas;
      var handle = context.__chuzzCanvasHandle != null ? context.__chuzzCanvasHandle : canvas && canvas.__chuzzCanvasHandle;
      if (handle == null) { return context; }
      context.__chuzzGpuWrapped = true; context.__chuzzCanvasHandle = Number(handle);
      if (canvas) { canvas.__chuzzWebGpuContext = context; }
      if (typeof gpuContextsByCanvas !== 'undefined') { gpuContextsByCanvas.set(Number(handle), context); }
      context.configure = function (configuration) {
        configuration = configuration || {};
        var device = configuration.device;
        if (!device || !device.__gpuHandle) { throw new TypeError('GPUCanvasConfiguration.device is required'); }
        var dimensions = canvasBufferSize(canvas, context), width = dimensions[0], height = dimensions[1];
        if (!Number.isFinite(width) || !Number.isFinite(height) || width < 0 || height < 0) { throw new TypeError('invalid canvas drawing buffer size'); }
        context.__chuzzDevice = device; context.__chuzzGeneration = (context.__chuzzGeneration || 0) + 1;
        context.__chuzzConfiguration = { device: device, format: configuration.format || gpu.getPreferredCanvasFormat(), usage: configuration.usage == null ? GPUTextureUsage.RENDER_ATTACHMENT : configuration.usage, alphaMode: configuration.alphaMode || 'premultiplied', viewFormats: configuration.viewFormats || [] };
        gpuCommand(device.__gpuHandle, 'configure', context.__chuzzCanvasHandle, { generation: context.__chuzzGeneration, format: context.__chuzzConfiguration.format, usage: context.__chuzzConfiguration.usage, alphaMode: context.__chuzzConfiguration.alphaMode, viewFormats: context.__chuzzConfiguration.viewFormats, width: width, height: height }, true);
        gpuCommand(device.__gpuHandle, 'setCanvasId', context.__chuzzCanvasHandle, 'canvas:#' + String(canvas.id || context.__chuzzCanvasHandle), true);
        if (gpuCanvasContexts.indexOf(context) < 0) { gpuCanvasContexts.push(context); }
      };
      context.unconfigure = function () {
        if (!context.__chuzzDevice) { return; }
        gpuCommand(context.__chuzzDevice.__gpuHandle, 'unconfigure', context.__chuzzCanvasHandle, context.__chuzzGeneration + 1, true);
        context.__chuzzDevice = null; context.__chuzzConfiguration = null; context.__chuzzCurrentTexture = null;
      };
      context.getConfiguration = function () { return context.__chuzzConfiguration || null; };
      context.getCurrentTexture = function () {
        var device = context.__chuzzDevice, config = context.__chuzzConfiguration;
        if (!device || !config) { throw new TypeError('GPUCanvasContext is not configured'); }
        var dimensions = canvasBufferSize(canvas, context), textureId = gpuHandles++, width = dimensions[0], height = dimensions[1];
        if (!Number.isFinite(width) || !Number.isFinite(height) || width < 0 || height < 0) { throw new TypeError('invalid canvas drawing buffer size'); }
        var texture = gpuObject('GPUTexture', device.__gpuHandle, textureId, {}); texture.width = width; texture.height = height; texture.format = config.format; texture.usage = config.usage;
        texture.createView = function (descriptor) { var viewId = gpuHandles++; gpuCommand(device.__gpuHandle, 'createView', viewId, { texture: textureId, descriptor: descriptor || {} }, true); return gpuObject('GPUTextureView', device.__gpuHandle, viewId, {}); };
        texture.destroy = function () {};
        gpuCommand(device.__gpuHandle, 'currentTexture', textureId, { canvas: context.__chuzzCanvasHandle, generation: context.__chuzzGeneration, width: width, height: height }, true);
        context.__chuzzCurrentTexture = texture;
        return texture;
      };
      return context;
    }
    function patchCanvasGetContext() {
      if (typeof Object.getPrototypeOf !== "function") { return; }
      var canvas = g.document && g.document.createElement && g.document.createElement('canvas');
      var prototype = canvas && Object.getPrototypeOf(canvas);
      if (!prototype || typeof prototype.getContext !== 'function' || prototype.__chuzzGpuPatched) { return; }
      var original = prototype.getContext;
      prototype.getContext = function (kind) { var context = original.apply(this, arguments); return String(kind).toLowerCase() === 'webgpu' ? installCanvasContext(context) : context; };
      prototype.__chuzzGpuPatched = true;
    }
    function safeCanvasInteger(value) {
      return Number.isFinite(value) && Math.floor(value) === value && Math.abs(value) <= 9007199254740991;
    }
    var native2dContexts = new Map(), gpuContextsByCanvas = new Map(), nextCanvasSource = 1;
    function canvasRequest(request, inputs) {
      if (typeof __chuzzCanvas2D !== 'function') { throw new TypeError('native 2D canvas bridge is unavailable'); }
      return __chuzzCanvas2D(request, inputs || []);
    }
    function canvasDimensions(canvas) {
      var dimensions = canvasBufferSize(canvas), width = dimensions[0], height = dimensions[1];
      if (!safeCanvasInteger(width) || !safeCanvasInteger(height) || width <= 0 || height <= 0) { throw new TypeError('invalid canvas dimensions'); }
      return [width, height];
    }
    function canvasDrawState(context) {
      return {
        matrix: [1, 0, 0, 1, 0, 0], alpha: 1, composite: 'source-over', width: 1,
        cap: 'butt', join: 'miter', miter: 10, dash: [], dashOffset: 0,
        font: '10px sans-serif', kerning: 'auto', fontStretch: 'normal', fontVariantCaps: 'normal',
        letterSpacing: 0, wordSpacing: 0, textRendering: 'auto', align: 'start', baseline: 'alphabetic',
        direction: 'inherit', smoothing: context.imageSmoothingEnabled !== false, shadowBlur: 0,
        shadowColor: 'rgba(0, 0, 0, 0)', shadowOffsetX: 0, shadowOffsetY: 0, filter: []
      };
    }
    function sourceImageFor2d(source) {
      if (!source) { throw new TypeError('drawImage source is required'); }
      var imageSource = source.__gpuImageSource;
      if (imageSource && imageSource.kind === 'bitmap') {
        return { width: Number(source.width), height: Number(source.height), image: { bitmap: imageSource.handle }, inputs: [] };
      }
      var textureSource = source.__gpuKind === 'GPUTexture' ? {
        kind: 'texture', device: source.__gpuDevice, texture: source.__gpuHandle, premultipliedAlpha: false
      } : null;
      var canvas = source.canvas || source;
      var canvasHandle = canvas && canvas.__chuzzCanvasHandle;
      var gpuContext = canvas && canvas.__chuzzWebGpuContext || gpuContextsByCanvas.get(Number(canvasHandle));
      if (!textureSource && gpuContext && gpuContext.__chuzzConfiguration) {
        var dimensions = canvasDimensions(canvas);
        textureSource = {
          kind: 'canvas', canvas: Number(canvasHandle), generation: gpuContext.__chuzzGeneration,
          width: dimensions[0], height: dimensions[1]
        };
      }
      if (textureSource) {
        var marker = Math.pow(2, 51) + nextCanvasSource++;
        var resolved = canvasRequest({ action: 'source', id: marker, serial: marker, source: textureSource }, []);
        if (!Array.isArray(resolved) || resolved.length !== 2) { throw new TypeError('native canvas source returned invalid dimensions'); }
        var width = Number(resolved[0]), height = Number(resolved[1]);
        return { width: width, height: height, marker: marker, image: { native: marker }, inputs: [] };
      }
      var twoD = canvas && (canvas.__chuzz2dContext || native2dContexts.get(Number(canvasHandle)));
      if (twoD) {
        var size = canvasDimensions(canvas);
        var pixels = __chuzzCopyBytes(canvasRequest({ action: 'snapshot', id: Number(canvasHandle), width: size[0], height: size[1] }, []));
        return { width: size[0], height: size[1], image: { index: 0 }, inputs: [pixels] };
      }
      if (source.__chuzzNativeImageHandle != null) {
        var imageWidth = Number(source.width), imageHeight = Number(source.height);
        return { width: imageWidth, height: imageHeight, image: { native: Number(source.__chuzzNativeImageHandle) }, inputs: [] };
      }
      throw new TypeError('drawImage source cannot be resolved by the native canvas bridge');
    }
    function native2dContext(canvas) {
      var handle = Number(canvas.__chuzzCanvasHandle);
      if (!safeCanvasInteger(handle) || handle <= 0) { throw new TypeError('canvas has no native handle'); }
      var context = {
        canvas: canvas,
        imageSmoothingEnabled: true,
        drawImage: function (source) {
          if (arguments.length !== 3 && arguments.length !== 5 && arguments.length !== 9) { throw new TypeError('drawImage expects 3, 5, or 9 arguments'); }
          var image = sourceImageFor2d(source);
          var dimensions = canvasDimensions(canvas), width = image.width, height = image.height;
          if (!safeCanvasInteger(width) || !safeCanvasInteger(height) || width <= 0 || height <= 0 || !safeCanvasInteger(width * height * 4)) { throw new TypeError('invalid drawImage source dimensions'); }
          var values;
          if (arguments.length === 3) {
            values = [0, 0, width, height, Number(arguments[1]), Number(arguments[2]), width, height];
          } else if (arguments.length === 5) {
            values = [0, 0, width, height, Number(arguments[1]), Number(arguments[2]), Number(arguments[3]), Number(arguments[4])];
          } else if (arguments.length === 9) {
            values = Array.prototype.slice.call(arguments, 1).map(Number);
          }
          var descriptor = {
            index: 0, width: width, height: height, offset: 0, length: width * height * 4
          };
          if (image.image.index != null) { descriptor.index = image.image.index; }
          if (image.image.bitmap != null) { descriptor.bitmap = image.image.bitmap; }
          if (image.image.native != null) { descriptor.native = image.image.native; }
          var command = {
            op: 'drawImage', n: values, state: canvasDrawState(context),
            brush: { kind: 'pattern', matrix: [1, 0, 0, 1, 0, 0], repetition: 'no-repeat', image: descriptor }
          };
          try {
            canvasRequest({ action: 'submit', id: handle, width: dimensions[0], height: dimensions[1], commands: [command] }, image.inputs);
          } finally {
            if (image.marker != null) { canvasRequest({ action: 'release', id: image.marker }, []); }
          }
        },
        getImageData: function (x, y, width, height) {
          var dimensions = canvasDimensions(canvas);
          x = Number(x); y = Number(y); width = Number(width); height = Number(height);
          if (![x, y, width, height].every(Number.isFinite) || width === 0 || height === 0) { throw new TypeError('invalid getImageData rectangle'); }
          if (width < 0) { x += width; width = -width; }
          if (height < 0) { y += height; height = -height; }
          x = Math.trunc(x); y = Math.trunc(y); width = Math.trunc(width); height = Math.trunc(height);
          if (!safeCanvasInteger(width) || !safeCanvasInteger(height) || width <= 0 || height <= 0 || width > 4294967295 || height > 4294967295 || !safeCanvasInteger(width * height * 4)) { throw new TypeError('invalid getImageData dimensions'); }
          var data = __chuzzCopyBytes(canvasRequest({ action: 'read', id: handle, width: dimensions[0], height: dimensions[1], n: [x, y, width, height] }, []));
          return { data: data, width: width, height: height, colorSpace: 'srgb' };
        }
      };
      canvas.__chuzz2dContext = context;
      native2dContexts.set(handle, context);
      return context;
    }
    g.__chuzzCreateCanvasContext = function (canvas, kind, options) {
      kind = String(kind).toLowerCase();
      var handle = Number(canvas && canvas.__chuzzCanvasHandle);
      if (!canvas || !safeCanvasInteger(handle) || handle <= 0) { return null; }
      if (kind === 'webgpu') {
        if (!canvas.__chuzzWebGpuContext) {
          canvas.__chuzzWebGpuContext = installCanvasContext({ canvas: canvas, __chuzzCanvasHandle: handle });
          gpuContextsByCanvas.set(handle, canvas.__chuzzWebGpuContext);
        }
        return canvas.__chuzzWebGpuContext;
      }
      if (kind === '2d') { return canvas.__chuzz2dContext || native2dContext(canvas); }
      return null;
    };
    var gpu = {
      getPreferredCanvasFormat: function () { return 'rgba8unorm'; },
      requestAdapter: function (options) { return gpuSend({ webgpu: 'requestAdapter', options: options || {} }).then(function (reply) {
        if (!reply || reply.handle == null) { return null; }
        var adapter = gpuObject('GPUAdapter', 0, reply.handle, {}); adapter.features = features(reply.features || []); adapter.limits = reply.limits || {}; adapter.info = reply.info || {};
        adapter.requestDevice = function (deviceOptions) { return createDevice(adapter, deviceOptions); }; return adapter;
      }); }
    };
    var gpuImageHandles = 1;
    g.createImageBitmap = function (source, sx, sy, sw, sh, bitmapOptions) {
      var crop = null, options = bitmapOptions || {};
      if (arguments.length >= 5 && typeof sx === 'number') {
        crop = [sx, sy, sw, sh];
      } else if (sx && typeof sx === 'object') {
        options = sx;
      }
      var descriptor = {
        source: null,
        crop: crop,
        flipY: options.imageOrientation === 'flipY',
        resizeWidth: options.resizeWidth == null ? null : Number(options.resizeWidth),
        resizeHeight: options.resizeHeight == null ? null : Number(options.resizeHeight),
        resizeQuality: options.resizeQuality || 'low',
        readPixels: false
      };
      var bytes, upload;
      if (source && source._bytes != null) {
        bytes = __chuzzCopyBytes(source._bytes);
        upload = gpuUpload(bytes, 0, bytes.length);
        descriptor.source = { kind: 'encoded', data: upload };
      } else if (source && source.data != null && source.width != null && source.height != null) {
        bytes = __chuzzCopyBytes(source.data);
        upload = gpuUpload(bytes, 0, bytes.length);
        descriptor.source = { kind: 'pixels', width: Number(source.width), height: Number(source.height), data: upload };
      } else {
        return Promise.reject(new TypeError('createImageBitmap source is not supported by the native image bridge'));
      }
      var id = gpuImageHandles++;
      return gpuSend({ webgpu: 'command', device: 0, command: { op: 'createImageBitmap', handle: id, args: descriptor } }, false, [upload]).then(function (reply) {
        if (!reply || reply.handle !== id || !Number.isFinite(reply.width) || !Number.isFinite(reply.height)) { throw new TypeError('invalid native ImageBitmap reply'); }
        var bitmap = {
          width: reply.width,
          height: reply.height,
          __gpuImageSource: { kind: 'bitmap', handle: id },
          close: function () {
            if (!bitmap.__gpuImageSource) { return; }
            gpuSend({ webgpu: 'command', device: 0, command: { op: 'closeImage', handle: id, args: {} } }, true);
            bitmap.__gpuImageSource = null;
          }
        };
        return bitmap;
      });
    };
    g.navigator.gpu = gpu;
    patchCanvasGetContext();
    g.GPUBufferUsage = g.GPUBufferUsage || { MAP_READ:1, MAP_WRITE:2, COPY_SRC:4, COPY_DST:8, INDEX:16, VERTEX:32, UNIFORM:64, STORAGE:128, INDIRECT:256, QUERY_RESOLVE:512 };
    g.GPUTextureUsage = g.GPUTextureUsage || { COPY_SRC:1, COPY_DST:2, TEXTURE_BINDING:4, STORAGE_BINDING:8, RENDER_ATTACHMENT:16 };
    g.GPUShaderStage = g.GPUShaderStage || { VERTEX:1, FRAGMENT:2, COMPUTE:4 };
    g.GPUMapMode = g.GPUMapMode || { READ:1, WRITE:2 };
    g.GPUColorWrite = g.GPUColorWrite || { RED:1, GREEN:2, BLUE:4, ALPHA:8, ALL:15 };
    g.GPUTextureAspect = g.GPUTextureAspect || { ALL:'all', STENCIL_ONLY:'stencil-only', DEPTH_ONLY:'depth-only' };
  }
})(globalThis);"#,
    ),
];

/// Install each bounded subsystem activation on the same document realm.
pub fn install(document: &mut EkojitDocument) -> Result<(), String> {
    for (name, source) in BOOTSTRAPS {
        document
            .eval_source(Arc::new(PageSource {
                text: Arc::from(*source),
                url: Arc::from(format!("chuzz:page-bootstrap/{name}")),
            }))
            .map_err(|error| format!("could not install Ekojit {name} bootstrap: {error}"))?;
    }
    crate::page_ekojit_network::install(document)?;
    Ok(())
}
