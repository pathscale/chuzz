// JavaScript syntax only. Boa evaluates this source directly.
(function () {
  var dirty = new Set();
  var brushes = new WeakMap();
  var active = null;
  var identity = [1, 0, 0, 1, 0, 0];

  function missing(member) {
    throw new Error("not implemented in chuzz: CanvasRenderingContext2D." + member);
  }
  function multiply(a, b) {
    return [
      a[0] * b[0] + a[2] * b[1], a[1] * b[0] + a[3] * b[1],
      a[0] * b[2] + a[2] * b[3], a[1] * b[2] + a[3] * b[3],
      a[0] * b[4] + a[2] * b[5] + a[4],
      a[1] * b[4] + a[3] * b[5] + a[5]
    ];
  }
  function matrix(value) {
    value = value || {};
    if (value.is2D === false) { missing("setTransform.3d"); }
    var aliases = ["m11", "m12", "m21", "m22", "m41", "m42"];
    return ["a", "b", "c", "d", "e", "f"].map(function (name, index) {
      var a = value[name], b = value[aliases[index]];
      if (a !== undefined && b !== undefined && Number(a) !== Number(b)) {
        throw new TypeError("inconsistent transform matrix");
      }
      return Number(a !== undefined ? a : b !== undefined ? b : identity[index]);
    });
  }
  function finite(n) { return n.every(Number.isFinite); }
  function native(request, buffers) {
    return globalThis.__chuzzCanvas2D(JSON.stringify(request), buffers || []);
  }
  // Pages assign the same few colour and font strings on every draw; each
  // check is a JSON round trip to the host, so remember the answers.
  var MEMO_LIMIT = 1024;
  function memo(map, key, compute) {
    if (map.has(key)) { return map.get(key); }
    var value = compute();
    if (map.size >= MEMO_LIMIT) { map.clear(); }
    map.set(key, value);
    return value;
  }
  var colorMemo = new Map();
  var fontMemo = new Map();
  function validColor(value) {
    return memo(colorMemo, value, function () { return native({ action: "color", value: value }); });
  }
  function fillRule(value) {
    if (value === undefined || value === "nonzero") { return 0; }
    if (value === "evenodd") { return 1; }
    if (typeof value === "object") { return missing("fill.Path2D"); }
    throw new TypeError("invalid canvas fill rule");
  }

  globalThis.__chuzzMake2D = function (canvas, id, pixelsOf) {
    var context = { canvas: canvas };
    var commands = [], buffers = [], references = [], transferBytes = 0, stack = [];
    var width = canvas.width, height = canvas.height;
    var state;
    function order() {
      var previous = active;
      active = context;
      if (previous && previous !== context) { previous.__submit(); }
    }
    function defaults() {
      state = {
        fillStyle: "#000000", strokeStyle: "#000000", globalAlpha: 1,
        globalCompositeOperation: "source-over", lineWidth: 1, lineCap: "butt",
        lineJoin: "miter", miterLimit: 10, lineDashOffset: 0, dash: [],
        font: "10px sans-serif", textAlign: "start", textBaseline: "alphabetic",
        direction: "inherit", imageSmoothingEnabled: true,
        imageSmoothingQuality: "low", matrix: identity.slice()
      };
      stack = [];
    }
    function reset() {
      order();
      width = canvas.width; height = canvas.height;
      commands = []; buffers = []; references = []; transferBytes = 0;
      defaults();
      native({ action: "reset", id: id, width: width, height: height });
      dirty.add(context);
    }
    function ensure() {
      if (width !== canvas.width || height !== canvas.height) { reset(); }
    }
    function snapshot() {
      var direction = state.direction;
      if (direction === "inherit") {
        direction = typeof getComputedStyle === "function" ? getComputedStyle(canvas).direction : "ltr";
        if (direction !== "rtl") { direction = "ltr"; }
      }
      return {
        matrix: state.matrix.slice(), alpha: state.globalAlpha,
        composite: state.globalCompositeOperation, width: state.lineWidth,
        cap: state.lineCap, join: state.lineJoin, miter: state.miterLimit,
        dash: state.dash.slice(), dashOffset: state.lineDashOffset,
        font: state.font, align: state.textAlign, baseline: state.textBaseline,
        direction: direction, smoothing: state.imageSmoothingEnabled
      };
    }
    function flush(action, extra) {
      if (action !== "submit") { order(); }
      ensure();
      var request = { action: action, id: id, width: width, height: height, commands: commands };
      if (extra) {
        Object.keys(extra).forEach(function (name) { request[name] = extra[name]; });
      }
      var queuedBuffers = buffers, queuedReferences = references;
      commands = []; buffers = []; references = []; transferBytes = 0;
      try {
        return native(request, queuedBuffers);
      } finally {
        queuedReferences.length = 0;
        if (active === context) { active = null; }
      }
    }
    function room(bytes) {
      if (commands.length >= 4096 || buffers.length >= 512 || transferBytes + bytes > 32 * 1024 * 1024) {
        flush("submit");
      }
    }
    function source(image) {
      if (image.nativeCanvas !== undefined || image.nativeBitmap !== undefined || image.nativeImage !== undefined) {
        room(0);
        var descriptor = {
          index: 0, width: image.width, height: image.height,
          offset: 0, length: image.width * image.height * 4
        };
        if (image.nativeCanvas !== undefined) { descriptor.canvas = image.nativeCanvas; }
        if (image.nativeBitmap !== undefined) { descriptor.bitmap = image.nativeBitmap; }
        if (image.nativeImage !== undefined) { descriptor.native = image.nativeImage; }
        return descriptor;
      }
      if (image.nativeVideo !== undefined) {
        room(0);
        return {
          index: 0, width: image.width, height: image.height,
          offset: 0, length: image.width * image.height * 4,
          video: image.nativeVideo
        };
      }
      if (image.data.byteLength > 64 * 1024 * 1024) { throw new RangeError("canvas image limit reached"); }
      room(image.data.byteLength);
      // Snapshot mutable page bytes now. Native descriptors never contain pixels.
      var data = image.data.slice();
      var index = buffers.length;
      buffers.push(data.buffer);
      transferBytes += data.byteLength;
      return {
        index: index, width: image.width, height: image.height,
        offset: data.byteOffset, length: data.byteLength
      };
    }
    function brush(value) {
      var resource = brushes.get(value);
      if (!resource) { return { kind: "color", color: String(value) }; }
      if (resource.kind === "pattern") {
        var image = source(pixelsOf(resource.source));
        references.push(resource.source);
        return {
          kind: "pattern", matrix: multiply(state.matrix, resource.matrix),
          repetition: resource.repetition, image: image
        };
      }
      return {
        kind: resource.kind, n: resource.n.slice(), matrix: resource.matrix.slice(),
        stops: resource.stops.map(function (stop) { return stop.slice(); })
      };
    }
    function record(op, n, drawing, value, text) {
      order();
      ensure();
      room(0);
      var command = { op: op, n: n || [], matrix: state.matrix.slice() };
      if (drawing) {
        command.state = snapshot();
        command.brush = brush(value);
        dirty.add(context);
      }
      if (text !== undefined) { command.text = text; }
      commands.push(command);
      active = context;
    }
    function property(name, validate) {
      Object.defineProperty(context, name, {
        enumerable: true,
        get: function () { ensure(); return state[name]; },
        set: function (value) { ensure(); var next = validate(value); if (next !== undefined) { state[name] = next; } }
      });
    }
    function positive(value) { value = Number(value); return Number.isFinite(value) && value > 0 ? value : undefined; }
    function enumeration(values) {
      return function (value) { value = String(value); return values.indexOf(value) >= 0 ? value : undefined; };
    }
    ["fillStyle", "strokeStyle"].forEach(function (name) {
      property(name, function (value) {
        if (brushes.has(value)) { return value; }
        value = String(value);
        return validColor(value) ? value : undefined;
      });
    });
    property("globalAlpha", function (value) {
      value = Number(value);
      return Number.isFinite(value) && value >= 0 && value <= 1 ? value : undefined;
    });
    property("globalCompositeOperation", function (value) {
      value = String(value);
      if (["source-over", "lighter", "multiply", "destination-out"].indexOf(value) < 0) {
        missing("globalCompositeOperation." + value);
      }
      return value;
    });
    property("lineWidth", positive);
    property("miterLimit", positive);
    property("lineCap", enumeration(["butt", "round", "square"]));
    property("lineJoin", enumeration(["round", "bevel", "miter"]));
    property("lineDashOffset", function (value) { value = Number(value); return Number.isFinite(value) ? value : undefined; });
    property("textAlign", enumeration(["start", "end", "left", "right", "center"]));
    property("textBaseline", enumeration(["top", "hanging", "middle", "alphabetic", "ideographic", "bottom"]));
    property("direction", enumeration(["inherit", "ltr", "rtl"]));
    property("imageSmoothingEnabled", Boolean);
    property("imageSmoothingQuality", enumeration(["low", "medium", "high"]));
    property("font", function (value) {
      var base = 16, root = 16;
      value = String(value);
      // Only relative sizes depend on the element and root font sizes, and
      // resolving those styles costs more than the font parse itself.
      if (/em|%|larger|smaller/i.test(value) && typeof getComputedStyle === "function") {
        base = parseFloat(getComputedStyle(canvas).fontSize) || 16;
        root = parseFloat(getComputedStyle(document.documentElement).fontSize) || 16;
      }
      try {
        return memo(fontMemo, value + "|" + base + "|" + root, function () {
          return native({ action: "font", value: value, fontBase: base, rootBase: root });
        });
      } catch (error) {
        if (String(error).indexOf("not implemented in chuzz:") >= 0) { throw error; }
        return undefined;
      }
    });
    ["shadowBlur", "shadowColor", "shadowOffsetX", "shadowOffsetY", "filter",
     "fontKerning", "fontStretch", "fontVariantCaps", "letterSpacing", "wordSpacing",
     "textRendering"].forEach(function (name) {
      Object.defineProperty(context, name, {
        get: function () { return missing(name); },
        set: function () { return missing(name); }
      });
    });
    context.beginPath = function () { record("beginPath"); };
    context.closePath = function () { record("closePath"); };
    [["moveTo", 2], ["lineTo", 2], ["rect", 4], ["quadraticCurveTo", 4],
     ["bezierCurveTo", 6], ["arcTo", 5]].forEach(function (entry) {
      context[entry[0]] = function () {
        var n = Array.prototype.slice.call(arguments, 0, entry[1]).map(Number);
        if (n.length !== entry[1] || !finite(n)) { return; }
        if (entry[0] === "arcTo" && n[4] < 0) {
          throw new DOMException("negative radius", "IndexSizeError");
        }
        record(entry[0], n);
      };
    });
    context.arc = function (x, y, r, start, end, ccw) {
      var n = [x, y, r, start, end].map(Number);
      if (!finite(n)) { return; }
      if (n[2] < 0) { throw new DOMException("negative radius", "IndexSizeError"); }
      n.push(ccw ? 1 : 0); record("arc", n);
    };
    context.ellipse = function (x, y, rx, ry, rotation, start, end, ccw) {
      var n = [x, y, rx, ry, rotation, start, end].map(Number);
      if (!finite(n)) { return; }
      if (n[2] < 0 || n[3] < 0) { throw new DOMException("negative radius", "IndexSizeError"); }
      n.push(ccw ? 1 : 0); record("ellipse", n);
    };
    context.roundRect = function (x, y, w, h, radii) {
      var n = [x, y, w, h].map(Number);
      if (!finite(n)) { return; }
      radii = radii === undefined ? [0] : Array.isArray(radii) ? radii : [radii];
      if (!radii.length || radii.length > 4) { throw new RangeError("invalid roundRect radii"); }
      var points = radii.map(function (r) {
        return typeof r === "object" ? [Number(r.x), Number(r.y)] : [Number(r), Number(r)];
      });
      if (points.some(function (r) { return r[0] < 0 || r[1] < 0; })) { throw new RangeError("negative radius"); }
      if (!points.every(finite)) { return; }
      var corners = points.length === 1 ? [points[0], points[0], points[0], points[0]]
        : points.length === 2 ? [points[0], points[1], points[0], points[1]]
        : points.length === 3 ? [points[0], points[1], points[2], points[1]] : points;
      if (n[2] < 0) {
        n[0] += n[2]; n[2] = -n[2];
        corners = [corners[1], corners[0], corners[3], corners[2]];
      }
      if (n[3] < 0) {
        n[1] += n[3]; n[3] = -n[3];
        corners = [corners[3], corners[2], corners[1], corners[0]];
      }
      corners.forEach(function (r) { n.push(r[0], r[1]); });
      record("roundRect", n);
    };
    context.fill = function (value) { record("fill", [fillRule(value)], true, state.fillStyle); };
    context.stroke = function (path) {
      if (path !== undefined) { return missing("stroke.Path2D"); }
      record("stroke", [], true, state.strokeStyle);
    };
    context.clip = function (value) { record("clip", [fillRule(value)]); };
    ["fillRect", "strokeRect", "clearRect"].forEach(function (name) {
      context[name] = function (x, y, w, h) {
        var n = [x, y, w, h].map(Number);
        if (!finite(n) || !n[2] || !n[3]) { return; }
        record(name, n, true, name === "strokeRect" ? state.strokeStyle : state.fillStyle);
      };
    });
    context.save = function () {
      ensure();
      if (stack.length >= 256) { throw new RangeError("canvas save limit reached in chuzz"); }
      var saved = Object.assign({}, state);
      saved.matrix = state.matrix.slice(); saved.dash = state.dash.slice();
      stack.push(saved); record("save");
    };
    context.restore = function () {
      ensure();
      if (!stack.length) { return; }
      state = stack.pop(); record("restore");
    };
    context.setLineDash = function (segments) {
      ensure();
      var values = Array.from(segments, Number);
      if (values.length > 1024) { throw new RangeError("canvas dash limit reached in chuzz"); }
      if (!finite(values) || values.some(function (n) { return n < 0; })) { return; }
      state.dash = values.length % 2 ? values.concat(values) : values;
    };
    context.getLineDash = function () { ensure(); return state.dash.slice(); };
    context.transform = function (a, b, c, d, e, f) {
      ensure(); var n = [a, b, c, d, e, f].map(Number);
      if (finite(n)) { state.matrix = multiply(state.matrix, n); }
    };
    context.setTransform = function (a, b, c, d, e, f) {
      ensure();
      var n = arguments.length <= 1 ? matrix(a) : [a, b, c, d, e, f].map(Number);
      if (finite(n)) { state.matrix = n; }
    };
    context.resetTransform = function () { ensure(); state.matrix = identity.slice(); };
    context.translate = function (x, y) { context.transform(1, 0, 0, 1, Number(x), Number(y)); };
    context.scale = function (x, y) { context.transform(Number(x), 0, 0, Number(y), 0, 0); };
    context.rotate = function (angle) {
      angle = Number(angle); context.transform(Math.cos(angle), Math.sin(angle), -Math.sin(angle), Math.cos(angle), 0, 0);
    };
    context.getTransform = function () {
      ensure();
      if (typeof DOMMatrix === "function") { return new DOMMatrix(state.matrix); }
      return { a: state.matrix[0], b: state.matrix[1], c: state.matrix[2],
        d: state.matrix[3], e: state.matrix[4], f: state.matrix[5], is2D: true };
    };
    function gradient(kind, n) {
      ensure();
      n = n.map(Number);
      if (!finite(n)) { throw new TypeError("invalid gradient coordinates"); }
      if (kind === "radial" && (n[2] < 0 || n[5] < 0)) {
        throw new DOMException("negative radius", "IndexSizeError");
      }
      var object = {};
      var resource = { kind: kind, n: n, matrix: state.matrix.slice(), stops: [] };
      brushes.set(object, resource);
      object.addColorStop = function (offset, value) {
        offset = Number(offset); value = String(value);
        if (!Number.isFinite(offset) || offset < 0 || offset > 1) {
          throw new DOMException("invalid colour stop offset", "IndexSizeError");
        }
        if (!validColor(value)) { throw new DOMException("invalid colour stop", "SyntaxError"); }
        if (resource.stops.length >= 4096) { throw new RangeError("canvas gradient stop limit reached in chuzz"); }
        resource.stops.push([offset, value]);
      };
      return object;
    }
    context.createLinearGradient = function (x0, y0, x1, y1) { return gradient("linear", [x0, y0, x1, y1]); };
    context.createRadialGradient = function (x0, y0, r0, x1, y1, r1) { return gradient("radial", [x0, y0, r0, x1, y1, r1]); };
    context.createPattern = function (image, repetition) {
      ensure();
      repetition = repetition == null || repetition === "" ? "repeat" : String(repetition);
      if (["repeat", "repeat-x", "repeat-y", "no-repeat"].indexOf(repetition) < 0) {
        throw new DOMException("invalid pattern repetition", "SyntaxError");
      }
      if (image && image.complete === false) { return null; }
      var pixels = pixelsOf(image);
      if (!pixels.width || !pixels.height) { return null; }
      var object = {}, resource = { kind: "pattern", source: image, repetition: repetition, matrix: identity.slice() };
      brushes.set(object, resource);
      object.setTransform = function (value) {
        var n = matrix(value); if (finite(n)) { resource.matrix = n; }
      };
      return object;
    };
    ["fillText", "strokeText"].forEach(function (name) {
      context[name] = function (text, x, y, maxWidth) {
        var n = [Number(x), Number(y), maxWidth === undefined ? 0 : Number(maxWidth)];
        if (!finite(n) || (maxWidth !== undefined && n[2] <= 0)) { return; }
        record(name, n, true, name === "fillText" ? state.fillStyle : state.strokeStyle, String(text));
      };
    });
    context.measureText = function (text) {
      ensure();
      return JSON.parse(flush("measure", { text: String(text), state: snapshot() }));
    };
    context.createImageData = function (width, height) {
      if (typeof width === "object") { height = width.height; width = width.width; }
      return new ImageData(Math.abs(Math.trunc(width)), Math.abs(Math.trunc(height)));
    };
    context.getImageData = function (x, y, w, h, options) {
      if (options && options.colorSpace && options.colorSpace !== "srgb") { missing("getImageData.colorSpace"); }
      var n = [x, y, w, h].map(function (value) { return Math.trunc(Number(value)); });
      if (!finite(n)) { throw new TypeError("invalid image rectangle"); }
      if (n[2] < 0) { n[0] += n[2]; n[2] = -n[2]; }
      if (n[3] < 0) { n[1] += n[3]; n[3] = -n[3]; }
      if (!n[2] || !n[3]) { throw new DOMException("empty image rectangle", "IndexSizeError"); }
      return new ImageData(flush("read", { n: n }), n[2], n[3]);
    };
    context.putImageData = function (image, x, y, dirtyX, dirtyY, dirtyWidth, dirtyHeight) {
      order();
      ensure();
      if (!image || !(image.data instanceof Uint8ClampedArray)) { throw new TypeError("expected ImageData"); }
      if (image.colorSpace && image.colorSpace !== "srgb") { missing("putImageData.colorSpace"); }
      var n = [x, y, dirtyX === undefined ? 0 : dirtyX, dirtyY === undefined ? 0 : dirtyY,
        dirtyWidth === undefined ? image.width : dirtyWidth,
        dirtyHeight === undefined ? image.height : dirtyHeight].map(function (value) { return Math.trunc(Number(value)); });
      if (!finite(n)) { throw new TypeError("invalid image rectangle"); }
      room(0);
      var input = source(image);
      commands.push({ op: "putImageData", n: n, brush: { kind: "image", image: input } });
      active = context;
      dirty.add(context);
    };
    context.drawImage = function (image) {
      order();
      ensure();
      var pixels = pixelsOf(image);
      var args = Array.prototype.slice.call(arguments, 1).map(Number);
      var n;
      if (args.length === 2) { n = [0, 0, pixels.width, pixels.height, args[0], args[1], pixels.width, pixels.height]; }
      else if (args.length === 4) { n = [0, 0, pixels.width, pixels.height].concat(args); }
      else if (args.length === 8) { n = args; }
      else { throw new TypeError("invalid drawImage arguments"); }
      if (!finite(n) || !n[2] || !n[3] || !n[6] || !n[7]) { return; }
      if (n[2] < 0) { n[0] += n[2]; n[2] = -n[2]; }
      if (n[3] < 0) { n[1] += n[3]; n[3] = -n[3]; }
      if (n[6] < 0) { n[4] += n[6]; n[6] = -n[6]; }
      if (n[7] < 0) { n[5] += n[7]; n[7] = -n[7]; }
      room(0);
      var input = source(pixels);
      references.push(image);
      commands.push({ op: "drawImage", n: n, state: snapshot(), brush: { kind: "image", image: input } });
      active = context;
      dirty.add(context);
    };
    context.getContextAttributes = function () { return { alpha: true, colorSpace: "srgb" }; };
    ["createConicGradient", "isPointInPath", "isPointInStroke", "drawFocusIfNeeded",
     "scrollPathIntoView"].forEach(function (name) {
      context[name] = function () { return missing(name); };
    });
    Object.defineProperties(context, {
      __pixels: { value: function () { return flush("snapshot"); } },
      __source: { value: function () {
        ensure();
        context.__submit();
        return { nativeCanvas: id, width: width, height: height };
      } },
      __submit: { value: function () {
        ensure();
        if (commands.length) { flush("submit"); }
      } },
      __resize: { value: reset },
      __paint: { value: function () { flush("paint"); } }
    });
    context.reset = reset;
    var setAttribute = canvas.setAttribute;
    canvas.setAttribute = function (name, value) {
      var result = setAttribute.call(canvas, name, value);
      name = String(name).toLowerCase();
      if (name === "width" || name === "height") { reset(); }
      return result;
    };
    reset();
    return context;
  };
  globalThis.__chuzzSubmit2D = function () {
    var context = active;
    if (context) { context.__submit(); }
    active = null;
  };
  globalThis.__chuzzFlush2D = function () {
    var batch = Array.from(dirty);
    dirty.clear();
    batch.forEach(function (context) { context.__paint(); });
    active = null;
  };
})();
