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
  // url() is an SVG filter reference. Canvas has no filter primitives for it,
  // so a url() function is dropped and a filter that is only url() is "none".
  function parseFilter(value, canvas) {
    var source = String(value).trim();
    if (source.length > 8192) { return null; }
    if (source.toLowerCase() === "none") { return { css: "none", ops: [] }; }
    if (!source) { return null; }
    var i = 0;
    var ops = [];
    var sawUrl = false;
    function skip() {
      while (i < source.length && /\s/.test(source.charAt(i))) { i++; }
    }
    function currentColor() {
      if (typeof getComputedStyle === "function") {
        var computed = getComputedStyle(canvas).color;
        if (computed && validColor(computed)) { return computed; }
      }
      return "#000000";
    }
    function readNumber(text, at) {
      var match = /^[+-]?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?/.exec(text.slice(at.i));
      if (!match) { return null; }
      var n = Number(match[0]);
      if (!Number.isFinite(n)) { return null; }
      at.i += match[0].length;
      return n;
    }
    function readLength(text, at, allowNegative) {
      while (at.i < text.length && /\s/.test(text.charAt(at.i))) { at.i++; }
      var start = at.i;
      var n = readNumber(text, at);
      if (n === null) { at.i = start; return null; }
      var unit = "";
      if (at.i < text.length && /[a-z%]/i.test(text.charAt(at.i))) {
        var matched = /^[a-z%]+/i.exec(text.slice(at.i));
        unit = matched[0].toLowerCase();
        at.i += unit.length;
      }
      if (unit === "%") { at.i = start; return null; }
      if (unit === "") {
        if (n !== 0) { at.i = start; return null; }
      } else if (unit !== "px") {
        at.i = start;
        return null;
      }
      if (!allowNegative && n < 0) { at.i = start; return null; }
      if (Math.abs(n) > 1e6) { at.i = start; return null; }
      return n;
    }
    function readAmount(text, at, clampUnit) {
      while (at.i < text.length && /\s/.test(text.charAt(at.i))) { at.i++; }
      var n = readNumber(text, at);
      if (n === null) { return null; }
      if (text.charAt(at.i) === "%") { at.i++; n = n / 100; }
      if (!Number.isFinite(n) || n < 0 || n > 1e6) { return null; }
      if (clampUnit && n > 1) { n = 1; }
      return n;
    }
    function readAngle(text, at) {
      while (at.i < text.length && /\s/.test(text.charAt(at.i))) { at.i++; }
      var n = readNumber(text, at);
      if (n === null) { return null; }
      var unit = "";
      if (at.i < text.length && /[a-z]/i.test(text.charAt(at.i))) {
        var matched = /^[a-z]+/i.exec(text.slice(at.i));
        unit = matched[0].toLowerCase();
        at.i += unit.length;
      }
      var deg = n;
      if (unit === "" || unit === "deg") {
        if (unit === "" && n !== 0) { return null; }
        deg = n;
      } else if (unit === "rad") { deg = n * 180 / Math.PI; }
      else if (unit === "grad") { deg = n * 0.9; }
      else if (unit === "turn") { deg = n * 360; }
      else { return null; }
      if (!Number.isFinite(deg) || Math.abs(deg) > 1e8) { return null; }
      return deg;
    }
    function finish(text, at) {
      while (at.i < text.length && /\s/.test(text.charAt(at.i))) { at.i++; }
      return at.i === text.length;
    }
    function omitted(text) { return !/\S/.test(text); }
    function readColorToken(text, at) {
      while (at.i < text.length && /\s/.test(text.charAt(at.i))) { at.i++; }
      if (at.i >= text.length) { return null; }
      var rest = text.slice(at.i);
      if (text.charAt(at.i) === "#") {
        var hex = /^#[0-9a-fA-F]+/.exec(rest);
        if (!hex) { return null; }
        at.i += hex[0].length;
        return hex[0];
      }
      var ident = /^[a-zA-Z]+/.exec(rest);
      if (!ident) { return null; }
      var name = ident[0];
      var after = at.i + name.length;
      if (text.charAt(after) === "(") {
        var depth = 1;
        var j = after + 1;
        while (j < text.length && depth > 0) {
          if (text.charAt(j) === "(") { depth++; }
          else if (text.charAt(j) === ")") {
            depth--;
            if (depth === 0) { break; }
          }
          j++;
        }
        if (depth !== 0) { return null; }
        var fn = text.slice(at.i, j + 1);
        at.i = j + 1;
        return fn;
      }
      at.i += name.length;
      return name;
    }
    function parseDropShadow(args) {
      var at = { i: 0 };
      var tokens = [];
      while (at.i < args.length) {
        while (at.i < args.length && /\s/.test(args.charAt(at.i))) { at.i++; }
        if (at.i >= args.length) { break; }
        var start = at.i;
        if (/[+-.\d]/.test(args.charAt(at.i))) {
          var length = readLength(args, at, true);
          if (length === null) { return null; }
          tokens.push({ k: "l", v: length });
          continue;
        }
        var color = readColorToken(args, at);
        if (color === null || at.i === start) { return null; }
        tokens.push({ k: "c", v: color });
      }
      var kinds = tokens.map(function (token) { return token.k; }).join("");
      if (["ll", "lll", "llc", "lllc", "cll", "clll"].indexOf(kinds) < 0) { return null; }
      var lengths = tokens.filter(function (token) { return token.k === "l"; }).map(function (token) { return token.v; });
      var colorToken = null;
      tokens.forEach(function (token) { if (token.k === "c") { colorToken = token.v; } });
      var blur = lengths.length === 3 ? lengths[2] : 0;
      if (blur < 0) { return null; }
      var color = colorToken || currentColor();
      if (String(color).toLowerCase() === "currentcolor") { color = currentColor(); }
      if (!validColor(color)) { return null; }
      return { fn: "drop-shadow", x: lengths[0], y: lengths[1], blur: blur, color: color };
    }
    while (i < source.length) {
      skip();
      if (i >= source.length) { break; }
      var nameMatch = /^[a-zA-Z-]+/.exec(source.slice(i));
      if (!nameMatch) { return null; }
      var name = nameMatch[0].toLowerCase();
      i += nameMatch[0].length;
      skip();
      if (source.charAt(i) !== "(") { return null; }
      i++;
      var argStart = i;
      var depth = 1;
      while (i < source.length && depth > 0) {
        if (source.charAt(i) === "(") { depth++; }
        else if (source.charAt(i) === ")") {
          depth--;
          if (depth === 0) { break; }
        }
        i++;
      }
      if (depth !== 0) { return null; }
      var args = source.slice(argStart, i);
      i++;
      if (name === "url") { sawUrl = true; continue; }
      var at = { i: 0 };
      var op = null;
      if (name === "blur") {
        var px = omitted(args) ? 0 : readLength(args, at, false);
        if (px === null || !finish(args, at)) { return null; }
        op = { fn: "blur", px: px };
      } else if (name === "hue-rotate") {
        var deg = omitted(args) ? 0 : readAngle(args, at);
        if (deg === null || !finish(args, at)) { return null; }
        op = { fn: "hue-rotate", deg: deg };
      } else if (name === "drop-shadow") {
        op = parseDropShadow(args);
        if (!op) { return null; }
      } else if (["brightness", "contrast", "saturate"].indexOf(name) >= 0) {
        var scale = omitted(args) ? 1 : readAmount(args, at, false);
        if (scale === null || !finish(args, at)) { return null; }
        op = { fn: name, amount: scale };
      } else if (["grayscale", "invert", "opacity", "sepia"].indexOf(name) >= 0) {
        var amount = omitted(args) ? 1 : readAmount(args, at, true);
        if (amount === null || !finish(args, at)) { return null; }
        op = { fn: name, amount: amount };
      } else {
        return null;
      }
      ops.push(op);
      if (ops.length > 64) { return null; }
    }
    if (!ops.length) { return sawUrl ? { css: "none", ops: [] } : null; }
    var css = ops.map(function (op) {
      if (op.fn === "blur") { return "blur(" + op.px + "px)"; }
      if (op.fn === "hue-rotate") { return "hue-rotate(" + op.deg + "deg)"; }
      if (op.fn === "drop-shadow") {
        return "drop-shadow(" + op.x + "px " + op.y + "px " + op.blur + "px " + op.color + ")";
      }
      return op.fn + "(" + op.amount + ")";
    }).join(" ");
    return { css: css, ops: ops };
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
        imageSmoothingQuality: "low", matrix: identity.slice(),
        shadowBlur: 0, shadowColor: "rgba(0, 0, 0, 0)", shadowOffsetX: 0, shadowOffsetY: 0,
        filter: "none", filterOps: []
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
        direction: direction, smoothing: state.imageSmoothingEnabled,
        shadowBlur: state.shadowBlur, shadowColor: state.shadowColor,
        shadowOffsetX: state.shadowOffsetX, shadowOffsetY: state.shadowOffsetY,
        filter: state.filterOps
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
      } catch (error) {
        // Name the canvas, so a page with several can tell which context hit a limit.
        var element = context.canvas;
        if (error && typeof error.message === "string" && element && element.getAttribute) {
          var name = element.id ? "#" + element.id : (element.getAttribute("class") ? "." + element.getAttribute("class").split(" ")[0] : "<canvas>");
          error.message += " [" + name + "]";
        }
        throw error;
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
      // Every operator in the HTML spec is drawn; other strings are ignored, as in browsers.
      return [
        "source-over", "source-in", "source-out", "source-atop", "destination-over",
        "destination-in", "destination-out", "destination-atop", "lighter", "copy", "xor",
        "multiply", "screen", "overlay", "darken", "lighten", "color-dodge", "color-burn",
        "hard-light", "soft-light", "difference", "exclusion", "hue", "saturation", "color",
        "luminosity",
      ].indexOf(value) < 0 ? undefined : value;
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
    property("shadowBlur", function (value) {
      value = Number(value);
      return Number.isFinite(value) && value >= 0 && value <= 1e6 ? value : undefined;
    });
    property("shadowOffsetX", function (value) {
      value = Number(value);
      return Number.isFinite(value) && Math.abs(value) <= 1e6 ? value : undefined;
    });
    property("shadowOffsetY", function (value) {
      value = Number(value);
      return Number.isFinite(value) && Math.abs(value) <= 1e6 ? value : undefined;
    });
    property("shadowColor", function (value) {
      value = String(value);
      return validColor(value) ? value : undefined;
    });
    property("filter", function (value) {
      var parsed = parseFilter(value, canvas);
      if (!parsed) { return undefined; }
      state.filterOps = parsed.ops;
      return parsed.css;
    });
    ["fontKerning", "fontStretch", "fontVariantCaps", "letterSpacing", "wordSpacing",
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
      saved.filterOps = state.filterOps.slice();
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

  // toBlob and toDataURL encode PNG (the one type every browser supports; other
  // types fall back to it, as the standard allows). The pixels come through a
  // scratch 2D canvas and drawImage, which already reads 2D, WebGPU and video
  // backed canvases alike.
  function canvasPng(canvas) {
    var width = canvas.width, height = canvas.height;
    if (!width || !height) { return null; }
    var scratch = document.createElement("canvas");
    scratch.width = width;
    scratch.height = height;
    var context = scratch.getContext("2d");
    context.drawImage(canvas, 0, 0);
    var pixels = context.getImageData(0, 0, width, height).data;
    var buffer = pixels.buffer.slice(pixels.byteOffset, pixels.byteOffset + pixels.byteLength);
    return globalThis.__chuzzEncodePng(width, height, buffer);
  }
  var canvasPrototype = Object.getPrototypeOf(document.createElement("canvas"));
  while (canvasPrototype && !Object.prototype.hasOwnProperty.call(canvasPrototype, "getContext")) {
    canvasPrototype = Object.getPrototypeOf(canvasPrototype);
  }
  if (canvasPrototype) {
    Object.defineProperty(canvasPrototype, "toBlob", {
      configurable: true,
      writable: true,
      value: function toBlob(callback, type, quality) {
        if (typeof callback !== "function") { throw new TypeError("toBlob requires a callback"); }
        var canvas = this;
        setTimeout(function () {
          var png = null;
          try { png = canvasPng(canvas); } catch (error) { png = null; }
          callback(png ? new Blob([png], { type: "image/png" }) : null);
        }, 0);
      }
    });
    Object.defineProperty(canvasPrototype, "toDataURL", {
      configurable: true,
      writable: true,
      value: function toDataURL(type, quality) {
        var png = canvasPng(this);
        if (!png) { return "data:,"; }
        var text = "";
        for (var index = 0; index < png.length; index += 0x8000) {
          text += String.fromCharCode.apply(null, png.subarray(index, index + 0x8000));
        }
        return "data:image/png;base64," + btoa(text);
      }
    });
  }
})();
