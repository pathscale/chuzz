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
        "fetch",
        r#"(function (g) {
  'use strict';
  // The network host reports a record with {ok,status,statusText,url,headers,bytes}
  // or {ok:false,error}. Keep body conversion in this realm and preserve bytes.
  if (typeof __chuzzNetAsync === 'function') {
    function headersObject(entries) {
      var values = Object.create(null);
      (entries || []).forEach(function (pair) {
        var key = String(pair[0]).toLowerCase();
        values[key] = values[key] ? values[key] + ', ' + String(pair[1]) : String(pair[1]);
      });
      return {
        get: function (name) { return values[String(name).toLowerCase()] || null; },
        has: function (name) { return Object.prototype.hasOwnProperty.call(values, String(name).toLowerCase()); },
        forEach: function (callback) { Object.keys(values).forEach(function (key) { callback(values[key], key, this); }, this); }
      };
    }
    function makeResponse(reply) {
      if (!reply || !reply.ok) { throw new TypeError(reply && reply.error || 'Network request failed'); }
      var data = reply.bytes || new Uint8Array(0);
      var used = false;
      var response = {
        ok: reply.status >= 200 && reply.status < 300,
        status: reply.status,
        statusText: reply.statusText || '',
        url: reply.url || '',
        headers: headersObject(reply.headers),
        redirected: false,
        type: 'basic',
        bodyUsed: false,
        arrayBuffer: function () {
          if (used) { return Promise.reject(new TypeError('body already used')); }
          used = true; response.bodyUsed = true;
          var copy = new Uint8Array(data.length); copy.set(data);
          return Promise.resolve(copy.buffer);
        },
        text: function () {
          if (used) { return Promise.reject(new TypeError('body already used')); }
          used = true; response.bodyUsed = true;
          return Promise.resolve(__chuzzDecodeUtf8(data));
        },
        json: function () { return response.text().then(function (text) { return JSON.parse(text); }); },
        blob: function () { return response.arrayBuffer().then(function (buffer) { return new Blob([buffer]); }); },
        clone: function () {
          var copy = new Uint8Array(data.length); copy.set(data);
          return makeResponse({ ok: true, status: reply.status, statusText: reply.statusText, url: reply.url, headers: reply.headers, bytes: copy });
        }
      };
      return response;
    }
    g.fetch = function (input, init) {
      init = init || {};
      var url = input && typeof input === 'object' && input.url !== undefined ? input.url : String(input);
      try { url = new URL(url, g.location && g.location.href || undefined).href; }
      catch (_) { return Promise.reject(new TypeError('invalid request URL')); }
      var headers = [];
      if (Array.isArray(init.headers)) { headers = init.headers; }
      else if (init.headers && typeof init.headers.forEach === 'function') {
        init.headers.forEach(function (value, name) { headers.push([name, value]); });
      } else if (init.headers) { Object.keys(init.headers).forEach(function (name) { headers.push([name, init.headers[name]]); }); }
      var body = init.body == null ? null : init.body;
      if (typeof body === 'string') { body = __chuzzEncodeUtf8(body); }
      else if (body != null) { body = __chuzzCopyBytes(body); }
      var request = { url: url, method: String(init.method || (input && input.method) || 'GET').toUpperCase(), headers: headers, body: body, credentials: init.credentials || 'same-origin' };
      return new Promise(function (resolve, reject) {
        try { __chuzzNetAsync(request, function (reply) { try { resolve(makeResponse(reply)); } catch (error) { reject(error); } }); }
        catch (error) { reject(error); }
      });
    };
  }
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
    function gpuSerialize(value) {
      if (value == null || typeof value !== 'object') { return value; }
      if (value.__gpuHandle != null) { return value.__gpuHandle; }
      if (Array.isArray(value)) { return value.map(gpuSerialize); }
      if (__chuzzIsByteBuffer(value)) { return __chuzzBytesArray(value); }
      var result = {};
      Object.keys(value).forEach(function (key) { if (key.indexOf('__gpu') !== 0) { result[key] = gpuSerialize(value[key]); } });
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
        device.createBuffer = function (desc) { return makeResource(device, 'GPUBuffer', 'createBuffer', desc); };
        device.createTexture = function (desc) { return makeResource(device, 'GPUTexture', 'createTexture', desc); };
        device.createShaderModule = function (desc) { return makeResource(device, 'GPUShaderModule', 'createShaderModule', desc); };
        device.createSampler = function (desc) { return makeResource(device, 'GPUSampler', 'createSampler', desc); };
        device.createBindGroupLayout = function (desc) { return makeResource(device, 'GPUBindGroupLayout', 'createBindGroupLayout', desc); };
        device.createPipelineLayout = function (desc) { return makeResource(device, 'GPUPipelineLayout', 'createPipelineLayout', desc); };
        device.createBindGroup = function (desc) { return makeResource(device, 'GPUBindGroup', 'createBindGroup', desc); };
        device.createRenderPipeline = function (desc) { return makeResource(device, 'GPURenderPipeline', 'createRenderPipeline', desc); };
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
        var upload = __chuzzGpuPutBytes(copy.buffer, start, length);
        gpuSend({ webgpu: 'command', device: device.__gpuHandle, command: { op: 'writeBuffer', handle: buffer.__gpuHandle, args: { offset: offset, data: upload } } }, true, [upload]);
      };
      queue.writeTexture = function (destination, data, layout, size) {
        var bytes = __chuzzCopyBytes(data);
        var copy = new Uint8Array(bytes.length); copy.set(bytes);
        var upload = __chuzzGpuPutBytes(copy.buffer, 0, copy.length);
        var options = { texture: destination.texture.__gpuHandle, mipLevel: destination.mipLevel || 0, origin: destination.origin || [0, 0, 0], aspect: destination.aspect || 'all', data: upload, offset: layout && layout.offset || 0, bytesPerRow: layout && layout.bytesPerRow, rowsPerImage: layout && layout.rowsPerImage, size: size };
        gpuSend({ webgpu: 'command', device: device.__gpuHandle, command: { op: 'writeTexture', handle: 0, args: gpuSerialize(options) } }, true, [upload]);
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
      gpuCommand(device.__gpuHandle, 'createCommandEncoder', id, descriptor, true);
      function encode(op, args) { if (finished) { throw new TypeError('command encoder is finished'); } operations.push({ op: op, args: gpuSerialize(args || {}) }); }
      encoder.beginRenderPass = function (desc) {
        var render = gpuSerialize(desc || {}); render.commands = []; var ended = false;
        operations.push({ op: 'render', args: render });
        var pass = gpuObject('GPURenderPassEncoder', device.__gpuHandle, gpuHandles++, {});
        function passCommand(op, values) { if (ended) { throw new TypeError("render pass has ended"); } render.commands.push({ op: op, args: gpuSerialize(values) }); }
        pass.setPipeline = function (pipeline) { passCommand('pipeline', [pipeline]); };
        pass.setBindGroup = function (index, group, offsets) { passCommand('bindGroup', [index, group, offsets || []]); };
        pass.setVertexBuffer = function (slot, buffer, offset, size) { passCommand('vertexBuffer', [slot, buffer, offset || 0, size]); };
        pass.setIndexBuffer = function (buffer, format, offset, size) { passCommand('indexBuffer', [buffer, format, offset || 0, size]); };
        pass.draw = function (vertexCount, instanceCount, firstVertex, firstInstance) { passCommand('draw', [vertexCount, instanceCount || 1, firstVertex || 0, firstInstance || 0]); };
        pass.drawIndexed = function (indexCount, instanceCount, firstIndex, baseVertex, firstInstance) { passCommand('drawIndexed', [indexCount, instanceCount || 1, firstIndex || 0, baseVertex || 0, firstInstance || 0]); };
        pass.end = function () { if (ended) { throw new TypeError("render pass has ended"); } ended = true; };
        return pass;
      };
      encoder.copyBufferToTexture = function (source, destination, size) { encode('copyBufferToTexture', { source: source, destination: destination, size: size }); };
      encoder.copyTextureToBuffer = function (source, destination, size) { encode('copyTextureToBuffer', { source: source, destination: destination, size: size }); };
      encoder.clearBuffer = function (buffer, offset, size) { encode('clearBuffer', { buffer: buffer, offset: offset || 0, size: size }); };
      encoder.finish = function () { finished = true; var commandId = gpuHandles++; gpuCommand(device.__gpuHandle, 'finish', commandId, { encoder: id, label: descriptor.label || '', operations: operations }, true); return gpuObject('GPUCommandBuffer', device.__gpuHandle, commandId, {}); };
      return encoder;
    }
    var gpuCanvasContexts = [];
    function installCanvasContext(context) {
      if (!context || context.__chuzzGpuWrapped) { return context; }
      var canvas = context.canvas;
      var handle = context.__chuzzCanvasHandle != null ? context.__chuzzCanvasHandle : canvas && canvas.__chuzzCanvasHandle;
      if (handle == null) { return context; }
      context.__chuzzGpuWrapped = true; context.__chuzzCanvasHandle = Number(handle);
      context.configure = function (configuration) {
        configuration = configuration || {};
        var device = configuration.device;
        if (!device || !device.__gpuHandle) { throw new TypeError('GPUCanvasConfiguration.device is required'); }
        context.__chuzzDevice = device; context.__chuzzGeneration = (context.__chuzzGeneration || 0) + 1;
        var width = Number(context.__chuzzCanvasWidth != null ? context.__chuzzCanvasWidth : canvas.width);
        var height = Number(context.__chuzzCanvasHeight != null ? context.__chuzzCanvasHeight : canvas.height);
        if (!Number.isFinite(width) || !Number.isFinite(height) || width < 0 || height < 0) { throw new TypeError('invalid canvas drawing buffer size'); }
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
        var textureId = gpuHandles++, width = Number(context.__chuzzCanvasWidth != null ? context.__chuzzCanvasWidth : canvas.width), height = Number(context.__chuzzCanvasHeight != null ? context.__chuzzCanvasHeight : canvas.height);
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
    var gpu = {
      getPreferredCanvasFormat: function () { return 'rgba8unorm'; },
      requestAdapter: function (options) { return gpuSend({ webgpu: 'requestAdapter', options: options || {} }).then(function (reply) {
        if (!reply || reply.handle == null) { return null; }
        var adapter = gpuObject('GPUAdapter', 0, reply.handle, {}); adapter.features = features(reply.features || []); adapter.limits = reply.limits || {}; adapter.info = reply.info || {};
        adapter.requestDevice = function (deviceOptions) { return createDevice(adapter, deviceOptions); }; return adapter;
      }); }
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
    Ok(())
}
