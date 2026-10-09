// This source uses JavaScript syntax only: include_str! embeds it in the
// browser and Boa evaluates it without a TypeScript compiler.
(function () {
  if (!globalThis.navigator || globalThis.navigator.gpu !== undefined) {
    return;
  }

  var pending = Object.create(null);
  var nextId = 1;
  var adapterHandles = new WeakMap();
  var deviceHandles = new WeakMap();
  var deviceStates = new WeakMap();
  var devices = Object.create(null);
  var queueStates = new WeakMap();
  var bufferStates = new WeakMap();
  var textureStates = new WeakMap();
  var objectStates = new WeakMap();
  var canvasStates = new WeakMap();
  var contextStates = new WeakMap();
  var contexts = Object.create(null);
  var otherContexts = new WeakMap();
  var transfer = ArrayBuffer.prototype.transfer;
  var allocationLimit = 64 * 1024 * 1024;

  function missing(member) {
    throw new Error("not implemented in chuzz: " + member);
  }

  function method(prototype, name, body) {
    Object.defineProperty(prototype, name, {
      value: body,
      writable: true,
      configurable: true
    });
  }

  function unsupportedMethods(prototype, type, names) {
    names.forEach(function (name) {
      method(prototype, name, function () {
        return missing(type + "." + name);
      });
    });
  }

  function unsupportedProperty(prototype, type, name) {
    Object.defineProperty(prototype, name, {
      get: function () { return missing(type + "." + name); },
      set: function () { return missing(type + "." + name); },
      configurable: true
    });
  }

  function readonly(object, name, value) {
    Object.defineProperty(object, name, {
      value: value,
      enumerable: true
    });
  }

  function dictionary(value, member, allowed) {
    if (value === undefined || value === null) { return {}; }
    if (typeof value !== "object") {
      throw new TypeError(member + " must be a dictionary");
    }
    Object.keys(value).forEach(function (name) {
      if (value[name] !== undefined && allowed.indexOf(name) === -1) {
        missing(member + "." + name);
      }
    });
    return value;
  }

  function uint(value, member, fallback, maximum) {
    if (value === undefined && fallback !== undefined) { return fallback; }
    value = Number(value);
    if (!Number.isSafeInteger(value) || value < 0 ||
        value > (maximum === undefined ? 9007199254740991 : maximum)) {
      throw new TypeError("invalid " + member);
    }
    return value;
  }

  function label(value) {
    return value === undefined ? "" : String(value);
  }

  function operationError(message) {
    return new DOMException(message, "OperationError");
  }

  function receiver(map, object, type) {
    var state = map.get(object);
    if (!state) { throw new TypeError("invalid " + type + " receiver"); }
    return state;
  }

  function send(operation, fields) {
    return new Promise(function (resolve, reject) {
      var id = nextId++;
      // The host poll loop owns request deadlines. A page timer here becomes
      // script-pump work and can hold document load while host replies wait
      // for that same document to poll.
      pending[id] = { resolve: resolve, reject: reject };
      fields.webgpu = operation;
      fields.id = id;
      try {
        window.ipc.postMessage(JSON.stringify(fields));
      } catch (error) {
        delete pending[id];
        reject(error);
      }
    });
  }

  globalThis.__chuzzGpuResolve = function (id, result) {
    var entry = pending[id];
    if (!entry) { return; }
    delete pending[id];
    if (result.ok) { entry.resolve(result.value); }
    else {
      var error = new Error(result.error);
      error.kind = result.kind;
      error.reported = !!result.reported;
      entry.reject(error);
    }
  };

  function command(device, op, handle, args) {
    return send("command", {
      device: device.handle,
      command: {
        op: op,
        handle: handle || 0,
        args: args === undefined ? null : args
      }
    });
  }

  function fire(device, op, handle, args) {
    command(device, op, handle, args).catch(function (error) {
      // Native validation was already routed to the matching page scope.
      // Transport failures have no native event and must be observable.
      if (!error.reported) {
        dispatch(device, new GPUUncapturedErrorEvent("uncapturederror", {
          error: new GPUInternalError(error.message)
        }));
      }
    });
  }

  function GPU() { throw new TypeError("Illegal constructor: GPU"); }
  function GPUAdapter() { throw new TypeError("Illegal constructor: GPUAdapter"); }
  function GPUDevice() { throw new TypeError("Illegal constructor: GPUDevice"); }
  function GPUQueue() { throw new TypeError("Illegal constructor: GPUQueue"); }
  function GPUBuffer() { throw new TypeError("Illegal constructor: GPUBuffer"); }
  function GPUTexture() { throw new TypeError("Illegal constructor: GPUTexture"); }
  function GPUTextureView() { throw new TypeError("Illegal constructor: GPUTextureView"); }
  function GPUSampler() { throw new TypeError("Illegal constructor: GPUSampler"); }
  function GPUCanvasContext() { throw new TypeError("Illegal constructor: GPUCanvasContext"); }
  function GPUDeviceLostInfo() { throw new TypeError("Illegal constructor: GPUDeviceLostInfo"); }
  function GPUError() { throw new TypeError("Illegal constructor: GPUError"); }
  function GPUValidationError(message) {
    if (!(this instanceof GPUValidationError)) { throw new TypeError("Illegal constructor"); }
    readonly(this, "message", String(message));
  }
  function GPUOutOfMemoryError(message) {
    if (!(this instanceof GPUOutOfMemoryError)) { throw new TypeError("Illegal constructor"); }
    readonly(this, "message", String(message));
  }
  function GPUInternalError(message) {
    if (!(this instanceof GPUInternalError)) { throw new TypeError("Illegal constructor"); }
    readonly(this, "message", String(message));
  }
  [GPUValidationError, GPUOutOfMemoryError, GPUInternalError].forEach(function (type) {
    Object.setPrototypeOf(type.prototype, GPUError.prototype);
  });
  function GPUUncapturedErrorEvent(type, init) {
    if (!(this instanceof GPUUncapturedErrorEvent)) {
      throw new TypeError("Illegal constructor");
    }
    if (!init || !(init.error instanceof GPUError)) {
      throw new TypeError("GPUUncapturedErrorEvent requires a GPUError");
    }
    var event = new Event(String(type), {
      bubbles: !!init.bubbles,
      cancelable: !!init.cancelable
    });
    Object.setPrototypeOf(event, GPUUncapturedErrorEvent.prototype);
    readonly(event, "error", init.error);
    return event;
  }
  Object.setPrototypeOf(GPUUncapturedErrorEvent.prototype, Event.prototype);
  function GPUSupportedFeatures() {
    throw new TypeError("Illegal constructor: GPUSupportedFeatures");
  }
  function GPUSupportedLimits() {
    throw new TypeError("Illegal constructor: GPUSupportedLimits");
  }

  function gpuError(value) {
    var type = value.kind === "out-of-memory" ? GPUOutOfMemoryError
      : value.kind === "internal" ? GPUInternalError : GPUValidationError;
    return new type(value.message);
  }

  function invoke(callback, device, event) {
    try {
      if (typeof callback === "function") { callback.call(device, event); }
      else if (callback && typeof callback.handleEvent === "function") {
        callback.handleEvent(event);
      }
    } catch (error) {
      setTimeout(function () { throw error; }, 0);
    }
  }

  function dispatch(state, event) {
    if (!event || typeof event.type !== "string") {
      throw new TypeError("GPUDevice.dispatchEvent requires an Event");
    }
    var stopped = false;
    var stop = event.stopImmediatePropagation;
    Object.defineProperty(event, "target", { value: state.object, configurable: true });
    Object.defineProperty(event, "currentTarget", { value: state.object, configurable: true });
    Object.defineProperty(event, "stopImmediatePropagation", {
      configurable: true,
      value: function () {
        stopped = true;
        if (stop) { stop.call(event); }
      }
    });
    var listeners = (state.listeners[event.type] || []).slice();
    listeners.forEach(function (entry) {
      if (stopped || (state.listeners[event.type] || []).indexOf(entry) === -1) { return; }
      if (entry.once) {
        state.object.removeEventListener(event.type, entry.callback, entry.capture);
      }
      invoke(entry.callback, state.object, event);
    });
    if (!stopped && event.type === "uncapturederror" && state.onuncapturederror) {
      invoke(state.onuncapturederror, state.object, event);
    }
    Object.defineProperty(event, "currentTarget", { value: null, configurable: true });
    return !event.defaultPrevented;
  }

  function detachRanges(state) {
    state.ranges.forEach(function (range) {
      if (range.buffer.byteLength && typeof transfer === "function") {
        transfer.call(range.buffer, 0);
      }
    });
    state.ranges = [];
    state.bytes = null;
  }

  function cancelMapping(state) {
    state.epoch++;
    if (state.rejectMap) {
      state.rejectMap(new DOMException("GPUBuffer mapping was cancelled", "AbortError"));
      state.rejectMap = null;
    }
    detachRanges(state);
    state.mapState = "unmapped";
  }

  function lose(state, reason, message) {
    if (state.lostSettled) { return; }
    state.lostSettled = true;
    state.destroyed = true;
    state.buffers.forEach(function (buffer) {
      cancelMapping(bufferStates.get(buffer));
    });
    var info = Object.create(GPUDeviceLostInfo.prototype);
    readonly(info, "reason", reason);
    readonly(info, "message", message);
    state.resolveLost(info);
  }

  globalThis.__chuzzGpuEvent = function (handle, value) {
    var state = devices[handle];
    if (!state) { return; }
    if (value.type === "lost") { lose(state, value.reason, value.message); }
    else if (value.type === "error") {
      dispatch(state, new GPUUncapturedErrorEvent("uncapturederror", {
        error: gpuError(value.error),
        cancelable: true
      }));
    }
  };

  function features(values) {
    var set = new Set(values);
    var object = Object.create(GPUSupportedFeatures.prototype);
    readonly(object, "size", set.size);
    readonly(object, "has", function (value) { return set.has(value); });
    readonly(object, "keys", function () { return set.keys(); });
    readonly(object, "values", function () { return set.values(); });
    readonly(object, "entries", function () { return set.entries(); });
    readonly(object, Symbol.iterator, function () { return set.values(); });
    readonly(object, "forEach", function (callback, receiver) {
      set.forEach(function (value) {
        callback.call(receiver, value, value, object);
      });
    });
    return Object.freeze(object);
  }

  function limits(values) {
    var object = Object.create(GPUSupportedLimits.prototype);
    var names = Object.keys(values);
    names.forEach(function (name) {
      readonly(object, name, values[name]);
    });
    method(object, "keys", function () { return names[Symbol.iterator](); });
    method(object, "values", function () {
      return names.map(function (name) { return values[name]; })[Symbol.iterator]();
    });
    method(object, "entries", function () {
      return names.map(function (name) { return [name, values[name]]; })[Symbol.iterator]();
    });
    method(object, Symbol.iterator, object.entries);
    return Object.freeze(object);
  }

  function makeAdapter(value) {
    if (value === null) { return null; }
    var object = Object.create(GPUAdapter.prototype);
    adapterHandles.set(object, value.handle);
    readonly(object, "features", features(value.features));
    readonly(object, "limits", limits(value.limits));
    readonly(object, "isFallbackAdapter", value.isFallbackAdapter);
    readonly(object, "info", Object.freeze(value.info));
    return object;
  }

  function makeDevice(value) {
    var object = Object.create(GPUDevice.prototype);
    var state = {
      object: object,
      handle: value.handle,
      destroyed: false,
      lostSettled: false,
      listeners: Object.create(null),
      onuncapturederror: null,
      buffers: new Set()
    };
    state.lost = new Promise(function (resolve) { state.resolveLost = resolve; });
    deviceStates.set(object, state);
    devices[value.handle] = state;
    deviceHandles.set(object, value.handle);
    var queue = Object.create(GPUQueue.prototype);
    queueStates.set(queue, state);
    queue.label = "";
    readonly(object, "features", features(value.features));
    readonly(object, "limits", limits(value.limits));
    readonly(object, "queue", queue);
    readonly(object, "adapterInfo", Object.freeze(value.info));
    readonly(object, "label", value.label);
    return object;
  }

  method(GPU.prototype, "requestAdapter", function (input) {
    try {
      var options = dictionary(input, "GPURequestAdapterOptions", [
        "powerPreference", "forceFallbackAdapter"
      ]);
      var preference = options.powerPreference;
      if (preference !== undefined &&
          preference !== "low-power" && preference !== "high-performance") {
        throw new TypeError("invalid GPURequestAdapterOptions.powerPreference");
      }
      return send("requestAdapter", {
        options: {
          powerPreference: preference,
          forceFallbackAdapter: !!options.forceFallbackAdapter
        }
      }).then(makeAdapter);
    } catch (error) {
      return Promise.reject(error);
    }
  });

  method(GPU.prototype, "getPreferredCanvasFormat", function () {
    return "rgba8unorm";
  });
  unsupportedProperty(GPU.prototype, "GPU", "wgslLanguageFeatures");

  method(GPUAdapter.prototype, "requestDevice", function (input) {
    try {
      if (!adapterHandles.has(this)) {
        throw new TypeError("GPUAdapter.requestDevice called on an invalid receiver");
      }
      var options = dictionary(input, "GPUDeviceDescriptor", [
        "label", "requiredFeatures", "requiredLimits", "defaultQueue"
      ]);
      if (options.defaultQueue !== undefined) {
        missing("GPUDeviceDescriptor.defaultQueue");
      }
      var requiredFeatures = options.requiredFeatures === undefined
        ? [] : Array.from(options.requiredFeatures, String);
      var requiredLimits = options.requiredLimits === undefined
        ? {} : options.requiredLimits;
      if (requiredLimits === null || typeof requiredLimits !== "object") {
        throw new TypeError("GPUDeviceDescriptor.requiredLimits must be a dictionary");
      }
      var copiedLimits = Object.create(null);
      Object.keys(requiredLimits).forEach(function (name) {
        var value = requiredLimits[name];
        if (value === undefined) { return; }
        if (!Number.isSafeInteger(value) || value < 0) {
          throw new TypeError("invalid WebGPU limit: " + name);
        }
        copiedLimits[name] = value;
      });
      return send("requestDevice", {
        adapter: adapterHandles.get(this),
        options: {
          label: options.label === undefined ? "" : String(options.label),
          requiredFeatures: requiredFeatures,
          requiredLimits: copiedLimits
        }
      }).then(makeDevice);
    } catch (error) {
      return Promise.reject(error);
    }
  });

  method(GPUAdapter.prototype, "requestAdapterInfo", function () {
    if (!adapterHandles.has(this)) { return Promise.reject(new TypeError("invalid GPUAdapter")); }
    return Promise.resolve(this.info);
  });

  Object.defineProperty(GPUDevice.prototype, "lost", {
    get: function () { return receiver(deviceStates, this, "GPUDevice").lost; },
    configurable: true
  });
  Object.defineProperty(GPUDevice.prototype, "onuncapturederror", {
    get: function () {
      return receiver(deviceStates, this, "GPUDevice").onuncapturederror;
    },
    set: function (value) {
      receiver(deviceStates, this, "GPUDevice").onuncapturederror =
        typeof value === "function" ? value : null;
    },
    configurable: true
  });
  method(GPUDevice.prototype, "destroy", function () {
    var state = receiver(deviceStates, this, "GPUDevice");
    if (state.destroyed) { return; }
    fire(state, "destroy");
    lose(state, "destroyed", "GPUDevice.destroy() was called");
  });
  method(GPUDevice.prototype, "pushErrorScope", function (filter) {
    var state = receiver(deviceStates, this, "GPUDevice");
    filter = String(filter);
    if (["validation", "out-of-memory", "internal"].indexOf(filter) === -1) {
      throw new TypeError("invalid GPUErrorFilter");
    }
    fire(state, "pushErrorScope", 0, filter);
  });
  method(GPUDevice.prototype, "popErrorScope", function () {
    var state = receiver(deviceStates, this, "GPUDevice");
    return command(state, "popErrorScope").then(function (value) {
      return value === null ? null : gpuError(value);
    }, function (error) {
      throw operationError(error.message);
    });
  });
  method(GPUDevice.prototype, "addEventListener", function (type, callback, options) {
    var state = receiver(deviceStates, this, "GPUDevice");
    if (callback === null || callback === undefined) { return; }
    type = String(type);
    var capture = typeof options === "boolean" ? options : !!(options && options.capture);
    var listeners = state.listeners[type] || (state.listeners[type] = []);
    if (listeners.some(function (entry) {
      return entry.callback === callback && entry.capture === capture;
    })) { return; }
    if (options && options.signal && options.signal.aborted) { return; }
    listeners.push({
      callback: callback,
      capture: capture,
      once: !!(options && options.once)
    });
    if (options && options.signal) {
      var self = this;
      options.signal.addEventListener("abort", function () {
        self.removeEventListener(type, callback, capture);
      }, { once: true });
    }
  });
  method(GPUDevice.prototype, "removeEventListener", function (type, callback, options) {
    var state = receiver(deviceStates, this, "GPUDevice");
    type = String(type);
    var capture = typeof options === "boolean" ? options : !!(options && options.capture);
    state.listeners[type] = (state.listeners[type] || []).filter(function (entry) {
      return entry.callback !== callback || entry.capture !== capture;
    });
  });
  method(GPUDevice.prototype, "dispatchEvent", function (event) {
    return dispatch(receiver(deviceStates, this, "GPUDevice"), event);
  });

  function byteView(data) {
    if (data instanceof ArrayBuffer) { return new Uint8Array(data); }
    if (ArrayBuffer.isView(data)) {
      return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
    }
    throw new TypeError("WebGPU data must be an ArrayBuffer or an ArrayBufferView");
  }

  function copyBytes(data, offset, size, elements) {
    var view = byteView(data);
    var unit = elements && data.BYTES_PER_ELEMENT ? data.BYTES_PER_ELEMENT : 1;
    offset = uint(offset, "dataOffset", 0) * unit;
    size = size === undefined ? view.byteLength - offset : uint(size, "size") * unit;
    if (!Number.isSafeInteger(offset) || !Number.isSafeInteger(size) ||
        size < 0 || offset > view.byteLength || size > view.byteLength - offset) {
      throw operationError("WebGPU data range is out of bounds");
    }
    if (size > allocationLimit) {
      throw operationError("WebGPU transfer limit reached in chuzz");
    }
    return Array.from(new Uint8Array(view.buffer, view.byteOffset + offset, size));
  }

  method(GPUDevice.prototype, "createBuffer", function (input) {
    var device = receiver(deviceStates, this, "GPUDevice");
    var options = dictionary(input, "GPUBufferDescriptor", [
      "label", "size", "usage", "mappedAtCreation"
    ]);
    var size = uint(options.size, "GPUBufferDescriptor.size");
    if (size > allocationLimit) {
      throw new RangeError("WebGPU allocation limit reached in chuzz");
    }
    var usage = uint(options.usage, "GPUBufferDescriptor.usage", undefined, 4294967295);
    var object = Object.create(GPUBuffer.prototype);
    var state = {
      handle: nextId++,
      object: object,
      device: device,
      size: size,
      usage: usage,
      destroyed: false,
      epoch: 0,
      mapState: options.mappedAtCreation ? "mapped" : "unmapped",
      writable: !!options.mappedAtCreation,
      mapOffset: 0,
      mapSize: size,
      bytes: options.mappedAtCreation ? new Uint8Array(size) : null,
      ranges: [],
      rejectMap: null
    };
    bufferStates.set(object, state);
    device.buffers.add(object);
    readonly(object, "size", size);
    readonly(object, "usage", usage);
    object.label = label(options.label);
    fire(device, "createBuffer", state.handle, {
      label: object.label,
      size: size,
      usage: usage,
      mappedAtCreation: !!options.mappedAtCreation
    });
    return object;
  });

  Object.defineProperty(GPUBuffer.prototype, "mapState", {
    get: function () { return receiver(bufferStates, this, "GPUBuffer").mapState; },
    configurable: true
  });
  method(GPUBuffer.prototype, "mapAsync", function (mode, offset, size) {
    var state = receiver(bufferStates, this, "GPUBuffer");
    try {
      if (mode !== 1) { missing("GPUBuffer.mapAsync WRITE"); }
      offset = uint(offset, "GPUBuffer.mapAsync.offset", 0);
      size = uint(size, "GPUBuffer.mapAsync.size", state.size - offset);
      if (state.destroyed || state.device.destroyed || state.mapState !== "unmapped") {
        throw operationError("GPUBuffer is unavailable for mapping");
      }
      if (offset % 8 !== 0 || size === 0 || size % 4 !== 0 ||
          offset > state.size || size > state.size - offset) {
        throw operationError("invalid GPUBuffer mapping range");
      }
      state.mapState = "pending";
      var epoch = ++state.epoch;
      return new Promise(function (resolve, reject) {
        state.rejectMap = reject;
        command(state.device, "mapRead", state.handle, {
          offset: offset, size: size
        }).then(function (bytes) {
          if (state.epoch !== epoch) { return; }
          state.rejectMap = null;
          if (state.destroyed || state.device.destroyed) {
            state.mapState = "unmapped";
            reject(new DOMException("GPUBuffer mapping was cancelled", "AbortError"));
            return;
          }
          state.bytes = new Uint8Array(bytes);
          state.mapOffset = offset;
          state.mapSize = size;
          state.writable = false;
          state.mapState = "mapped";
          resolve();
        }, function (error) {
          if (state.epoch !== epoch) { return; }
          state.rejectMap = null;
          state.mapState = "unmapped";
          reject(operationError(error.message));
        });
      });
    } catch (error) {
      return Promise.reject(error);
    }
  });
  method(GPUBuffer.prototype, "getMappedRange", function (offset, size) {
    var state = receiver(bufferStates, this, "GPUBuffer");
    if (typeof transfer !== "function") {
      missing("GPUBuffer.getMappedRange (ArrayBuffer detachment)");
    }
    offset = uint(offset, "GPUBuffer.getMappedRange.offset", 0);
    size = uint(size, "GPUBuffer.getMappedRange.size", state.size - offset);
    if (state.destroyed || state.mapState !== "mapped" ||
        offset % 8 !== 0 || size % 4 !== 0 ||
        offset < state.mapOffset ||
        offset > state.mapOffset + state.mapSize ||
        size > state.mapOffset + state.mapSize - offset) {
      throw operationError("invalid GPUBuffer mapped range");
    }
    if (state.ranges.some(function (range) {
      return offset < range.offset + range.size && range.offset < offset + size;
    })) {
      throw operationError("GPUBuffer mapped ranges overlap");
    }
    var buffer = state.bytes.slice(
      offset - state.mapOffset, offset - state.mapOffset + size
    ).buffer;
    state.ranges.push({ offset: offset, size: size, buffer: buffer });
    return buffer;
  });
  method(GPUBuffer.prototype, "unmap", function () {
    var state = receiver(bufferStates, this, "GPUBuffer");
    if (state.destroyed) { return; }
    var ranges = state.writable && state.mapState === "mapped"
      ? state.ranges.map(function (range) {
        return { offset: range.offset, data: Array.from(new Uint8Array(range.buffer)) };
      }) : [];
    cancelMapping(state);
    state.writable = false;
    fire(state.device, "unmap", state.handle, { ranges: ranges });
  });
  method(GPUBuffer.prototype, "destroy", function () {
    var state = receiver(bufferStates, this, "GPUBuffer");
    if (state.destroyed) { return; }
    state.destroyed = true;
    cancelMapping(state);
    state.device.buffers.delete(this);
    fire(state.device, "destroyBuffer", state.handle);
  });
  method(GPUQueue.prototype, "writeBuffer", function (buffer, offset, data, dataOffset, size) {
    var device = receiver(queueStates, this, "GPUQueue");
    var target = receiver(bufferStates, buffer, "GPUBuffer");
    if (target.device !== device) {
      throw new TypeError("GPUBuffer belongs to another GPUDevice");
    }
    var bytes = copyBytes(data, dataOffset, size, true);
    if (bytes.length % 4 !== 0) {
      throw operationError("GPUQueue.writeBuffer data size must be a multiple of 4");
    }
    fire(device, "writeBuffer", target.handle, {
      offset: uint(offset, "GPUQueue.writeBuffer.bufferOffset"),
      data: bytes
    });
  });

  function extent(input, member) {
    var sequence = Array.isArray(input) ||
      (input && typeof input[Symbol.iterator] === "function");
    if (sequence) {
      var values = Array.from(input);
      if (values.length < 1 || values.length > 3) {
        throw new TypeError("invalid " + member);
      }
      return [
        uint(values[0], member + ".width", undefined, 4294967295),
        uint(values[1], member + ".height", 1, 4294967295),
        uint(values[2], member + ".depthOrArrayLayers", 1, 4294967295)
      ];
    }
    var value = dictionary(input, member, ["width", "height", "depthOrArrayLayers"]);
    return [
      uint(value.width, member + ".width", undefined, 4294967295),
      uint(value.height, member + ".height", 1, 4294967295),
      uint(value.depthOrArrayLayers, member + ".depthOrArrayLayers", 1, 4294967295)
    ];
  }

  function origin(input) {
    if (input === undefined) { return [0, 0, 0]; }
    var sequence = Array.isArray(input) ||
      (input && typeof input[Symbol.iterator] === "function");
    var value = sequence ? Array.from(input)
      : dictionary(input, "GPUOrigin3D", ["x", "y", "z"]);
    if (sequence && value.length > 3) { throw new TypeError("invalid GPUOrigin3D"); }
    return [
      uint(sequence ? value[0] : value.x, "GPUOrigin3D.x", 0, 4294967295),
      uint(sequence ? value[1] : value.y, "GPUOrigin3D.y", 0, 4294967295),
      uint(sequence ? value[2] : value.z, "GPUOrigin3D.z", 0, 4294967295)
    ];
  }

  function textureOptions(input) {
    var value = dictionary(input, "GPUTextureDescriptor", [
      "label", "size", "mipLevelCount", "sampleCount", "dimension",
      "format", "usage", "viewFormats"
    ]);
    if (value.format === undefined) { throw new TypeError("GPUTextureDescriptor.format is required"); }
    return {
      label: label(value.label),
      size: extent(value.size, "GPUExtent3D"),
      mipLevelCount: uint(value.mipLevelCount, "mipLevelCount", 1, 4294967295),
      sampleCount: uint(value.sampleCount, "sampleCount", 1, 4294967295),
      dimension: value.dimension === undefined ? "2d" : String(value.dimension),
      format: String(value.format),
      usage: uint(value.usage, "GPUTextureDescriptor.usage", undefined, 4294967295),
      viewFormats: value.viewFormats === undefined ? [] : Array.from(value.viewFormats, String)
    };
  }

  function makeTexture(device, options, handle) {
    var object = Object.create(GPUTexture.prototype);
    textureStates.set(object, {
      handle: handle,
      device: device,
      destroyed: false,
      canvas: null
    });
    object.label = options.label;
    readonly(object, "width", options.size[0]);
    readonly(object, "height", options.size[1]);
    readonly(object, "depthOrArrayLayers", options.size[2]);
    readonly(object, "mipLevelCount", options.mipLevelCount);
    readonly(object, "sampleCount", options.sampleCount);
    readonly(object, "dimension", options.dimension);
    readonly(object, "format", options.format);
    readonly(object, "usage", options.usage);
    return object;
  }

  method(GPUDevice.prototype, "createTexture", function (input) {
    var device = receiver(deviceStates, this, "GPUDevice");
    var options = textureOptions(input);
    var handle = nextId++;
    var object = makeTexture(device, options, handle);
    fire(device, "createTexture", handle, options);
    return object;
  });
  method(GPUTexture.prototype, "createView", function (input) {
    var state = receiver(textureStates, this, "GPUTexture");
    var value = dictionary(input, "GPUTextureViewDescriptor", [
      "label", "format", "dimension", "aspect", "baseMipLevel",
      "mipLevelCount", "baseArrayLayer", "arrayLayerCount"
    ]);
    var descriptor = { label: label(value.label) };
    ["format", "dimension", "aspect"].forEach(function (name) {
      if (value[name] !== undefined) { descriptor[name] = String(value[name]); }
    });
    ["baseMipLevel", "mipLevelCount", "baseArrayLayer", "arrayLayerCount"].forEach(function (name) {
      if (value[name] !== undefined) {
        descriptor[name] = uint(value[name], name, undefined, 4294967295);
      }
    });
    var handle = nextId++;
    var object = makeGpuObject(GPUTextureView, "GPUTextureView", state.device, handle, descriptor.label);
    objectStates.get(object).texture = state;
    fire(state.device, "createView", handle, {
      texture: state.handle,
      descriptor: descriptor
    });
    return object;
  });
  method(GPUTexture.prototype, "destroy", function () {
    var state = receiver(textureStates, this, "GPUTexture");
    if (state.destroyed) { return; }
    state.destroyed = true;
    fire(state.device, "destroyTexture", state.handle);
  });
  method(GPUQueue.prototype, "writeTexture", function (destination, data, layout, size) {
    var device = receiver(queueStates, this, "GPUQueue");
    var target = dictionary(destination, "GPUTexelCopyTextureInfo", [
      "texture", "mipLevel", "origin", "aspect"
    ]);
    var texture = receiver(textureStates, target.texture, "GPUTexture");
    if (texture.device !== device) {
      throw new TypeError("GPUTexture belongs to another GPUDevice");
    }
    var source = dictionary(layout, "GPUTexelCopyBufferLayout", [
      "offset", "bytesPerRow", "rowsPerImage"
    ]);
    var offset = uint(source.offset, "offset", 0);
    var view = byteView(data);
    if (offset > view.byteLength) {
      throw operationError("GPUQueue.writeTexture offset is out of bounds");
    }
    // Offset is measured from this view, including for a DataView or a
    // subarray. Drop unused leading bytes before crossing the IPC boundary.
    fire(device, "writeTexture", 0, {
      texture: texture.handle,
      mipLevel: uint(target.mipLevel, "mipLevel", 0, 4294967295),
      origin: origin(target.origin),
      aspect: target.aspect === undefined ? "all" : String(target.aspect),
      data: copyBytes(view, offset, undefined, false),
      offset: 0,
      bytesPerRow: source.bytesPerRow === undefined ? null
        : uint(source.bytesPerRow, "bytesPerRow", undefined, 4294967295),
      rowsPerImage: source.rowsPerImage === undefined ? null
        : uint(source.rowsPerImage, "rowsPerImage", undefined, 4294967295),
      size: extent(size, "GPUExtent3D")
    });
  });
  method(GPUDevice.prototype, "createSampler", function (input) {
    var device = receiver(deviceStates, this, "GPUDevice");
    var value = dictionary(input, "GPUSamplerDescriptor", [
      "label", "addressModeU", "addressModeV", "addressModeW",
      "magFilter", "minFilter", "mipmapFilter",
      "lodMinClamp", "lodMaxClamp", "compare", "maxAnisotropy"
    ]);
    var descriptor = { label: label(value.label) };
    ["addressModeU", "addressModeV", "addressModeW", "magFilter",
      "minFilter", "mipmapFilter", "compare"].forEach(function (name) {
      if (value[name] !== undefined) { descriptor[name] = String(value[name]); }
    });
    ["lodMinClamp", "lodMaxClamp"].forEach(function (name) {
      if (value[name] !== undefined) {
        var number = Number(value[name]);
        if (!Number.isFinite(number)) { throw new TypeError("invalid " + name); }
        descriptor[name] = number;
      }
    });
    if (value.maxAnisotropy !== undefined) {
      descriptor.maxAnisotropy = uint(value.maxAnisotropy, "maxAnisotropy", undefined, 65535);
    }
    var handle = nextId++;
    var object = makeGpuObject(GPUSampler, "GPUSampler", device, handle, descriptor.label);
    fire(device, "createSampler", handle, descriptor);
    return object;
  });

  function canvasSize(canvas) {
    function dimension(name, fallback) {
      var value = canvas[name];
      if (value === undefined) {
        var attribute = canvas.getAttribute(name);
        value = attribute === null ? fallback : Number(attribute);
      }
      return uint(value, "HTMLCanvasElement." + name, fallback, 4294967295);
    }
    return [dimension("width", 300), dimension("height", 150)];
  }

  function expireCurrent(state, destroy) {
    state.presentable = false;
    if (!state.current) { return; }
    var texture = textureStates.get(state.current);
    texture.destroyed = true;
    if (destroy) { fire(texture.device, "destroyTexture", texture.handle); }
    state.current = null;
  }

  function configureNativeCanvas(state) {
    var size = canvasSize(state.canvas);
    state.size = size;
    fire(state.device, "configure", state.handle, {
      generation: state.generation,
      format: state.configuration.format,
      usage: state.configuration.usage,
      alphaMode: state.configuration.alphaMode,
      viewFormats: state.configuration.viewFormats,
      width: size[0],
      height: size[1]
    });
  }

  function resizeCanvas(state, force) {
    if (!state || !state.configuration || !state.device) { return; }
    var size = canvasSize(state.canvas);
    if (!force && state.size && size[0] === state.size[0] && size[1] === state.size[1]) {
      return;
    }
    expireCurrent(state, true);
    state.generation++;
    configureNativeCanvas(state);
  }

  var reflectedCanvases = new WeakSet();
  function reflectCanvas(canvas) {
    if (reflectedCanvases.has(canvas)) { return; }
    reflectedCanvases.add(canvas);
    ["width", "height"].forEach(function (name) {
      var fallback = name === "width" ? 300 : 150;
      var initial = canvas[name];
      if (initial !== undefined && canvas.getAttribute(name) === null) {
        canvas.setAttribute(name, String(uint(initial, name, fallback, 4294967295)));
      }
      Object.defineProperty(canvas, name, {
        configurable: true,
        enumerable: true,
        get: function () {
          var value = canvas.getAttribute(name);
          return value === null ? fallback : uint(Number(value), name, fallback, 4294967295);
        },
        set: function (value) {
          canvas.setAttribute(name, String(uint(value, name, undefined, 4294967295)));
          resizeCanvas(contextStates.get(canvasStates.get(canvas)), true);
          if (globalThis.__chuzzGpuResize2D) { globalThis.__chuzzGpuResize2D(canvas); }
        }
      });
      if (canvas.getAttribute(name) === null) { canvas.setAttribute(name, String(fallback)); }
    });
  }

  method(GPUCanvasContext.prototype, "configure", function (input) {
    var state = receiver(contextStates, this, "GPUCanvasContext");
    var value = dictionary(input, "GPUCanvasConfiguration", [
      "device", "format", "usage", "viewFormats", "colorSpace",
      "toneMapping", "alphaMode"
    ]);
    var device = receiver(deviceStates, value.device, "GPUDevice");
    if (value.colorSpace !== undefined && value.colorSpace !== "srgb") {
      missing("GPUCanvasConfiguration.colorSpace." + value.colorSpace);
    }
    if (value.toneMapping !== undefined) { missing("GPUCanvasConfiguration.toneMapping"); }
    var format = String(value.format);
    if (format !== "rgba8unorm" && format !== "bgra8unorm") {
      missing("GPUCanvasConfiguration.format." + format);
    }
    var alpha = value.alphaMode === undefined ? "opaque" : String(value.alphaMode);
    if (alpha !== "opaque" && alpha !== "premultiplied") {
      throw new TypeError("invalid GPUCanvasAlphaMode");
    }
    var usage = uint(value.usage, "GPUCanvasConfiguration.usage", 16, 4294967295);
    var views = value.viewFormats === undefined ? [] : Array.from(value.viewFormats, String);
    expireCurrent(state, true);
    state.generation++;
    if (state.device) {
      fire(state.device, "unconfigure", state.handle, state.generation);
    }
    state.device = device;
    state.configuration = {
      device: value.device,
      format: format,
      usage: usage,
      alphaMode: alpha,
      viewFormats: views,
      colorSpace: "srgb"
    };
    configureNativeCanvas(state);
  });
  method(GPUCanvasContext.prototype, "unconfigure", function () {
    var state = receiver(contextStates, this, "GPUCanvasContext");
    expireCurrent(state, true);
    state.generation++;
    if (state.device) {
      fire(state.device, "unconfigure", state.handle, state.generation);
    }
    state.device = null;
    state.configuration = null;
  });
  method(GPUCanvasContext.prototype, "getCurrentTexture", function () {
    var state = receiver(contextStates, this, "GPUCanvasContext");
    if (!state.configuration || !state.device || state.device.destroyed) {
      throw new DOMException("GPUCanvasContext is not configured", "InvalidStateError");
    }
    resizeCanvas(state, false);
    var size = canvasSize(state.canvas);
    if (size[0] === 0 || size[1] === 0) {
      throw operationError("GPUCanvasContext has an empty drawing buffer");
    }
    if (state.current &&
        (state.current.width !== size[0] || state.current.height !== size[1])) {
      expireCurrent(state, true);
    }
    if (state.current) { return state.current; }
    var handle = nextId++;
    state.current = makeTexture(state.device, {
      label: "chuzz canvas current texture",
      size: [size[0], size[1], 1],
      mipLevelCount: 1,
      sampleCount: 1,
      dimension: "2d",
      format: state.configuration.format,
      usage: state.configuration.usage
    }, handle);
    textureStates.get(state.current).canvas = state;
    state.presentable = false;
    fire(state.device, "currentTexture", handle, {
      canvas: state.handle,
      generation: state.generation,
      width: size[0],
      height: size[1]
    });
    return state.current;
  });
  method(GPUCanvasContext.prototype, "getConfiguration", function () {
    var state = receiver(contextStates, this, "GPUCanvasContext");
    if (!state.configuration) { return null; }
    var value = Object.assign({}, state.configuration);
    value.viewFormats = value.viewFormats.slice();
    return value;
  });

  globalThis.__chuzzGpuCanvasLive = function (handle, generation, width, height) {
    var state = contexts[handle];
    if (!state || state.generation !== generation) { return false; }
    if (width === 0 && height === 0) { return true; }
    if (!state.configuration || !state.device || state.device.destroyed) { return false; }
    var size = canvasSize(state.canvas);
    return size[0] === width && size[1] === height;
  };
  globalThis.__chuzzGpuFlushCanvases = function () {
    Object.keys(contexts).forEach(function (handle) {
      var state = contexts[handle];
      resizeCanvas(state, false);
      if (!state.current || !state.presentable) { return; }
      var texture = textureStates.get(state.current);
      if (texture.destroyed || !state.device || state.device.destroyed) {
        expireCurrent(state, true);
        return;
      }
      var currentHandle = texture.handle;
      expireCurrent(state, false);
      fire(state.device, "present", currentHandle, {
        canvas: state.handle,
        generation: state.generation
      });
    });
  };

  function gpuType(name) {
    return function () { throw new TypeError("Illegal constructor: " + name); };
  }
  var GPUShaderModule = gpuType("GPUShaderModule");
  var GPUCompilationInfo = gpuType("GPUCompilationInfo");
  var GPUCompilationMessage = gpuType("GPUCompilationMessage");
  var GPUBindGroupLayout = gpuType("GPUBindGroupLayout");
  var GPUPipelineLayout = gpuType("GPUPipelineLayout");
  var GPUBindGroup = gpuType("GPUBindGroup");
  var GPURenderPipeline = gpuType("GPURenderPipeline");
  var GPUComputePipeline = gpuType("GPUComputePipeline");
  var GPUCommandEncoder = gpuType("GPUCommandEncoder");
  var GPUCommandBuffer = gpuType("GPUCommandBuffer");
  var GPURenderPassEncoder = gpuType("GPURenderPassEncoder");
  var GPUComputePassEncoder = gpuType("GPUComputePassEncoder");
  var GPUQuerySet = gpuType("GPUQuerySet");

  method(GPUDevice.prototype, "createQuerySet", function (input) {
    var device = receiver(deviceStates, this, "GPUDevice");
    var value = dictionary(input, "GPUQuerySetDescriptor", ["label", "type", "count"]);
    if (value.type !== "occlusion") { missing("GPUQuerySet.type." + value.type); }
    var count = u32(value.count, "GPUQuerySet.count");
    var object = created(device, GPUQuerySet, "GPUQuerySet", "createQuerySet", {
      label: label(value.label), type: value.type, count: count
    });
    readonly(object, "type", value.type);
    readonly(object, "count", count);
    return object;
  });
  method(GPUQuerySet.prototype, "destroy", function () {
    var state = receiver(objectStates, this, "GPUQuerySet");
    if (state.kind !== "GPUQuerySet") { throw new TypeError("invalid GPUQuerySet"); }
    if (state.destroyed) { return; }
    state.destroyed = true;
    fire(state.device, "destroyQuerySet", state.handle);
  });

  function GPUPipelineError(message, options) {
    if (!(this instanceof GPUPipelineError)) { throw new TypeError("Illegal constructor"); }
    options = dictionary(options, "GPUPipelineErrorInit", ["reason"]);
    if (options.reason !== "validation" && options.reason !== "internal") {
      throw new TypeError("invalid GPUPipelineError.reason");
    }
    var error = new Error(String(message));
    Object.setPrototypeOf(error, GPUPipelineError.prototype);
    readonly(error, "reason", options.reason);
    return error;
  }
  Object.setPrototypeOf(GPUPipelineError.prototype, Error.prototype);
  Object.defineProperty(GPUPipelineError.prototype, "name", {
    value: "GPUPipelineError", configurable: true
  });

  function u32(value, name, fallback) {
    return uint(value, name, fallback, 4294967295);
  }

  function signed(value, name, fallback) {
    if (value === undefined) { return fallback; }
    value = Number(value);
    if (!Number.isInteger(value) || value < -2147483648 || value > 2147483647) {
      throw new TypeError("invalid " + name);
    }
    return value;
  }

  function number(value, name, fallback) {
    if (value === undefined && fallback !== undefined) { return fallback; }
    value = Number(value);
    if (!Number.isFinite(value)) { throw new TypeError("invalid " + name); }
    return value;
  }

  function text(value, fallback) {
    return value === undefined ? fallback : String(value);
  }

  function optionalText(value) {
    return value === undefined ? null : String(value);
  }

  function makeGpuObject(type, kind, device, handle, name) {
    var object = Object.create(type.prototype);
    object.label = label(name);
    objectStates.set(object, {
      kind: kind, device: device, handle: handle, object: object
    });
    return object;
  }

  function objectState(object, kind, device) {
    var state;
    if (kind === "GPUBuffer") { state = receiver(bufferStates, object, kind); }
    else if (kind === "GPUTexture") { state = receiver(textureStates, object, kind); }
    else {
      state = receiver(objectStates, object, kind);
      if (state.kind !== kind) { throw new TypeError("invalid " + kind); }
    }
    if (device && state.device !== device) {
      throw new TypeError(kind + " belongs to another GPUDevice");
    }
    if (state.destroyed || state.device.destroyed) {
      throw operationError(kind + " is unavailable");
    }
    return state;
  }

  function objectHandle(object, kind, device) {
    return objectState(object, kind, device).handle;
  }

  function created(device, type, kind, op, descriptor) {
    var handle = nextId++;
    var object = makeGpuObject(type, kind, device, handle, descriptor.label);
    fire(device, op, handle, descriptor);
    return object;
  }

  method(GPUDevice.prototype, "createShaderModule", function (input) {
    var device = receiver(deviceStates, this, "GPUDevice");
    var value = dictionary(input, "GPUShaderModuleDescriptor", [
      "label", "code", "compilationHints"
    ]);
    if (value.compilationHints !== undefined) {
      missing("GPUShaderModuleDescriptor.compilationHints");
    }
    if (value.code === undefined) { throw new TypeError("GPUShaderModuleDescriptor.code is required"); }
    return created(device, GPUShaderModule, "GPUShaderModule", "createShaderModule", {
      label: label(value.label), code: String(value.code)
    });
  });

  method(GPUShaderModule.prototype, "getCompilationInfo", function () {
    var state = objectState(this, "GPUShaderModule");
    return command(state.device, "getCompilationInfo", state.handle).then(function (value) {
      var info = Object.create(GPUCompilationInfo.prototype);
      var messages = value.messages.map(function (value) {
        var message = Object.create(GPUCompilationMessage.prototype);
        ["message", "type", "lineNum", "linePos", "offset", "length"].forEach(function (name) {
          readonly(message, name, value[name]);
        });
        return Object.freeze(message);
      });
      readonly(info, "messages", Object.freeze(messages));
      return Object.freeze(info);
    });
  });

  method(GPUDevice.prototype, "createBindGroupLayout", function (input) {
    var device = receiver(deviceStates, this, "GPUDevice");
    var value = dictionary(input, "GPUBindGroupLayoutDescriptor", ["label", "entries"]);
    var entries = Array.from(value.entries).map(function (input) {
      var value = dictionary(input, "GPUBindGroupLayoutEntry", [
        "binding", "visibility", "buffer", "sampler", "texture", "storageTexture", "externalTexture"
      ]);
      if (value.externalTexture !== undefined) {
        missing("GPUBindGroupLayoutEntry.externalTexture");
      }
      var entry = {
        binding: u32(value.binding, "binding"),
        visibility: u32(value.visibility, "visibility")
      };
      if (value.buffer !== undefined) {
        var buffer = dictionary(value.buffer, "GPUBufferBindingLayout", [
          "type", "hasDynamicOffset", "minBindingSize"
        ]);
        entry.buffer = {
          type: text(buffer.type, "uniform"),
          hasDynamicOffset: !!buffer.hasDynamicOffset,
          minBindingSize: uint(buffer.minBindingSize, "minBindingSize", 0)
        };
      }
      if (value.sampler !== undefined) {
        var sampler = dictionary(value.sampler, "GPUSamplerBindingLayout", ["type"]);
        entry.sampler = { type: text(sampler.type, "filtering") };
      }
      if (value.texture !== undefined) {
        var texture = dictionary(value.texture, "GPUTextureBindingLayout", [
          "sampleType", "viewDimension", "multisampled"
        ]);
        entry.texture = {
          sampleType: text(texture.sampleType, "float"),
          viewDimension: text(texture.viewDimension, "2d"),
          multisampled: !!texture.multisampled
        };
      }
      if (value.storageTexture !== undefined) {
        var storage = dictionary(value.storageTexture, "GPUStorageTextureBindingLayout", [
          "access", "format", "viewDimension"
        ]);
        if (storage.format === undefined) { throw new TypeError("storageTexture.format is required"); }
        entry.storageTexture = {
          access: text(storage.access, "write-only"),
          format: String(storage.format),
          viewDimension: text(storage.viewDimension, "2d")
        };
      }
      return entry;
    });
    return created(device, GPUBindGroupLayout, "GPUBindGroupLayout", "createBindGroupLayout", {
      label: label(value.label), entries: entries
    });
  });

  method(GPUDevice.prototype, "createPipelineLayout", function (input) {
    var device = receiver(deviceStates, this, "GPUDevice");
    var value = dictionary(input, "GPUPipelineLayoutDescriptor", ["label", "bindGroupLayouts"]);
    return created(device, GPUPipelineLayout, "GPUPipelineLayout", "createPipelineLayout", {
      label: label(value.label),
      bindGroupLayouts: Array.from(value.bindGroupLayouts).map(function (layout) {
        return layout === null ? null : objectHandle(layout, "GPUBindGroupLayout", device);
      })
    });
  });

  method(GPUDevice.prototype, "createBindGroup", function (input) {
    var device = receiver(deviceStates, this, "GPUDevice");
    var value = dictionary(input, "GPUBindGroupDescriptor", ["label", "layout", "entries"]);
    return created(device, GPUBindGroup, "GPUBindGroup", "createBindGroup", {
      label: label(value.label),
      layout: objectHandle(value.layout, "GPUBindGroupLayout", device),
      entries: Array.from(value.entries).map(function (input) {
        var value = dictionary(input, "GPUBindGroupEntry", ["binding", "resource"]);
        var state = objectStates.get(value.resource);
        var resource;
        if (state && state.kind === "GPUSampler") {
          resource = { kind: "sampler", handle: objectHandle(value.resource, "GPUSampler", device) };
        } else if (state && state.kind === "GPUTextureView") {
          resource = { kind: "view", handle: objectHandle(value.resource, "GPUTextureView", device) };
        } else {
          var buffer = dictionary(value.resource, "GPUBufferBinding", ["buffer", "offset", "size"]);
          resource = {
            kind: "buffer",
            handle: objectHandle(buffer.buffer, "GPUBuffer", device),
            offset: uint(buffer.offset, "offset", 0),
            size: buffer.size === undefined ? null : uint(buffer.size, "size")
          };
        }
        return { binding: u32(value.binding, "binding"), resource: resource };
      })
    });
  });

  function pipelineLayout(value, device) {
    if (value === "auto") { return null; }
    return objectHandle(value, "GPUPipelineLayout", device);
  }

  function shaderStage(input, device, kind) {
    var allowed = ["module", "entryPoint", "constants"];
    if (kind === "vertex") { allowed.push("buffers"); }
    if (kind === "fragment") { allowed.push("targets"); }
    var value = dictionary(input, "GPU" + kind + "State", allowed);
    var constants = dictionary(value.constants, "GPUPipelineConstants", Object.keys(value.constants || {}));
    var copied = Object.create(null);
    Object.keys(constants).forEach(function (name) {
      if (constants[name] !== undefined) { copied[name] = number(constants[name], "constant " + name); }
    });
    var stage = {
      module: objectHandle(value.module, "GPUShaderModule", device),
      entryPoint: optionalText(value.entryPoint),
      constants: copied
    };
    if (kind === "vertex") {
      stage.buffers = value.buffers === undefined ? [] : Array.from(value.buffers).map(function (input) {
        if (input === null) { return { arrayStride: 0, stepMode: "vertex", attributes: [] }; }
        var value = dictionary(input, "GPUVertexBufferLayout", ["arrayStride", "stepMode", "attributes"]);
        return {
          arrayStride: uint(value.arrayStride, "arrayStride"),
          stepMode: text(value.stepMode, "vertex"),
          attributes: Array.from(value.attributes).map(function (input) {
            var value = dictionary(input, "GPUVertexAttribute", ["format", "offset", "shaderLocation"]);
            if (value.format === undefined) { throw new TypeError("GPUVertexAttribute.format is required"); }
            return {
              format: String(value.format),
              offset: uint(value.offset, "offset"),
              shaderLocation: u32(value.shaderLocation, "shaderLocation")
            };
          })
        };
      });
    }
    if (kind === "fragment") {
      stage.targets = Array.from(value.targets).map(function (input) {
        if (input === null) { return null; }
        var value = dictionary(input, "GPUColorTargetState", ["format", "blend", "writeMask"]);
        if (value.format === undefined) { throw new TypeError("GPUColorTargetState.format is required"); }
        var blend = null;
        if (value.blend !== undefined) {
          var parts = dictionary(value.blend, "GPUBlendState", ["color", "alpha"]);
          blend = { color: blendComponent(parts.color), alpha: blendComponent(parts.alpha) };
        }
        return {
          format: String(value.format), blend: blend,
          writeMask: u32(value.writeMask, "writeMask", 15)
        };
      });
    }
    return stage;
  }

  function blendComponent(input) {
    var value = dictionary(input, "GPUBlendComponent", ["operation", "srcFactor", "dstFactor"]);
    return {
      operation: text(value.operation, "add"),
      srcFactor: text(value.srcFactor, "one"),
      dstFactor: text(value.dstFactor, "zero")
    };
  }

  function stencilFace(input) {
    var value = dictionary(input, "GPUStencilFaceState", ["compare", "failOp", "depthFailOp", "passOp"]);
    return {
      compare: text(value.compare, "always"),
      failOp: text(value.failOp, "keep"),
      depthFailOp: text(value.depthFailOp, "keep"),
      passOp: text(value.passOp, "keep")
    };
  }

  function pipelineDescriptor(input, device, render) {
    var value = dictionary(input, render ? "GPURenderPipelineDescriptor" : "GPUComputePipelineDescriptor",
      render ? ["label", "layout", "vertex", "fragment", "primitive", "depthStencil", "multisample"]
        : ["label", "layout", "compute"]);
    var descriptor = { label: label(value.label), layout: pipelineLayout(value.layout, device) };
    if (!render) {
      descriptor.compute = shaderStage(value.compute, device, "compute");
      return descriptor;
    }
    descriptor.vertex = shaderStage(value.vertex, device, "vertex");
    descriptor.fragment = value.fragment === undefined ? null : shaderStage(value.fragment, device, "fragment");
    var primitive = dictionary(value.primitive, "GPUPrimitiveState", [
      "topology", "stripIndexFormat", "frontFace", "cullMode", "unclippedDepth"
    ]);
    descriptor.primitive = {
      topology: text(primitive.topology, "triangle-list"),
      stripIndexFormat: optionalText(primitive.stripIndexFormat),
      frontFace: text(primitive.frontFace, "ccw"),
      cullMode: primitive.cullMode === undefined || primitive.cullMode === "none"
        ? null : String(primitive.cullMode),
      unclippedDepth: !!primitive.unclippedDepth
    };
    descriptor.depthStencil = null;
    if (value.depthStencil !== undefined) {
      var depth = dictionary(value.depthStencil, "GPUDepthStencilState", [
        "format", "depthWriteEnabled", "depthCompare", "stencilFront", "stencilBack",
        "stencilReadMask", "stencilWriteMask", "depthBias", "depthBiasSlopeScale", "depthBiasClamp"
      ]);
      if (depth.format === undefined) { throw new TypeError("GPUDepthStencilState.format is required"); }
      descriptor.depthStencil = {
        format: String(depth.format),
        depthWriteEnabled: depth.depthWriteEnabled === undefined ? null : !!depth.depthWriteEnabled,
        depthCompare: optionalText(depth.depthCompare),
        stencilFront: stencilFace(depth.stencilFront),
        stencilBack: stencilFace(depth.stencilBack),
        stencilReadMask: u32(depth.stencilReadMask, "stencilReadMask", 4294967295),
        stencilWriteMask: u32(depth.stencilWriteMask, "stencilWriteMask", 4294967295),
        depthBias: signed(depth.depthBias, "depthBias", 0),
        depthBiasSlopeScale: number(depth.depthBiasSlopeScale, "depthBiasSlopeScale", 0),
        depthBiasClamp: number(depth.depthBiasClamp, "depthBiasClamp", 0)
      };
    }
    var samples = dictionary(value.multisample, "GPUMultisampleState", [
      "count", "mask", "alphaToCoverageEnabled"
    ]);
    descriptor.multisample = {
      count: u32(samples.count, "count", 1),
      mask: u32(samples.mask, "mask", 4294967295),
      alphaToCoverageEnabled: !!samples.alphaToCoverageEnabled
    };
    return descriptor;
  }

  function installPipeline(name, render, asynchronous) {
    var type = render ? GPURenderPipeline : GPUComputePipeline;
    var kind = render ? "GPURenderPipeline" : "GPUComputePipeline";
    method(GPUDevice.prototype, name, function (input) {
      var device = receiver(deviceStates, this, "GPUDevice");
      if (!asynchronous) {
        return created(device, type, kind, name, pipelineDescriptor(input, device, render));
      }
      try {
        var descriptor = pipelineDescriptor(input, device, render);
        var handle = nextId++;
        return command(device, name, handle, descriptor).then(function () {
          return makeGpuObject(type, kind, device, handle, descriptor.label);
        }, function (error) {
          throw new GPUPipelineError(error.message, {
            reason: error.kind === "validation" ? "validation" : "internal"
          });
        });
      } catch (error) {
        return Promise.reject(error);
      }
    });
    method(type.prototype, "getBindGroupLayout", function (index) {
      var state = objectState(this, kind);
      var handle = nextId++;
      var object = makeGpuObject(GPUBindGroupLayout, "GPUBindGroupLayout", state.device, handle, "");
      fire(state.device, "getBindGroupLayout", handle, {
        pipeline: state.handle,
        kind: render ? "render" : "compute",
        index: u32(index, "index")
      });
      return object;
    });
  }
  installPipeline("createRenderPipeline", true, false);
  installPipeline("createRenderPipelineAsync", true, true);
  installPipeline("createComputePipeline", false, false);
  installPipeline("createComputePipelineAsync", false, true);

  function clearColor(input) {
    if (input === undefined) { return [0, 0, 0, 0]; }
    if (input && typeof input[Symbol.iterator] === "function") {
      var sequence = Array.from(input);
      if (sequence.length !== 4) { throw new TypeError("GPUColor requires four components"); }
      return sequence.map(function (value) { return number(value, "GPUColor"); });
    }
    var value = dictionary(input, "GPUColor", ["r", "g", "b", "a"]);
    return [
      number(value.r, "GPUColor.r"), number(value.g, "GPUColor.g"),
      number(value.b, "GPUColor.b"), number(value.a, "GPUColor.a")
    ];
  }

  function renderPassDescriptor(input, device) {
    var value = dictionary(input, "GPURenderPassDescriptor", [
      "label", "colorAttachments", "depthStencilAttachment", "occlusionQuerySet",
      "timestampWrites", "maxDrawCount"
    ]);
    if (value.timestampWrites !== undefined) { missing("GPURenderPassDescriptor.timestampWrites"); }
    if (value.maxDrawCount !== undefined) { uint(value.maxDrawCount, "maxDrawCount"); }
    var canvasTextures = [];
    function viewHandle(view) {
      var state = objectState(view, "GPUTextureView", device);
      if (state.texture && state.texture.canvas) { canvasTextures.push(state.texture); }
      return state.handle;
    }
    var descriptor = {
      label: label(value.label),
      canvasTextures: canvasTextures,
      occlusionQuerySet: value.occlusionQuerySet === undefined || value.occlusionQuerySet === null
        ? null : objectHandle(value.occlusionQuerySet, "GPUQuerySet", device),
      colorAttachments: Array.from(value.colorAttachments).map(function (input) {
        if (input === null) { return null; }
        var value = dictionary(input, "GPURenderPassColorAttachment", [
          "view", "resolveTarget", "depthSlice", "clearValue", "loadOp", "storeOp"
        ]);
        if (value.loadOp === undefined || value.storeOp === undefined) {
          throw new TypeError("color attachment requires loadOp and storeOp");
        }
        return {
          view: viewHandle(value.view),
          resolveTarget: value.resolveTarget === undefined || value.resolveTarget === null
            ? null : viewHandle(value.resolveTarget),
          depthSlice: value.depthSlice === undefined ? null : u32(value.depthSlice, "depthSlice"),
          clearValue: clearColor(value.clearValue),
          loadOp: String(value.loadOp), storeOp: String(value.storeOp)
        };
      }),
      depthStencilAttachment: null,
      commands: []
    };
    if (value.depthStencilAttachment !== undefined) {
      var depth = dictionary(value.depthStencilAttachment, "GPURenderPassDepthStencilAttachment", [
        "view", "depthClearValue", "depthLoadOp", "depthStoreOp", "depthReadOnly",
        "stencilClearValue", "stencilLoadOp", "stencilStoreOp", "stencilReadOnly"
      ]);
      descriptor.depthStencilAttachment = {
        view: objectHandle(depth.view, "GPUTextureView", device),
        depthClearValue: number(depth.depthClearValue, "depthClearValue", 0),
        depthLoadOp: optionalText(depth.depthLoadOp),
        depthStoreOp: optionalText(depth.depthStoreOp),
        depthReadOnly: !!depth.depthReadOnly,
        stencilClearValue: u32(depth.stencilClearValue, "stencilClearValue", 0),
        stencilLoadOp: optionalText(depth.stencilLoadOp),
        stencilStoreOp: optionalText(depth.stencilStoreOp),
        stencilReadOnly: !!depth.stencilReadOnly
      };
    }
    return descriptor;
  }

  function encoderState(object) {
    var state = objectState(object, "GPUCommandEncoder");
    if (state.finished || state.locked) {
      throw new DOMException("GPUCommandEncoder is not open", "InvalidStateError");
    }
    return state;
  }

  function countCommand(state) {
    if (state.count >= 65536) { throw new RangeError("WebGPU command limit reached in chuzz"); }
    state.count++;
  }

  function recordEncoder(object, op, args) {
    var state = encoderState(object);
    countCommand(state);
    state.operations.push({ op: op, args: args });
  }

  function passState(object) {
    var state = receiver(objectStates, object, "GPU pass encoder");
    if ((state.kind !== "GPURenderPassEncoder" && state.kind !== "GPUComputePassEncoder") ||
        state.ended || state.encoder.finished || state.encoder.locked !== state ||
        state.device.destroyed) {
      throw new DOMException("GPU pass encoder is not open", "InvalidStateError");
    }
    return state;
  }

  function recordPass(object, op, args) {
    var state = passState(object);
    countCommand(state.encoder);
    if (op === "draw" || op === "drawIndexed" || op === "drawIndirect") {
      state.drawCount++;
      if (state.drawCount > state.maxDrawCount) {
        throw operationError("GPURenderPassDescriptor.maxDrawCount exceeded");
      }
    }
    state.commands.push({ op: op, args: args });
  }

  method(GPUDevice.prototype, "createCommandEncoder", function (input) {
    var device = receiver(deviceStates, this, "GPUDevice");
    var value = dictionary(input, "GPUCommandEncoderDescriptor", ["label"]);
    var object = created(device, GPUCommandEncoder, "GPUCommandEncoder", "createCommandEncoder", {
      label: label(value.label)
    });
    var state = objectStates.get(object);
    state.finished = false;
    state.locked = null;
    state.operations = [];
    state.canvasTextures = new Set();
    state.count = 0;
    return object;
  });

  function beginPass(encoder, input, render) {
    var state = encoderState(encoder);
    var descriptor;
    if (render) { descriptor = renderPassDescriptor(input, state.device); }
    else {
      var value = dictionary(input, "GPUComputePassDescriptor", ["label", "timestampWrites"]);
      if (value.timestampWrites !== undefined) { missing("GPUComputePassDescriptor.timestampWrites"); }
      descriptor = { label: label(value.label), commands: [] };
    }
    (descriptor.canvasTextures || []).forEach(function (texture) {
      state.canvasTextures.add(texture);
    });
    delete descriptor.canvasTextures;
    countCommand(state);
    var type = render ? GPURenderPassEncoder : GPUComputePassEncoder;
    var kind = render ? "GPURenderPassEncoder" : "GPUComputePassEncoder";
    var object = makeGpuObject(type, kind, state.device, 0, descriptor.label);
    var pass = objectStates.get(object);
    pass.encoder = state;
    pass.commands = descriptor.commands;
    pass.ended = false;
    pass.drawCount = 0;
    pass.maxDrawCount = render ? uint(input.maxDrawCount, "maxDrawCount", 50000000) : 0;
    state.locked = pass;
    state.operations.push({ op: render ? "render" : "compute", args: descriptor });
    return object;
  }
  method(GPUCommandEncoder.prototype, "beginRenderPass", function (input) {
    return beginPass(this, input, true);
  });
  method(GPUCommandEncoder.prototype, "beginComputePass", function (input) {
    return beginPass(this, input, false);
  });
  method(GPUCommandEncoder.prototype, "finish", function (input) {
    var state = encoderState(this);
    var value = dictionary(input, "GPUCommandBufferDescriptor", ["label"]);
    var handle = nextId++;
    var object = makeGpuObject(GPUCommandBuffer, "GPUCommandBuffer", state.device, handle, value.label);
    objectStates.get(object).submitted = false;
    objectStates.get(object).canvasTextures = Array.from(state.canvasTextures);
    state.finished = true;
    fire(state.device, "finish", handle, {
      encoder: state.handle,
      label: label(value.label),
      operations: state.operations
    });
    state.operations = [];
    return object;
  });

  [GPURenderPassEncoder, GPUComputePassEncoder].forEach(function (type) {
    method(type.prototype, "setPipeline", function (pipeline) {
      var state = passState(this);
      var kind = state.kind === "GPURenderPassEncoder" ? "GPURenderPipeline" : "GPUComputePipeline";
      recordPass(this, "pipeline", objectHandle(pipeline, kind, state.device));
    });
    method(type.prototype, "setBindGroup", function (index, group, dynamicOffsets, start, length) {
      var state = passState(this);
      var offsets = dynamicOffsets === undefined ? [] : Array.from(dynamicOffsets);
      if (start !== undefined || length !== undefined) {
        start = uint(start, "dynamicOffsetsDataStart", 0);
        length = uint(length, "dynamicOffsetsDataLength", offsets.length - start);
        if (start > offsets.length || length > offsets.length - start) {
          throw new RangeError("dynamic offsets range is out of bounds");
        }
        offsets = offsets.slice(start, start + length);
      }
      offsets = offsets.map(function (value) { return u32(value, "dynamic offset"); });
      recordPass(this, "bindGroup", {
        index: u32(index, "index"),
        group: group === null ? null : objectHandle(group, "GPUBindGroup", state.device),
        offsets: offsets
      });
    });
    method(type.prototype, "end", function () {
      var state = passState(this);
      state.ended = true;
      state.encoder.locked = null;
    });
  });

  method(GPURenderPassEncoder.prototype, "setVertexBuffer", function (slot, buffer, offset, size) {
    var state = passState(this);
    if (buffer === null) { missing("GPURenderPassEncoder.setVertexBuffer(null)"); }
    recordPass(this, "vertexBuffer", {
      slot: u32(slot, "slot"),
      buffer: objectHandle(buffer, "GPUBuffer", state.device),
      offset: uint(offset, "offset", 0),
      size: size === undefined ? null : uint(size, "size")
    });
  });
  method(GPURenderPassEncoder.prototype, "setIndexBuffer", function (buffer, format, offset, size) {
    var state = passState(this);
    recordPass(this, "indexBuffer", {
      buffer: objectHandle(buffer, "GPUBuffer", state.device),
      format: String(format),
      offset: uint(offset, "offset", 0),
      size: size === undefined ? null : uint(size, "size")
    });
  });
  method(GPURenderPassEncoder.prototype, "draw", function (count, instances, first, firstInstance) {
    recordPass(this, "draw", {
      count: u32(count, "vertexCount"), instances: u32(instances, "instanceCount", 1),
      first: u32(first, "firstVertex", 0), firstInstance: u32(firstInstance, "firstInstance", 0)
    });
  });
  method(GPURenderPassEncoder.prototype, "drawIndexed", function (count, instances, first, baseVertex, firstInstance) {
    recordPass(this, "drawIndexed", {
      count: u32(count, "indexCount"), instances: u32(instances, "instanceCount", 1),
      first: u32(first, "firstIndex", 0), baseVertex: signed(baseVertex, "baseVertex", 0),
      firstInstance: u32(firstInstance, "firstInstance", 0)
    });
  });
  method(GPURenderPassEncoder.prototype, "setViewport", function (x, y, width, height, minDepth, maxDepth) {
    recordPass(this, "viewport", [
      number(x, "x"), number(y, "y"), number(width, "width"), number(height, "height"),
      number(minDepth, "minDepth"), number(maxDepth, "maxDepth")
    ]);
  });
  method(GPURenderPassEncoder.prototype, "setScissorRect", function (x, y, width, height) {
    recordPass(this, "scissor", [
      u32(x, "x"), u32(y, "y"), u32(width, "width"), u32(height, "height")
    ]);
  });
  method(GPURenderPassEncoder.prototype, "setBlendConstant", function (value) {
    recordPass(this, "blendConstant", clearColor(value));
  });
  method(GPURenderPassEncoder.prototype, "setStencilReference", function (value) {
    recordPass(this, "stencilReference", u32(value, "reference"));
  });
  method(GPUComputePassEncoder.prototype, "dispatchWorkgroups", function (x, y, z) {
    recordPass(this, "dispatch", [
      u32(x, "workgroupCountX"), u32(y, "workgroupCountY", 1), u32(z, "workgroupCountZ", 1)
    ]);
  });

  function textureCopy(input, device) {
    var value = dictionary(input, "GPUTexelCopyTextureInfo", [
      "texture", "mipLevel", "origin", "aspect"
    ]);
    return {
      texture: objectHandle(value.texture, "GPUTexture", device),
      mipLevel: u32(value.mipLevel, "mipLevel", 0),
      origin: origin(value.origin),
      aspect: text(value.aspect, "all")
    };
  }
  function bufferCopy(input, device) {
    var value = dictionary(input, "GPUTexelCopyBufferInfo", [
      "buffer", "offset", "bytesPerRow", "rowsPerImage"
    ]);
    return {
      buffer: objectHandle(value.buffer, "GPUBuffer", device),
      offset: uint(value.offset, "offset", 0),
      bytesPerRow: value.bytesPerRow === undefined ? null : u32(value.bytesPerRow, "bytesPerRow"),
      rowsPerImage: value.rowsPerImage === undefined ? null : u32(value.rowsPerImage, "rowsPerImage")
    };
  }
  method(GPUCommandEncoder.prototype, "copyBufferToBuffer", function (source, sourceOffset, destination, destinationOffset, size) {
    var state = encoderState(this);
    recordEncoder(this, "copyBufferToBuffer", {
      source: objectHandle(source, "GPUBuffer", state.device),
      sourceOffset: uint(sourceOffset, "sourceOffset"),
      destination: objectHandle(destination, "GPUBuffer", state.device),
      destinationOffset: uint(destinationOffset, "destinationOffset"),
      size: uint(size, "size")
    });
  });
  method(GPUCommandEncoder.prototype, "copyBufferToTexture", function (source, destination, size) {
    var state = encoderState(this);
    recordEncoder(this, "copyBufferToTexture", {
      source: bufferCopy(source, state.device),
      destination: textureCopy(destination, state.device),
      size: extent(size, "GPUExtent3D")
    });
  });
  method(GPUCommandEncoder.prototype, "copyTextureToBuffer", function (source, destination, size) {
    var state = encoderState(this);
    recordEncoder(this, "copyTextureToBuffer", {
      source: textureCopy(source, state.device),
      destination: bufferCopy(destination, state.device),
      size: extent(size, "GPUExtent3D")
    });
  });
  method(GPUCommandEncoder.prototype, "copyTextureToTexture", function (source, destination, size) {
    var state = encoderState(this);
    recordEncoder(this, "copyTextureToTexture", {
      source: textureCopy(source, state.device),
      destination: textureCopy(destination, state.device),
      size: extent(size, "GPUExtent3D")
    });
  });

  method(GPUQueue.prototype, "submit", function (input) {
    var device = receiver(queueStates, this, "GPUQueue");
    var states = Array.from(input).map(function (object) {
      return objectState(object, "GPUCommandBuffer", device);
    });
    var seen = new Set();
    states.forEach(function (state) {
      if (state.submitted || seen.has(state.handle)) {
        throw operationError("GPUCommandBuffer has already been submitted");
      }
      seen.add(state.handle);
    });
    states.forEach(function (state) {
      state.submitted = true;
      (state.canvasTextures || []).forEach(function (texture) {
        var canvas = texture.canvas;
        if (canvas && canvas.current && textureStates.get(canvas.current) === texture) {
          canvas.presentable = true;
        }
      });
    });
    fire(device, "submit", 0, states.map(function (state) { return state.handle; }));
  });
  method(GPUQueue.prototype, "onSubmittedWorkDone", function () {
    var device = receiver(queueStates, this, "GPUQueue");
    return command(device, "onSubmittedWorkDone").then(function () {});
  });

  method(GPUQueue.prototype, "copyExternalImageToTexture", function (input, output, size) {
    var device = receiver(queueStates, this, "GPUQueue");
    var source = dictionary(input, "GPUImageCopyExternalImage", ["source", "origin", "flipY"]);
    var destination = dictionary(output, "GPUImageCopyTextureTagged", [
      "texture", "mipLevel", "origin", "aspect", "colorSpace", "premultipliedAlpha"
    ]);
    var texture = objectState(destination.texture, "GPUTexture", device);
    var from = origin(source.origin);
    if (from[2] !== 0) { throw new TypeError("external image origin must be two-dimensional"); }
    if (!globalThis.__chuzzGpuImageSource) { missing("GPUQueue.copyExternalImageToTexture"); }
    var colorSpace = text(destination.colorSpace, "srgb");
    if (colorSpace !== "srgb") { missing("GPUImageCopyTextureTagged.colorSpace." + colorSpace); }
    fire(device, "copyExternalImageToTexture", 0, {
      source: globalThis.__chuzzGpuImageSource(source.source),
      sourceOrigin: from.slice(0, 2),
      flipY: !!source.flipY,
      texture: texture.handle,
      mipLevel: u32(destination.mipLevel, "mipLevel", 0),
      origin: origin(destination.origin),
      aspect: text(destination.aspect, "all"),
      colorSpace: colorSpace,
      premultipliedAlpha: !!destination.premultipliedAlpha,
      size: extent(size, "GPUExtent3D")
    });
  });
  method(GPUCommandEncoder.prototype, "resolveQuerySet", function (query, first, count, buffer, offset) {
    var state = encoderState(this);
    recordEncoder(this, "resolveQuerySet", {
      query: objectHandle(query, "GPUQuerySet", state.device),
      first: u32(first, "firstQuery"),
      count: u32(count, "queryCount"),
      destination: objectHandle(buffer, "GPUBuffer", state.device),
      offset: uint(offset, "destinationOffset")
    });
  });
  method(GPUCommandEncoder.prototype, "clearBuffer", function (buffer, offset, size) {
    var state = encoderState(this);
    recordEncoder(this, "clearBuffer", {
      buffer: objectHandle(buffer, "GPUBuffer", state.device),
      offset: uint(offset, "offset", 0),
      size: size === undefined ? null : uint(size, "size")
    });
  });
  method(GPURenderPassEncoder.prototype, "beginOcclusionQuery", function (index) {
    recordPass(this, "beginOcclusionQuery", u32(index, "queryIndex"));
  });
  method(GPURenderPassEncoder.prototype, "endOcclusionQuery", function () {
    recordPass(this, "endOcclusionQuery", null);
  });
  ["drawIndirect", "drawIndexedIndirect"].forEach(function (name) {
    method(GPURenderPassEncoder.prototype, name, function (buffer, offset) {
      var state = passState(this);
      recordPass(this, "drawIndirect", {
        buffer: objectHandle(buffer, "GPUBuffer", state.device),
        offset: uint(offset, "indirectOffset"),
        indexed: name === "drawIndexedIndirect"
      });
    });
  });
  method(GPUComputePassEncoder.prototype, "dispatchWorkgroupsIndirect", function (buffer, offset) {
    var state = passState(this);
    recordPass(this, "dispatchIndirect", {
      buffer: objectHandle(buffer, "GPUBuffer", state.device),
      offset: uint(offset, "indirectOffset")
    });
  });

  unsupportedMethods(GPUDevice.prototype, "GPUDevice", [
    "importExternalTexture",
    "createRenderBundleEncoder"
  ]);

  unsupportedMethods(GPUCommandEncoder.prototype, "GPUCommandEncoder", [
    "writeTimestamp",
    "insertDebugMarker", "pushDebugGroup", "popDebugGroup"
  ]);
  unsupportedMethods(GPURenderPassEncoder.prototype, "GPURenderPassEncoder", [
    "executeBundles",
    "insertDebugMarker", "pushDebugGroup", "popDebugGroup"
  ]);
  unsupportedMethods(GPUComputePassEncoder.prototype, "GPUComputePassEncoder", [
    "insertDebugMarker", "pushDebugGroup", "popDebugGroup"
  ]);

  [
    ["GPU", GPU],
    ["GPUAdapter", GPUAdapter],
    ["GPUDevice", GPUDevice],
    ["GPUQueue", GPUQueue],
    ["GPUBuffer", GPUBuffer],
    ["GPUTexture", GPUTexture],
    ["GPUTextureView", GPUTextureView],
    ["GPUSampler", GPUSampler],
    ["GPUCanvasContext", GPUCanvasContext],
    ["GPUDeviceLostInfo", GPUDeviceLostInfo],
    ["GPUError", GPUError],
    ["GPUValidationError", GPUValidationError],
    ["GPUOutOfMemoryError", GPUOutOfMemoryError],
    ["GPUInternalError", GPUInternalError],
    ["GPUUncapturedErrorEvent", GPUUncapturedErrorEvent],
    ["GPUSupportedFeatures", GPUSupportedFeatures],
    ["GPUSupportedLimits", GPUSupportedLimits],
    ["GPUShaderModule", GPUShaderModule],
    ["GPUCompilationInfo", GPUCompilationInfo],
    ["GPUCompilationMessage", GPUCompilationMessage],
    ["GPUBindGroupLayout", GPUBindGroupLayout],
    ["GPUPipelineLayout", GPUPipelineLayout],
    ["GPUBindGroup", GPUBindGroup],
    ["GPURenderPipeline", GPURenderPipeline],
    ["GPUComputePipeline", GPUComputePipeline],
    ["GPUPipelineError", GPUPipelineError],
    ["GPUCommandEncoder", GPUCommandEncoder],
    ["GPUCommandBuffer", GPUCommandBuffer],
    ["GPURenderPassEncoder", GPURenderPassEncoder],
    ["GPUComputePassEncoder", GPUComputePassEncoder],
    ["GPUQuerySet", GPUQuerySet]
  ].forEach(function (entry) {
    Object.defineProperty(entry[1].prototype, Symbol.toStringTag, {
      value: entry[0],
      configurable: true
    });
    Object.defineProperty(globalThis, entry[0], {
      value: entry[1],
      writable: true,
      configurable: true
    });
  });

  function constants(name, values) {
    Object.defineProperty(globalThis, name, {
      value: Object.freeze(values),
      configurable: true
    });
  }
  constants("GPUBufferUsage", {
    MAP_READ: 1, MAP_WRITE: 2, COPY_SRC: 4, COPY_DST: 8,
    INDEX: 16, VERTEX: 32, UNIFORM: 64, STORAGE: 128,
    INDIRECT: 256, QUERY_RESOLVE: 512
  });
  constants("GPUTextureUsage", {
    COPY_SRC: 1, COPY_DST: 2, TEXTURE_BINDING: 4,
    STORAGE_BINDING: 8, RENDER_ATTACHMENT: 16
  });
  constants("GPUShaderStage", { VERTEX: 1, FRAGMENT: 2, COMPUTE: 4 });
  constants("GPUMapMode", { READ: 1, WRITE: 2 });
  constants("GPUColorWrite", { RED: 1, GREEN: 2, BLUE: 4, ALPHA: 8, ALL: 15 });

  // Blitz shares the element prototype. Intercept only canvas WebGPU calls
  // and retain the native answers and context ownership for other types.
  var probe = document.createElement("canvas");
  var prototype = probe;
  while (prototype &&
         !Object.prototype.hasOwnProperty.call(prototype, "getContext")) {
    prototype = Object.getPrototypeOf(prototype);
  }
  if (prototype) {
    var getContext = prototype.getContext;
    method(prototype, "getContext", function (type) {
      type = String(type);
      if (String(this.tagName).toLowerCase() !== "canvas") {
        return getContext.apply(this, arguments);
      }
      reflectCanvas(this);
      var existing = canvasStates.get(this);
      if (type !== "webgpu") {
        if (existing) { return null; }
        var native = getContext.apply(this, arguments);
        if (native !== null && native !== undefined) { otherContexts.set(this, type); }
        return native;
      }
      if (otherContexts.has(this)) { return null; }
      if (existing) { return existing; }
      var object = Object.create(GPUCanvasContext.prototype);
      var handle = nextId++;
      var state = {
        handle: handle,
        canvas: this,
        generation: 0,
        device: null,
        configuration: null,
        current: null,
        presentable: false,
        size: null
      };
      this.setAttribute("data-chuzz-webgpu", String(handle));
      readonly(object, "canvas", this);
      canvasStates.set(this, object);
      contextStates.set(object, state);
      contexts[handle] = state;
      return object;
    });
  }

  globalThis.__chuzzGpuImageBridge = {
    nextHandle: function () { return nextId++; },
    send: function (op, handle, args) {
      return send("command", {
        device: 0,
        command: { op: op, handle: handle, args: args }
      });
    },
    reflectCanvas: reflectCanvas,
    claim2D: function (canvas) {
      if (canvasStates.has(canvas)) { return false; }
      otherContexts.set(canvas, "2d");
      return true;
    },
    canvasSource: function (canvas) {
      var context = canvasStates.get(canvas);
      if (!context) { return null; }
      var state = contextStates.get(context);
      resizeCanvas(state, false);
      if (!state.configuration || !state.device || state.device.destroyed) {
        throw new DOMException("GPUCanvasContext is not configured", "InvalidStateError");
      }
      if (state.current) {
        return {
          kind: "texture",
          device: state.device.handle,
          texture: objectHandle(state.current, "GPUTexture", state.device),
          premultipliedAlpha: state.configuration.alphaMode === "premultiplied"
        };
      }
      var size = canvasSize(canvas);
      return {
        kind: "canvas", canvas: state.handle, generation: state.generation,
        width: size[0], height: size[1]
      };
    }
  };

  Object.defineProperty(navigator, "gpu", {
    value: Object.create(GPU.prototype),
    enumerable: true,
    configurable: true
  });
})();

