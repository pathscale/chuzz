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
      if (allowed.indexOf(name) === -1) {
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
      var timer = setTimeout(function () {
        if (!pending[id]) { return; }
        delete pending[id];
        reject(new Error("WebGPU host did not answer in chuzz: " + operation));
      }, 15000);
      pending[id] = { resolve: resolve, reject: reject, timer: timer };
      fields.webgpu = operation;
      fields.id = id;
      try {
        window.ipc.postMessage(JSON.stringify(fields));
      } catch (error) {
        clearTimeout(timer);
        delete pending[id];
        reject(error);
      }
    });
  }

  globalThis.__chuzzGpuResolve = function (id, result) {
    var entry = pending[id];
    if (!entry) { return; }
    delete pending[id];
    clearTimeout(entry.timer);
    if (result.ok) { entry.resolve(result.value); }
    else {
      var error = new Error(result.error);
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
    Object.keys(values).forEach(function (name) {
      readonly(object, name, values[name]);
    });
    return Object.freeze(object);
  }

  function makeAdapter(value) {
    if (value === null) { return null; }
    var object = Object.create(GPUAdapter.prototype);
    adapterHandles.set(object, value.handle);
    readonly(object, "features", features(value.features));
    readonly(object, "limits", limits(value.limits));
    readonly(object, "isFallbackAdapter", value.isFallbackAdapter);
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

  unsupportedProperty(GPUAdapter.prototype, "GPUAdapter", "info");
  unsupportedMethods(GPUAdapter.prototype, "GPUAdapter", ["requestAdapterInfo"]);

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
      destroyed: false
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
    var object = Object.create(GPUTextureView.prototype);
    object.label = descriptor.label;
    fire(state.device, "createView", nextId++, {
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
    fire(device, "writeTexture", 0, {
      texture: texture.handle,
      mipLevel: uint(target.mipLevel, "mipLevel", 0, 4294967295),
      origin: origin(target.origin),
      aspect: target.aspect === undefined ? "all" : String(target.aspect),
      data: copyBytes(data, 0, undefined, false),
      offset: uint(source.offset, "offset", 0),
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
    var object = Object.create(GPUSampler.prototype);
    object.label = descriptor.label;
    fire(device, "createSampler", nextId++, descriptor);
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
    if (!state.current) { return; }
    var texture = textureStates.get(state.current);
    texture.destroyed = true;
    if (destroy) { fire(texture.device, "destroyTexture", texture.handle); }
    state.current = null;
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
      viewFormats: views
    };
    fire(device, "configure", state.handle, {
      generation: state.generation,
      format: format,
      usage: usage,
      alphaMode: alpha,
      viewFormats: views
    });
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
    fire(state.device, "currentTexture", handle, {
      canvas: state.handle,
      generation: state.generation,
      width: size[0],
      height: size[1]
    });
    return state.current;
  });
  unsupportedMethods(GPUCanvasContext.prototype, "GPUCanvasContext", ["getConfiguration"]);

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
      if (!state.current) { return; }
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

  unsupportedMethods(GPUDevice.prototype, "GPUDevice", [
    "importExternalTexture",
    "createBindGroupLayout",
    "createPipelineLayout",
    "createBindGroup",
    "createShaderModule",
    "createComputePipeline",
    "createRenderPipeline",
    "createComputePipelineAsync",
    "createRenderPipelineAsync",
    "createCommandEncoder",
    "createRenderBundleEncoder",
    "createQuerySet"
  ]);
  unsupportedProperty(GPUDevice.prototype, "GPUDevice", "adapterInfo");

  unsupportedMethods(GPUQueue.prototype, "GPUQueue", [
    "submit",
    "onSubmittedWorkDone",
    "copyExternalImageToTexture"
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
    ["GPUSupportedLimits", GPUSupportedLimits]
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
        current: null
      };
      this.setAttribute("data-chuzz-webgpu", String(handle));
      readonly(object, "canvas", this);
      canvasStates.set(this, object);
      contextStates.set(object, state);
      contexts[handle] = state;
      return object;
    });
  }

  Object.defineProperty(navigator, "gpu", {
    value: Object.create(GPU.prototype),
    enumerable: true,
    configurable: true
  });
})();

