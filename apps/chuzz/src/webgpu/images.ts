// JavaScript syntax only. Boa evaluates this source directly.
(function () {
  var bridge = globalThis.__chuzzGpuImageBridge;
  if (!bridge) { return; }
  var bitmaps = new WeakMap();
  var imageStates = new WeakMap();
  var canvasContexts = new WeakMap();
  var offscreens = new WeakMap();
  var markers = new WeakMap();
  // Readback request ids live above 2^52, clear of the bridge's own request counter.
  var readbackSerial = 4503599627370496;
  var limit = 64 * 1024 * 1024;

  function missing(member) { throw new Error("not implemented in chuzz: " + member); }
  function integer(value, name, fallback) {
    if (value === undefined && fallback !== undefined) { return fallback; }
    value = Number(value);
    if (!Number.isInteger(value) || value < 0 || value > 4294967295) {
      throw new TypeError("invalid " + name);
    }
    return value;
  }
  function length(width, height) {
    var size = width * height * 4;
    if (!Number.isSafeInteger(size) || size > limit) {
      throw new DOMException("image allocation limit reached in chuzz", "OperationError");
    }
    return size;
  }
  function marker(element) {
    var value = markers.get(element);
    if (value === undefined) {
      value = bridge.nextHandle();
      markers.set(element, value);
      element.setAttribute("data-chuzz-gpu-source", String(value));
    }
    return value;
  }
  function post(op, handle, args) {
    bridge.send(op, handle, args).catch(function (error) {
      setTimeout(function () { throw error; }, 0);
    });
  }
  function raw(width, height, data) {
    width = integer(width, "image.width");
    height = integer(height, "image.height");
    if (data.length !== length(width, height)) { throw new TypeError("invalid image byte length"); }
    return { kind: "pixels", width: width, height: height, data: data };
  }
  function canvasPixels(canvas) {
    var state = canvasContexts.get(canvas);
    if (!state) { return null; }
    state.width = canvas.width;
    state.height = canvas.height;
    state.data = state.context.__pixels();
    return state;
  }

  globalThis.__chuzzGpuImageSource = function (source) {
    var bitmap = bitmaps.get(source);
    if (bitmap) {
      if (bitmap.closed) { throw new DOMException("ImageBitmap is closed", "InvalidStateError"); }
      return { kind: "bitmap", handle: bitmap.handle };
    }
    var image = imageStates.get(source);
    if (image) {
      if (!image.bitmap) { throw new DOMException("image is not decoded", "InvalidStateError"); }
      return globalThis.__chuzzGpuImageSource(image.bitmap);
    }
    var canvas = offscreens.get(source) || source;
    if (String(canvas && canvas.tagName).toLowerCase() === "canvas") {
      var gpu = bridge.canvasSource(canvas);
      if (gpu) { return gpu; }
      var pixels = canvasPixels(canvas);
      if (pixels) { return raw(pixels.width, pixels.height, pixels.data); }
      return raw(canvas.width, canvas.height, new Uint8ClampedArray(length(canvas.width, canvas.height)));
    }
    if (source && source.data instanceof Uint8ClampedArray) {
      if (source.colorSpace && source.colorSpace !== "srgb") {
        missing("GPUQueue.copyExternalImageToTexture.ImageData.colorSpace." + source.colorSpace);
      }
      return raw(source.width, source.height, source.data);
    }
    if (String(source && source.tagName).toLowerCase() === "img") {
      return { kind: "element", handle: marker(source) };
    }
    throw new TypeError("unsupported external image source");
  };

  function ImageBitmap() { throw new TypeError("Illegal constructor: ImageBitmap"); }
  Object.defineProperty(ImageBitmap.prototype, Symbol.toStringTag, { value: "ImageBitmap" });
  Object.defineProperty(ImageBitmap.prototype, "width", {
    get: function () {
      var state = bitmaps.get(this);
      if (!state) { throw new TypeError("invalid ImageBitmap"); }
      return state.closed ? 0 : state.width;
    }
  });
  Object.defineProperty(ImageBitmap.prototype, "height", {
    get: function () {
      var state = bitmaps.get(this);
      if (!state) { throw new TypeError("invalid ImageBitmap"); }
      return state.closed ? 0 : state.height;
    }
  });
  ImageBitmap.prototype.close = function () {
    var state = bitmaps.get(this);
    if (!state) { throw new TypeError("invalid ImageBitmap"); }
    if (state.closed) { return; }
    state.closed = true;
    state.data = null;
    post("closeImage", state.handle, null);
  };
  function makeBitmap(value) {
    var object = Object.create(ImageBitmap.prototype);
    bitmaps.set(object, {
      handle: value.handle, width: value.width, height: value.height,
      data: value.data === null ? null : new Uint8ClampedArray(value.data),
      closed: false
    });
    return object;
  }
  function bitmapOptions(source, crop, options) {
    options = options || {};
    Object.keys(options).forEach(function (name) {
      if (["imageOrientation", "premultiplyAlpha", "colorSpaceConversion",
           "resizeWidth", "resizeHeight", "resizeQuality"].indexOf(name) === -1) {
        missing("ImageBitmapOptions." + name);
      }
    });
    var orientation = options.imageOrientation === undefined ? "from-image" : options.imageOrientation;
    if (["from-image", "none", "flipY"].indexOf(orientation) === -1) {
      throw new TypeError("invalid ImageBitmapOptions.imageOrientation");
    }
    if (options.premultiplyAlpha !== undefined &&
        ["default", "none", "premultiply"].indexOf(options.premultiplyAlpha) === -1) {
      throw new TypeError("invalid ImageBitmapOptions.premultiplyAlpha");
    }
    if (options.colorSpaceConversion !== undefined &&
        ["default", "none"].indexOf(options.colorSpaceConversion) === -1) {
      throw new TypeError("invalid ImageBitmapOptions.colorSpaceConversion");
    }
    // Native images remain canonical straight RGBA. Alpha conversion happens
    // when a consumer requests premultiplied storage.
    return {
      source: source, crop: crop, flipY: orientation === "flipY",
      resizeWidth: options.resizeWidth === undefined ? null : integer(options.resizeWidth, "resizeWidth"),
      resizeHeight: options.resizeHeight === undefined ? null : integer(options.resizeHeight, "resizeHeight"),
      resizeQuality: options.resizeQuality === undefined ? "low" : String(options.resizeQuality),
      readPixels: true
    };
  }
  function create(source, crop, options, node) {
    var handle = bridge.nextHandle();
    var input;
    if (source instanceof Blob) {
      input = source.arrayBuffer().then(function (buffer) {
        if (buffer.byteLength > limit) { throw new RangeError("image decode limit reached in chuzz"); }
        return { kind: "encoded", data: buffer, node: node || null };
      });
    } else {
      try { input = Promise.resolve(globalThis.__chuzzGpuImageSource(source)); }
      catch (error) { input = Promise.reject(error); }
    }
    return input.then(function (value) {
      return bridge.send("createImageBitmap", handle, bitmapOptions(value, crop, options));
    }).then(makeBitmap);
  }
  if (typeof globalThis.ImageBitmap === "undefined") { globalThis.ImageBitmap = ImageBitmap; }
  if (typeof globalThis.createImageBitmap === "undefined") {
    globalThis.createImageBitmap = function (source, sx, sy, sw, sh, options) {
      var crop = null;
      if (arguments.length > 2) {
        crop = [sx, sy, sw, sh].map(function (value) {
          value = Number(value);
          if (!Number.isInteger(value) || value < -2147483648 || value > 2147483647) {
            throw new TypeError("invalid ImageBitmap crop");
          }
          return value;
        });
      } else {
        options = sx;
      }
      return create(source, crop, options);
    };
  }

  if (typeof globalThis.ImageData === "undefined") {
    globalThis.ImageData = function (data, width, height, options) {
      if (!(this instanceof ImageData)) { throw new TypeError("Illegal constructor"); }
      if (data instanceof Uint8ClampedArray) {
        width = integer(width, "ImageData.width");
        height = height === undefined ? data.length / (width * 4) : integer(height, "ImageData.height");
        height = integer(height, "ImageData.height");
        if (!width || !height || data.length !== length(width, height)) {
          throw new DOMException("invalid ImageData dimensions", "IndexSizeError");
        }
      } else {
        options = height;
        height = integer(width, "ImageData.height");
        width = integer(data, "ImageData.width");
        if (!width || !height) { throw new DOMException("empty ImageData", "IndexSizeError"); }
        data = new Uint8ClampedArray(length(width, height));
      }
      if (options && options.colorSpace && options.colorSpace !== "srgb") {
        missing("ImageData.colorSpace." + options.colorSpace);
      }
      Object.defineProperties(this, {
        width: { value: width, enumerable: true },
        height: { value: height, enumerable: true },
        data: { value: data, enumerable: true },
        colorSpace: { value: "srgb", enumerable: true }
      });
    };
    Object.defineProperty(ImageData.prototype, Symbol.toStringTag, { value: "ImageData" });
  }

  function installImage(element) {
    if (imageStates.has(element)) { return element; }
    var state = { epoch: 0, bitmap: null, source: "", complete: false, promise: Promise.resolve() };
    imageStates.set(element, state);
    marker(element);
    var setAttribute = element.setAttribute;
    function load(value) {
      state.source = String(value);
      state.complete = false;
      var epoch = ++state.epoch;
      if (state.bitmap) { state.bitmap.close(); state.bitmap = null; }
      setAttribute.call(element, "src", state.source);
      state.promise = fetch(state.source).then(function (response) {
        if (!response.ok) { throw new Error("image fetch failed"); }
        return response.blob();
      }).then(function (blob) {
        return create(blob, null, {}, marker(element));
      }).then(function (bitmap) {
        if (state.epoch !== epoch) { bitmap.close(); return; }
        state.bitmap = bitmap;
        state.complete = true;
        post("paintImage", 0, {
          node: marker(element), source: globalThis.__chuzzGpuImageSource(bitmap)
        });
        element.dispatchEvent(new Event("load"));
      }, function (error) {
        if (state.epoch === epoch) {
          state.complete = true;
          element.dispatchEvent(new Event("error"));
        }
        throw error;
      });
      state.promise.catch(function () {});
    }
    Object.defineProperties(element, {
      src: { configurable: true, get: function () { return state.source; }, set: load },
      complete: { configurable: true, get: function () { return state.complete; } },
      naturalWidth: { configurable: true, get: function () { return state.bitmap ? state.bitmap.width : 0; } },
      naturalHeight: { configurable: true, get: function () { return state.bitmap ? state.bitmap.height : 0; } }
    });
    ["width", "height"].forEach(function (name) {
      Object.defineProperty(element, name, {
        configurable: true,
        get: function () {
          var attribute = element.getAttribute(name);
          return attribute === null ? (state.bitmap ? state.bitmap[name] : 0)
            : integer(Number(attribute), name);
        },
        set: function (value) { setAttribute.call(element, name, String(integer(value, name))); }
      });
    });
    element.decode = function () { return state.promise.then(function () {}); };
    element.setAttribute = function (name, value) {
      if (String(name).toLowerCase() === "src") { load(value); }
      else { return setAttribute.call(element, name, value); }
    };
    return element;
  }
  ["createElement", "createElementNS"].forEach(function (name) {
    var original = document[name];
    document[name] = function () {
      var element = original.apply(this, arguments);
      var tag = String(element.tagName).toLowerCase();
      if (tag === "img") { installImage(element); }
      if (tag === "canvas") { bridge.reflectCanvas(element); }
      return element;
    };
  });
  if (typeof globalThis.Image === "undefined") {
    globalThis.Image = function (width, height) {
      var element = document.createElement("img");
      if (width !== undefined) { element.width = width; }
      if (height !== undefined) { element.height = height; }
      return element;
    };
  }

  globalThis.__chuzzGpuResize2D = function (canvas) {
    var state = canvasContexts.get(canvas);
    if (!state) { return; }
    state.context.__resize();
    state.width = canvas.width;
    state.height = canvas.height;
    state.data = null;
  };
  function pixelsOf(source) {
    var bitmap = bitmaps.get(source);
    if (bitmap && !bitmap.closed && bitmap.data) { return bitmap; }
    var image = imageStates.get(source);
    if (image && image.bitmap) { return pixelsOf(image.bitmap); }
    var canvas = offscreens.get(source) || source;
    var pixels = canvasPixels(canvas);
    if (pixels) { return pixels; }
    if (source && source.data instanceof Uint8ClampedArray) { return source; }
    // A WebGPU canvas or a decoded element: read its pixels back synchronously, after the GPU work queued before.
    var gpu = String(canvas && canvas.tagName).toLowerCase() === "canvas" ? bridge.canvasSource(canvas) : null;
    if (!gpu && String(source && source.tagName).toLowerCase() === "img") {
      gpu = { kind: "element", handle: marker(source) };
    }
    if (!gpu) { return missing("CanvasRenderingContext2D.drawImage source readback"); }
    var result = globalThis.__chuzzCanvasReadback(JSON.stringify(gpu), ++readbackSerial);
    if (typeof result[0] === "string") { throw new DOMException(String(result[1]), "InvalidStateError"); }
    return { width: result[0], height: result[1], data: result[2] };
  }
  function make2D(canvas) {
    var context = globalThis.__chuzzMake2D(canvas, marker(canvas), pixelsOf);
    canvasContexts.set(canvas, {
      context: context, width: canvas.width, height: canvas.height, data: null
    });
    return context;
  }
  var probe = document.createElement("canvas");
  var prototype = probe;
  while (prototype && !Object.prototype.hasOwnProperty.call(prototype, "getContext")) {
    prototype = Object.getPrototypeOf(prototype);
  }
  if (prototype) {
    var originalContext = prototype.getContext;
    Object.defineProperty(prototype, "getContext", {
      configurable: true, writable: true,
      value: function (type, options) {
        if (String(this.tagName).toLowerCase() !== "canvas" || String(type) !== "2d") {
          return originalContext.apply(this, arguments);
        }
        bridge.reflectCanvas(this);
        if (!bridge.claim2D(this)) { return null; }
        if (options && options.colorSpace && options.colorSpace !== "srgb") {
          missing("CanvasRenderingContext2D.colorSpace." + options.colorSpace);
        }
        var state = canvasContexts.get(this);
        return state ? state.context : make2D(this);
      }
    });
  }
  if (typeof globalThis.OffscreenCanvas === "undefined") {
    globalThis.OffscreenCanvas = function (width, height) {
      if (!(this instanceof OffscreenCanvas)) { throw new TypeError("Illegal constructor"); }
      var canvas = document.createElement("canvas");
      canvas.width = integer(width, "OffscreenCanvas.width");
      canvas.height = integer(height, "OffscreenCanvas.height");
      offscreens.set(this, canvas);
    };
    ["width", "height"].forEach(function (name) {
      Object.defineProperty(OffscreenCanvas.prototype, name, {
        get: function () { return offscreens.get(this)[name]; },
        set: function (value) { offscreens.get(this)[name] = integer(value, name); }
      });
    });
    OffscreenCanvas.prototype.getContext = function () {
      return offscreens.get(this).getContext.apply(offscreens.get(this), arguments);
    };
    OffscreenCanvas.prototype.convertToBlob = function () {
      return Promise.reject(new Error("not implemented in chuzz: OffscreenCanvas.convertToBlob"));
    };
    OffscreenCanvas.prototype.transferToImageBitmap = function () {
      return missing("OffscreenCanvas.transferToImageBitmap");
    };
  }
})();

