// JavaScript syntax only. Boa evaluates this source directly.
(function () {
  var bridge = globalThis.__chuzzGpuImageBridge;
  if (!bridge) { return; }
  var resolveTransport;
  var transportReady = new Promise(function (resolve) { resolveTransport = resolve; });
  // The host resolves this after installing IPC and the reply poll hook.
  // Resolving more than once does not release another batch of starts.
  bridge.imagesReady = function () { resolveTransport(); };
  var bitmaps = new WeakMap();
  var imageStates = new WeakMap();
  var canvasContexts = new WeakMap();
  var offscreens = new WeakMap();
  var markers = new WeakMap();
  var sourceFinalizers = new FinalizationRegistry(function (handle) {
    globalThis.__chuzzCanvas2D(JSON.stringify({ action: "release", id: handle }), []);
  });
  var bitmapFinalizers = new FinalizationRegistry(function (handle) {
    post("closeImage", handle, null);
  });
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
  function marker(element, renew) {
    var value = markers.get(element);
    if (renew && value !== undefined) {
      sourceFinalizers.unregister(element);
      globalThis.__chuzzCanvas2D(JSON.stringify({ action: "release", id: value }), []);
      value = undefined;
    }
    if (value === undefined) {
      value = bridge.nextHandle();
      markers.set(element, value);
      element.setAttribute("data-chuzz-gpu-source", String(value));
      sourceFinalizers.register(element, value, element);
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
    var image = isImage(source) ? imageState(source) : imageStates.get(source);
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
    globalThis.__chuzzSubmit2D();
    state.closed = true;
    bitmapFinalizers.unregister(this);
    post("closeImage", state.handle, null);
  };
  function makeBitmap(value) {
    var object = Object.create(ImageBitmap.prototype);
    bitmaps.set(object, {
      handle: value.handle, width: value.width, height: value.height,
      closed: false
    });
    bitmapFinalizers.register(object, value.handle, object);
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
    // Native images remain straight RGBA, including RGB under alpha zero.
    // Keep those decoder bytes intact here; the external texture copy converts
    // only when its premultipliedAlpha destination option requests it.
    return {
      source: source, crop: crop, flipY: orientation === "flipY",
      resizeWidth: options.resizeWidth === undefined ? null : integer(options.resizeWidth, "resizeWidth"),
      resizeHeight: options.resizeHeight === undefined ? null : integer(options.resizeHeight, "resizeHeight"),
      resizeQuality: options.resizeQuality === undefined ? "low" : String(options.resizeQuality),
      readPixels: false
    };
  }
  function create(source, crop, options) {
    var handle = bridge.nextHandle();
    var input;
    if (source instanceof Blob) {
      input = source.arrayBuffer().then(function (buffer) {
        if (buffer.byteLength > limit) { throw new RangeError("image decode limit reached in chuzz"); }
        // A node snapshot has no URL identity and may contain a previous src.
        return { kind: "encoded", data: buffer };
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

  function isImage(element) {
    return !!element && element.nodeType === 1 &&
      String(element.tagName).toLowerCase() === "img" &&
      (!element.namespaceURI || element.namespaceURI === "http://www.w3.org/1999/xhtml");
  }
  function imageError(message) {
    return new DOMException(message || "image is not decoded", "EncodingError");
  }
  function imageUrl(element, source) {
    return new URL(source, element.baseURI || location.href).href;
  }
  function startImage(element, state, source, explicit) {
    var epoch = ++state.epoch;
    if (state.reject) { state.reject(imageError("image source changed")); }
    if (state.controller) { state.controller.abort(); }
    if (state.bitmap) { state.bitmap.close(); state.bitmap = null; }
    // Reserve the source and epoch before waiting for connection or transport.
    // Every discovery path sees this same request, including deferred starts.
    state.source = source;
    state.complete = source === null || source === "";
    state.controller = null;
    state.start = null;
    var resolve, reject;
    state.promise = new Promise(function (yes, no) { resolve = yes; reject = no; });
    state.promise.catch(function () {});
    state.reject = reject;
    // A paint already queued for an older request must not match this node.
    var node = marker(element, true);
    var eventQueued = false;
    function current() {
      // A markup image that left the document (the streaming parser rebuilds
      // parsed markup when it runs scripts) must not complete a load: its
      // replacement node loads on its own. Script-set sources keep loading
      // while detached, as `new Image().src` does in browsers.
      return state.epoch === epoch && element.ownerDocument === document &&
        element.getAttribute("src") === source && (explicit || element.isConnected);
    }
    function event(type, error) {
      if (!current() || eventQueued) { return; }
      eventQueued = true;
      setTimeout(function () {
        if (!current()) { return; }
        element.dispatchEvent(new Event(type));
        if (!current()) { return; }
        state.reject = null;
        if (error) { reject(error); }
        else { resolve(); }
      }, 0);
    }
    function failed() {
      if (!current() || eventQueued) { return; }
      if (state.bitmap) { state.bitmap.close(); state.bitmap = null; }
      state.controller = null;
      state.complete = true;
      event("error", imageError("image could not be loaded or decoded"));
    }
    if (source === null) {
      state.reject = null;
      reject(imageError("image has no source"));
      return;
    }
    state.start = function () {
      if (!current() || (!explicit && !element.isConnected)) { return; }
      // Clear synchronously, before attaching the readiness continuation.
      // Scans cannot enqueue a second continuation for this epoch.
      state.start = null;
      transportReady.then(function () {
        if (!current()) { throw imageError("image source changed"); }
        if (source === "") { throw imageError("image has an empty source"); }
        if (typeof AbortController === "function") { state.controller = new AbortController(); }
        var controller = state.controller;
        return fetch(imageUrl(element, source), controller ? { signal: controller.signal } : undefined);
      }).then(function (response) {
        if (!current()) { throw imageError("image source changed"); }
        if (!response.ok) { throw imageError("image fetch failed"); }
        return response.blob();
      }).then(function (blob) {
        if (!current()) { throw imageError("image source changed"); }
        return create(blob, null, {});
      }).then(function (bitmap) {
        if (!current()) { bitmap.close(); return; }
        state.bitmap = bitmap;
        state.complete = true;
        state.controller = null;
        return bridge.send("paintImage", 0, {
          node: node, source: globalThis.__chuzzGpuImageSource(bitmap)
        }).then(function () {
          if (current()) { event("load"); }
        });
      }).catch(failed);
    };
  }
  function imageState(element, force) {
    var state = imageStates.get(element);
    if (!state) {
      state = {
        epoch: 0, bitmap: null, source: undefined, complete: true,
        controller: null, start: null, reject: null, promise: Promise.reject(imageError())
      };
      state.promise.catch(function () {});
      imageStates.set(element, state);
    }
    // DOMParser and template owner documents remain inert until adoption.
    // srcset/sizes selection is not implemented; src is the fallback.
    var source = element.getAttribute("src");
    if (element.ownerDocument === document) {
      // Explicit src writes still load detached script-created images and
      // deliberately reassigning the same src still creates a new epoch.
      if (force || state.source !== source) { startImage(element, state, source, force); }
      if (state.start) { state.start(); }
    } else {
      state.complete = source === null || source === "";
    }
    return state;
  }
  function propertyDescriptor(prototype, name) {
    while (prototype) {
      var descriptor = Object.getOwnPropertyDescriptor(prototype, name);
      if (descriptor) { return descriptor; }
      prototype = Object.getPrototypeOf(prototype);
    }
    return null;
  }
  var imageProbe = document.createElement("img");
  var imagePrototype = Object.getPrototypeOf(imageProbe);
  function imageProperty(name, get, set) {
    var previous = propertyDescriptor(imagePrototype, name);
    var descriptor = {
      configurable: true, enumerable: previous ? previous.enumerable : true,
      get: function () {
        if (isImage(this)) { return get.call(this); }
        return previous ? (previous.get ? previous.get.call(this) : previous.value) : undefined;
      }
    };
    if (set || (previous && previous.set)) {
      descriptor.set = function (value) {
        if (isImage(this)) {
          if (set) { set.call(this, value); }
        } else if (previous && previous.set) {
          previous.set.call(this, value);
        } else if (!previous || previous.writable !== false) {
          Object.defineProperty(this, name, {
            configurable: true, enumerable: true, writable: true, value: value
          });
        }
      };
    }
    Object.defineProperty(imagePrototype, name, descriptor);
  }
  imageProperty("src", function () {
    imageState(this);
    var source = this.getAttribute("src");
    if (source === null) { return ""; }
    try { return imageUrl(this, source); }
    catch (error) { return source; }
  }, function (value) { this.setAttribute("src", String(value)); });
  imageProperty("complete", function () { return imageState(this).complete; });
  imageProperty("naturalWidth", function () {
    var state = imageState(this);
    return state.bitmap ? state.bitmap.width : 0;
  });
  imageProperty("naturalHeight", function () {
    var state = imageState(this);
    return state.bitmap ? state.bitmap.height : 0;
  });
  ["width", "height"].forEach(function (name) {
    imageProperty(name, function () {
      var state = imageState(this);
      var attribute = this.getAttribute(name);
      return attribute === null ? (state.bitmap ? state.bitmap[name] : 0)
        : integer(Number(attribute), name);
    }, function (value) { this.setAttribute(name, String(integer(value, name))); });
  });
  var previousDecode = propertyDescriptor(imagePrototype, "decode");
  Object.defineProperty(imagePrototype, "decode", {
    configurable: true, writable: true,
    value: function () {
      if (!isImage(this)) {
        var previous = previousDecode &&
          (previousDecode.get ? previousDecode.get.call(this) : previousDecode.value);
        if (typeof previous !== "function") { throw new TypeError("invalid HTMLImageElement"); }
        return previous.apply(this, arguments);
      }
      return imageState(this).promise.then(function () {});
    }
  });
  function wrapMethod(probe, name, wrap) {
    var owner = probe;
    while (owner && !Object.prototype.hasOwnProperty.call(owner, name)) {
      owner = Object.getPrototypeOf(owner);
    }
    if (!owner) { return; }
    var descriptor = Object.getOwnPropertyDescriptor(owner, name);
    if (typeof descriptor.value !== "function") { return; }
    descriptor.value = wrap(descriptor.value);
    Object.defineProperty(owner, name, descriptor);
  }
  ["setAttribute", "removeAttribute", "toggleAttribute",
   "setAttributeNS", "removeAttributeNS"].forEach(function (name) {
    wrapMethod(imageProbe, name, function (original) {
      return function () {
        var args = Array.from(arguments);
        var namespaced = name === "setAttributeNS" || name === "removeAttributeNS";
        var index = namespaced ? 1 : 0;
        var image = isImage(this);
        if (image && args.length > index) { args[index] = String(args[index]); }
        var source = image && args.length > index &&
          args[index].toLowerCase() === "src" &&
          (!namespaced || args[0] === null || args[0] === "");
        var result = original.apply(this, args);
        if (source) { imageState(this, true); }
        return result;
      };
    });
  });

  var imageObserver;
  var observedRoots = new WeakSet();
  var observedShadows = [];
  function observeImages(root) {
    if (!imageObserver || observedRoots.has(root)) { return; }
    observedRoots.add(root);
    if (root.host) { observedShadows.push(new WeakRef(root)); }
    imageObserver.observe(root, {
      subtree: true, childList: true, attributes: true, attributeFilter: ["src"]
    });
  }
  function walkImages(root, visit, templates) {
    var pending = [root];
    while (pending.length) {
      var node = pending.pop();
      if (!node) { continue; }
      if (isImage(node)) { visit(node); }
      if (templates && String(node.tagName).toLowerCase() === "template" && node.content) {
        pending.push(node.content);
      }
      if (node.shadowRoot) {
        if (!templates) { observeImages(node.shadowRoot); }
        pending.push(node.shadowRoot);
      }
      var children = node.childNodes;
      if (children) {
        for (var index = children.length - 1; index >= 0; index--) { pending.push(children[index]); }
      }
    }
  }
  function connectedImage(element) {
    if (isImage(element) && element.ownerDocument === document && element.isConnected) {
      imageState(element);
    }
  }
  function scanLightImages(root) {
    connectedImage(root);
    // Keep childList processing in native queries rather than allocating a
    // JavaScript wrapper for every descendant of an image-free SVG subtree.
    var images;
    if (typeof root.getElementsByTagName === "function") {
      images = root.getElementsByTagName("img");
    } else if (typeof root.querySelectorAll === "function") {
      images = root.querySelectorAll("img");
    } else {
      return;
    }
    for (var index = 0; index < images.length; index++) { connectedImage(images[index]); }
  }
  function scanImages(root) {
    if (!root || (root.nodeType !== 1 && root.nodeType !== 9 && root.nodeType !== 11)) { return; }
    if (root !== document && (root.ownerDocument !== document || !root.isConnected)) { return; }
    if (root.shadowRoot) { observeImages(root.shadowRoot); }
    scanLightImages(root);
    // Queries do not cross shadow boundaries. Roots attached while a host
    // was detached must be scanned when that host, or an ancestor, connects.
    // Retain weak references so removed hosts are not kept alive by this list.
    for (var index = 0; index < observedShadows.length;) {
      var shadow = observedShadows[index].deref();
      if (!shadow) { observedShadows.splice(index, 1); continue; }
      index++;
      var host = shadow.host;
      if (!host || host.ownerDocument !== document || !host.isConnected) { continue; }
      while (host) {
        if (root === host || (typeof root.contains === "function" && root.contains(host))) {
          scanLightImages(shadow);
          break;
        }
        // A nested host can itself live inside an open or closed shadow root.
        var tree = host.getRootNode();
        host = tree && tree.host;
      }
    }
  }
  function copiedImages(copy) {
    // cloneNode/importNode copy attributes, including a native paint marker.
    // They must not alias the original image's marker or request state.
    walkImages(copy, function (element) {
      element.removeAttribute("data-chuzz-gpu-source");
    }, true);
    return copy;
  }
  wrapMethod(imageProbe, "cloneNode", function (original) {
    return function () { return copiedImages(original.apply(this, arguments)); };
  });
  wrapMethod(document, "importNode", function (original) {
    return function () { return copiedImages(original.apply(this, arguments)); };
  });
  if (typeof MutationObserver === "function") {
    imageObserver = new MutationObserver(function (records) {
      records.forEach(function (record) {
        if (record.type === "attributes") {
          if (isImage(record.target)) { imageState(record.target); }
        } else if (record.type === "childList") {
          for (var index = 0; index < record.addedNodes.length; index++) {
            scanImages(record.addedNodes[index]);
          }
        }
      });
    });
    observeImages(document);
  }
  wrapMethod(imageProbe, "attachShadow", function (original) {
    return function () {
      var root = original.apply(this, arguments);
      observeImages(root);
      scanImages(root);
      return root;
    };
  });
  ["write", "writeln"].forEach(function (name) {
    wrapMethod(document, name, function (original) {
      return function () {
        var result = original.apply(this, arguments);
        if (this === document) { scanImages(this); }
        return result;
      };
    });
  });
  document.addEventListener("DOMContentLoaded", function () { scanImages(document); });
  // Discover any pre-existing open roots once. Subsequent scans use native
  // image queries and the roots registered here or by attachShadow.
  walkImages(document, connectedImage, false);

  ["createElement", "createElementNS"].forEach(function (name) {
    var original = document[name];
    document[name] = function () {
      var element = original.apply(this, arguments);
      var tag = String(element.tagName).toLowerCase();
      if (tag === "canvas") { bridge.reflectCanvas(element); }
      return element;
    };
  });
  // Replace the standalone preloader fallback once real image transport is
  // available. Constructed images must use the same decoded DOM path as img.
  {
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
    if (bitmap) {
      if (bitmap.closed) { throw new DOMException("ImageBitmap is closed", "InvalidStateError"); }
      return { nativeBitmap: bitmap.handle, width: bitmap.width, height: bitmap.height };
    }
    var image = isImage(source) ? imageState(source) : imageStates.get(source);
    if (image) {
      if (!image.bitmap) { throw new DOMException("image is not decoded", "InvalidStateError"); }
      return pixelsOf(image.bitmap);
    }
    var canvas = offscreens.get(source) || source;
    var state = canvasContexts.get(canvas);
    if (state) { return state.context.__source(); }
    if (source && source.data instanceof Uint8ClampedArray) { return source; }
    var isCanvas = String(canvas && canvas.tagName).toLowerCase() === "canvas";
    var gpu = isCanvas ? bridge.canvasSource(canvas) : null;
    if (isCanvas && !gpu) {
      var context = canvas.getContext("2d");
      if (context) { return context.__source(); }
    }
    if (!gpu && String(source && source.tagName).toLowerCase() === "img") {
      gpu = { kind: "element", handle: marker(source) };
    }
    if (!gpu) { return missing("CanvasRenderingContext2D.drawImage source readback"); }
    // Pin preceding uses before replacing this native source's current version.
    // Texture readback remains native, including its alpha conversion.
    globalThis.__chuzzSubmit2D();
    var handle = marker(isCanvas ? canvas : source);
    var result = globalThis.__chuzzCanvas2D(JSON.stringify({
      action: "source", id: handle, serial: ++readbackSerial, source: gpu
    }), []);
    return { nativeImage: handle, width: result[0], height: result[1] };
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
