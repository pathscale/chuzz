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
    else { entry.reject(new Error(result.error)); }
  };

  function GPU() { throw new TypeError("Illegal constructor: GPU"); }
  function GPUAdapter() { throw new TypeError("Illegal constructor: GPUAdapter"); }
  function GPUDevice() { throw new TypeError("Illegal constructor: GPUDevice"); }
  function GPUQueue() { throw new TypeError("Illegal constructor: GPUQueue"); }
  function GPUSupportedFeatures() {
    throw new TypeError("Illegal constructor: GPUSupportedFeatures");
  }
  function GPUSupportedLimits() {
    throw new TypeError("Illegal constructor: GPUSupportedLimits");
  }

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
    deviceHandles.set(object, value.handle);
    readonly(object, "features", features(value.features));
    readonly(object, "limits", limits(value.limits));
    readonly(object, "queue", Object.create(GPUQueue.prototype));
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
    // The planned page canvas target is an offscreen RGBA texture on every
    // backend, including headless readback into the CPU compositor.
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

  unsupportedMethods(GPUDevice.prototype, "GPUDevice", [
    "destroy",
    "createBuffer",
    "createTexture",
    "createSampler",
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
    "createQuerySet",
    "pushErrorScope",
    "popErrorScope",
    "addEventListener",
    "removeEventListener",
    "dispatchEvent"
  ]);
  unsupportedProperty(GPUDevice.prototype, "GPUDevice", "lost");
  unsupportedProperty(GPUDevice.prototype, "GPUDevice", "onuncapturederror");
  unsupportedProperty(GPUDevice.prototype, "GPUDevice", "adapterInfo");

  unsupportedMethods(GPUQueue.prototype, "GPUQueue", [
    "submit",
    "onSubmittedWorkDone",
    "writeBuffer",
    "writeTexture",
    "copyExternalImageToTexture"
  ]);
  unsupportedProperty(GPUQueue.prototype, "GPUQueue", "label");

  [
    ["GPU", GPU],
    ["GPUAdapter", GPUAdapter],
    ["GPUDevice", GPUDevice],
    ["GPUQueue", GPUQueue],
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

  // Blitz currently shares the element prototype and returns null for every
  // canvas context. Preserve its existing answers for other context types,
  // while making an explicit WebGPU call report the unfinished slice.
  var probe = document.createElement("canvas");
  var prototype = probe;
  while (prototype &&
         !Object.prototype.hasOwnProperty.call(prototype, "getContext")) {
    prototype = Object.getPrototypeOf(prototype);
  }
  if (prototype) {
    var getContext = prototype.getContext;
    method(prototype, "getContext", function (type) {
      if (String(type) === "webgpu") {
        return missing("HTMLCanvasElement.getContext(webgpu)");
      }
      return getContext.apply(this, arguments);
    });
  }

  Object.defineProperty(navigator, "gpu", {
    value: Object.create(GPU.prototype),
    enumerable: true,
    configurable: true
  });
})();

