//! Browser compatibility APIs that can be implemented in the page realm.
//!
//! Native DOM, networking and byte conversion remain owned by the page host.

use std::sync::Arc;

use blitz_script_ekojit::{EkojitDocument, PageSource};

const COMPAT: &str = r#"(function (g) {
  'use strict';

  if (typeof g.Error === 'undefined') {
    function Error(message) {
      var error = Object.create(Error.prototype);
      error.message = message === undefined ? '' : String(message);
      return error;
    }
    Error.prototype.name = 'Error';
    Error.prototype.message = '';
    Error.prototype.toString = function () {
      var name = this.name === undefined ? 'Error' : String(this.name);
      var message = this.message === undefined ? '' : String(this.message);
      return name === '' ? message : message === '' ? name : name + ': ' + message;
    };
    g.Error = Error;
    function errorType(name) {
      function NamedError(message) {
        var error = Object.create(NamedError.prototype);
        error.message = message === undefined ? '' : String(message);
        return error;
      }
      NamedError.prototype = Object.create(Error.prototype);
      NamedError.prototype.name = name;
      NamedError.prototype.constructor = NamedError;
      return NamedError;
    }
    g.TypeError = errorType('TypeError');
    g.RangeError = errorType('RangeError');
    g.ReferenceError = errorType('ReferenceError');
    g.SyntaxError = errorType('SyntaxError');
    g.EvalError = errorType('EvalError');
    g.URIError = errorType('URIError');
  }

  function define(name, value) {
    if (typeof g[name] === 'undefined') { g[name] = value; }
  }
  function namedError(name, message) {
    var error = new Error(message || name);
    error.name = name;
    return error;
  }

  if (typeof g.DOMException === 'undefined') {
    function DOMException(message, name) {
      this.name = name || 'Error';
      this.message = message || '';
      this.code = DOMException.codes[this.name] || 0;
    }
    DOMException.prototype = new Error();
    DOMException.prototype.constructor = DOMException;
    DOMException.prototype.toString = function () {
      return this.message ? this.name + ': ' + this.message : this.name;
    };
    DOMException.codes = {
      IndexSizeError: 1, HierarchyRequestError: 3, WrongDocumentError: 4,
      InvalidCharacterError: 5, NoModificationAllowedError: 7,
      NotFoundError: 8, NotSupportedError: 9, InvalidStateError: 11,
      SyntaxError: 12, InvalidModificationError: 13, NamespaceError: 14,
      InvalidAccessError: 15, TypeMismatchError: 17, SecurityError: 18,
      NetworkError: 19, AbortError: 20, URLMismatchError: 21,
      QuotaExceededError: 22, TimeoutError: 23, DataCloneError: 25
    };
    g.DOMException = DOMException;
  }

  if (typeof g.AbortSignal === 'undefined' || typeof g.AbortController === 'undefined') {
    function AbortSignal() {
      this.aborted = false;
      this.reason = undefined;
      this.onabort = null;
      this._listeners = [];
    }
    AbortSignal.prototype.addEventListener = function (type, callback) {
      if (type === 'abort' && typeof callback === 'function') { this._listeners.push(callback); }
    };
    AbortSignal.prototype.removeEventListener = function (type, callback) {
      if (type !== 'abort') { return; }
      var remaining = [];
      for (var i = 0; i < this._listeners.length; i++) {
        if (this._listeners[i] !== callback) { remaining.push(this._listeners[i]); }
      }
      this._listeners = remaining;
    };
    AbortSignal.prototype.throwIfAborted = function () {
      if (this.aborted) { throw this.reason; }
    };
    AbortSignal.abort = function (reason) {
      var signal = new AbortSignal();
      signal.aborted = true;
      signal.reason = arguments.length ? reason : new g.DOMException('The operation was aborted.', 'AbortError');
      return signal;
    };
    function AbortController() { this.signal = new AbortSignal(); }
    AbortController.prototype.abort = function (reason) {
      var signal = this.signal;
      if (signal.aborted) { return; }
      signal.aborted = true;
      signal.reason = arguments.length ? reason : new g.DOMException('The operation was aborted.', 'AbortError');
      var event = { type: 'abort', target: signal, currentTarget: signal };
      var listeners = signal._listeners.slice();
      for (var i = 0; i < listeners.length; i++) { listeners[i].call(signal, event); }
      if (typeof signal.onabort === 'function') { signal.onabort.call(signal, event); }
    };
    define('AbortSignal', AbortSignal);
    g.AbortController = AbortController;
  }

  if (typeof g.NodeFilter === 'undefined') {
    var nodeFilter = {
      FILTER_ACCEPT: 1, FILTER_REJECT: 2, FILTER_SKIP: 3,
      SHOW_ALL: 0xffffffff, SHOW_ELEMENT: 1, SHOW_ATTRIBUTE: 2,
      SHOW_TEXT: 4, SHOW_CDATA_SECTION: 8, SHOW_ENTITY_REFERENCE: 16,
      SHOW_ENTITY: 32, SHOW_PROCESSING_INSTRUCTION: 64, SHOW_COMMENT: 128,
      SHOW_DOCUMENT: 256, SHOW_DOCUMENT_TYPE: 512,
      SHOW_DOCUMENT_FRAGMENT: 1024, SHOW_NOTATION: 2048
    };
    if (typeof Object.freeze === 'function') { Object.freeze(nodeFilter); }
    g.NodeFilter = nodeFilter;
  }

  if (typeof g.matchMedia === 'undefined') {
    function splitMediaClauses(text) {
      var clauses = [], start = 0;
      for (var i = 1; i < text.length - 2; i++) {
        if (text.charAt(i) === 'a' && text.charAt(i + 1) === 'n' && text.charAt(i + 2) === 'd' &&
            text.charAt(i - 1) <= ' ' && text.charAt(i + 3) <= ' ') {
          clauses.push(text.substring(start, i).trim());
          start = i + 3;
          i += 2;
        }
      }
      clauses.push(text.substring(start).trim());
      return clauses;
    }
    function mediaFeature(clause) {
      var colon = clause.indexOf(':');
      if (colon < 0) { return null; }
      var name = clause.substring(0, colon).trim(), value = clause.substring(colon + 1).trim();
      var raw = value;
      var unit = '';
      if (value.length >= 3 && value.substring(value.length - 3) === 'rem') {
        unit = 'rem'; value = value.substring(0, value.length - 3).trim();
      } else if (value.length >= 2 && value.substring(value.length - 2) === 'px') {
        unit = 'px'; value = value.substring(0, value.length - 2).trim();
      } else if (value.length >= 2 && value.substring(value.length - 2) === 'em') {
        unit = 'em'; value = value.substring(0, value.length - 2).trim();
      }
      var number = Number(value);
      return { name: name, value: number, raw: raw, unit: unit, valid: value !== '' && Number.isFinite(number) };
    }
    function mediaMatchesSingle(query) {
      var text = String(query).trim().toLowerCase();
      if (!text) { return false; }
      var width = Number(g.innerWidth) || 0, height = Number(g.innerHeight) || 0;
      var clauses = splitMediaClauses(text), supported = true, matches = true;
      for (var i = 0; i < clauses.length; i++) {
        var clause = clauses[i].trim();
        while (clause.charAt(0) === '(') { clause = clause.substring(1).trim(); }
        while (clause.charAt(clause.length - 1) === ')') { clause = clause.substring(0, clause.length - 1).trim(); }
        if (!clause || clause === 'all' || clause === 'screen') { continue; }
        if (clause === 'not all' || clause === 'print' || clause === 'speech') { matches = false; continue; }
        var feature = mediaFeature(clause), name = feature && feature.name;
        if (feature && feature.valid && (name === 'min-width' || name === 'max-width' || name === 'min-height' || name === 'max-height' || name === 'width' || name === 'height')) {
          var dimension = name.indexOf('width') >= 0 ? width : height;
          var limit = feature.value * (feature.unit === 'em' || feature.unit === 'rem' ? 16 : 1);
          if (name.indexOf('min-') === 0) { matches = matches && dimension >= limit; }
          else if (name.indexOf('max-') === 0) { matches = matches && dimension <= limit; }
          else { matches = matches && dimension === limit; }
          continue;
        }
        if (feature && name === 'orientation' && (feature.raw === 'portrait' || feature.raw === 'landscape')) {
          matches = matches && (feature.raw === 'portrait' ? height >= width : width > height);
          continue;
        }
        if (clause === 'prefers-reduced-motion: no-preference' || clause === 'prefers-color-scheme: light') { continue; }
        if (clause === 'prefers-reduced-motion: reduce' || clause === 'prefers-color-scheme: dark' ||
            clause === 'hover: none' || clause === 'pointer: none') { matches = false; continue; }
        supported = false;
      }
      return supported && matches;
    }
    function mediaMatches(query) {
      var text = String(query), depth = 0, start = 0;
      for (var i = 0; i < text.length; i++) {
        if (text.charAt(i) === '(') { depth++; }
        else if (text.charAt(i) === ')' && depth > 0) { depth--; }
        else if (text.charAt(i) === ',' && depth === 0) {
          if (mediaMatchesSingle(text.substring(start, i))) { return true; }
          start = i + 1;
        }
      }
      return mediaMatchesSingle(text.substring(start));
    }
    var mediaLists = [];
    function updateMediaLists() {
      for (var i = 0; i < mediaLists.length; i++) {
        var list = mediaLists[i], matches = mediaMatches(list.media);
        if (matches === list.matches) { continue; }
        list.matches = matches;
        var event = { type: 'change', matches: matches, media: list.media, target: list, currentTarget: list };
        var listeners = list._listeners.slice();
        for (var j = 0; j < listeners.length; j++) {
          var listener = listeners[j];
          if (typeof listener === 'function') { listener.call(list, event); }
          else if (listener && typeof listener.handleEvent === 'function') { listener.handleEvent(event); }
        }
        if (typeof list.onchange === 'function') { list.onchange.call(list, event); }
      }
    }
    if (g.addEventListener) { g.addEventListener('resize', updateMediaLists); }
    g.matchMedia = function (query) {
      var media = String(query);
      var list = {
        matches: mediaMatches(media), media: media, onchange: null, _listeners: [],
        addListener: function (listener) {
          if (typeof listener === 'function' && this._listeners.indexOf(listener) < 0) { this._listeners.push(listener); }
        },
        removeListener: function (listener) {
          var index = this._listeners.indexOf(listener);
          if (index >= 0) { this._listeners.splice(index, 1); }
        },
        addEventListener: function (type, listener) {
          if (type === 'change') { this.addListener(listener); }
        },
        removeEventListener: function (type, listener) {
          if (type === 'change') { this.removeListener(listener); }
        },
        dispatchEvent: function (event) {
          if (!event || event.type !== 'change') { return true; }
          var listeners = this._listeners.slice();
          for (var i = 0; i < listeners.length; i++) {
            var listener = listeners[i];
            if (typeof listener === 'function') { listener.call(this, event); }
            else if (listener && typeof listener.handleEvent === 'function') { listener.handleEvent(event); }
          }
          if (typeof this.onchange === 'function') { this.onchange.call(this, event); }
          return true;
        }
      };
      mediaLists.push(list);
      return list;
    };
  }

  if (typeof g.CSS === 'undefined') { g.CSS = {}; }
  if (typeof g.CSS.supports !== 'function') {
    // Ask the document's style engine whether it accepts the declaration.
    // Compound conditions (and/or/not/selector()) are not parsed and report false.
    var cssDeclarationSupported = function (property, value) {
      property = String(property).trim();
      value = String(value).trim();
      if (property === '' || value === '') { return false; }
      if (property.indexOf('--') === 0) { return true; }
      var doc = g.document;
      var probe = doc && typeof doc.createElement === 'function' ? doc.createElement('div') : null;
      var style = probe && probe.style;
      if (!style || typeof style.setProperty !== 'function' || typeof style.getPropertyValue !== 'function') { return false; }
      try { style.setProperty(property, value); } catch (_) { return false; }
      return String(style.getPropertyValue(property)) !== '';
    };
    g.CSS.supports = function (property, value) {
      if (arguments.length === 1) {
        var condition = String(property).trim();
        if (/^(not\s|selector\()/i.test(condition) || /\)\s*(and|or)\s*\(/i.test(condition)) { return false; }
        if (condition.charAt(0) === '(' && condition.charAt(condition.length - 1) === ')') {
          condition = condition.substring(1, condition.length - 1).trim();
        }
        var colon = condition.indexOf(':');
        return colon > 0 && cssDeclarationSupported(condition.substring(0, colon), condition.substring(colon + 1));
      }
      return cssDeclarationSupported(property, value);
    };
  }

  // Initial observation records are delivered after the observing script
  // returns, as in browsers, never synchronously inside observe().
  function observerDelivery(callback) {
    if (typeof g.queueMicrotask === 'function') { g.queueMicrotask(callback); }
    else if (typeof g.Promise === 'function') { g.Promise.resolve().then(callback); }
    else if (typeof g.setTimeout === 'function') { g.setTimeout(callback, 0); }
  }

  if (typeof g.ResizeObserver === 'undefined') {
    function ResizeObserver(callback) {
      if (typeof callback !== 'function') { throw new g.TypeError('ResizeObserver callback must be callable'); }
      this._callback = callback;
      this._targets = [];
    }
    ResizeObserver.prototype.observe = function (target) {
      if (!target || this._targets.indexOf(target) >= 0) { return; }
      this._targets.push(target);
      var rect = typeof target.getBoundingClientRect === 'function'
        ? target.getBoundingClientRect() : { x: 0, y: 0, width: 0, height: 0, top: 0, right: 0, bottom: 0, left: 0 };
      var entry = { target: target, contentRect: rect, contentBoxSize: [{ inlineSize: rect.width || 0, blockSize: rect.height || 0 }], borderBoxSize: [{ inlineSize: rect.width || 0, blockSize: rect.height || 0 }], devicePixelContentBoxSize: [{ inlineSize: rect.width || 0, blockSize: rect.height || 0 }] };
      var observer = this;
      observerDelivery(function () {
        if (observer._targets.indexOf(target) >= 0) { observer._callback([entry], observer); }
      });
    };
    ResizeObserver.prototype.unobserve = function (target) {
      var index = this._targets.indexOf(target);
      if (index >= 0) { this._targets.splice(index, 1); }
    };
    ResizeObserver.prototype.disconnect = function () { this._targets = []; };
    g.ResizeObserver = ResizeObserver;
  }

  if (typeof g.IntersectionObserver === 'undefined') {
    function IntersectionObserver(callback, options) {
      if (typeof callback !== 'function') { throw new g.TypeError('IntersectionObserver callback must be callable'); }
      this._callback = callback;
      this._targets = [];
      this.root = options && options.root || null;
      this.rootMargin = options && options.rootMargin || '0px';
      this.thresholds = options && options.threshold !== undefined
        ? (Array.isArray(options.threshold) ? options.threshold.slice() : [options.threshold]) : [0];
    }
    IntersectionObserver.prototype.observe = function (target) {
      if (!target || this._targets.indexOf(target) >= 0) { return; }
      this._targets.push(target);
      var rect = typeof target.getBoundingClientRect === 'function'
        ? target.getBoundingClientRect() : { x: 0, y: 0, width: 0, height: 0, top: 0, right: 0, bottom: 0, left: 0 };
      var width = Number(g.innerWidth) || 0, height = Number(g.innerHeight) || 0;
      var left = Math.max(0, rect.left || 0), top = Math.max(0, rect.top || 0);
      var right = Math.min(width, rect.right === undefined ? left + (rect.width || 0) : rect.right);
      var bottom = Math.min(height, rect.bottom === undefined ? top + (rect.height || 0) : rect.bottom);
      var intersectionWidth = Math.max(0, right - left), intersectionHeight = Math.max(0, bottom - top);
      var area = Math.max(0, rect.width || 0) * Math.max(0, rect.height || 0);
      var intersectionArea = intersectionWidth * intersectionHeight;
      var entry = {
        time: g.performance && typeof g.performance.now === 'function' ? g.performance.now() : 0,
        target: target, rootBounds: null, boundingClientRect: rect,
        intersectionRect: { x: left, y: top, left: left, top: top, right: right, bottom: bottom, width: intersectionWidth, height: intersectionHeight },
        isIntersecting: intersectionWidth > 0 && intersectionHeight > 0,
        intersectionRatio: area > 0 ? intersectionArea / area : 0
      };
      var observer = this;
      observerDelivery(function () {
        if (observer._targets.indexOf(target) >= 0) { observer._callback([entry], observer); }
      });
    };
    IntersectionObserver.prototype.unobserve = function (target) {
      var index = this._targets.indexOf(target);
      if (index >= 0) { this._targets.splice(index, 1); }
    };
    IntersectionObserver.prototype.disconnect = function () { this._targets = []; };
    IntersectionObserver.prototype.takeRecords = function () { return []; };
    g.IntersectionObserver = IntersectionObserver;
  }

  function viewportScrollPosition(axis) {
    var document = g.document;
    if (!document) { return 0; }
    var root = document.scrollingElement ||
      (document.compatMode === 'BackCompat' ? document.body : document.documentElement) ||
      document.documentElement || document.body;
    var position = Number(root && root[axis]);
    return Number.isFinite(position) ? position : 0;
  }
  var missingScrollX = typeof g.scrollX === 'undefined';
  var missingScrollY = typeof g.scrollY === 'undefined';
  var missingPageXOffset = typeof g.pageXOffset === 'undefined';
  var missingPageYOffset = typeof g.pageYOffset === 'undefined';
  if (missingScrollX || missingScrollY || missingPageXOffset || missingPageYOffset) {
    function updateViewportScrollOffsets() {
      var x = viewportScrollPosition('scrollLeft'), y = viewportScrollPosition('scrollTop');
      if (missingScrollX) { g.scrollX = x; }
      if (missingScrollY) { g.scrollY = y; }
      if (missingPageXOffset) { g.pageXOffset = x; }
      if (missingPageYOffset) { g.pageYOffset = y; }
    }
    updateViewportScrollOffsets();
    var scrollDocument = g.document;
    var scrollTargets = [g, scrollDocument, scrollDocument && scrollDocument.documentElement, scrollDocument && scrollDocument.body];
    for (var scrollTargetIndex = 0; scrollTargetIndex < scrollTargets.length; scrollTargetIndex++) {
      var scrollTarget = scrollTargets[scrollTargetIndex];
      if (scrollTarget && typeof scrollTarget.addEventListener === 'function') {
        scrollTarget.addEventListener('scroll', updateViewportScrollOffsets);
      }
    }
  }

  if (typeof g.Path2D === 'undefined') {
    function finiteNumber(value) {
      var number = Number(value);
      if (!Number.isFinite(number)) { throw new g.TypeError('Path coordinates must be finite numbers'); }
      return number;
    }
    function requireArgs(args, count, name) {
      if (args.length < count) { throw new g.TypeError("Failed to execute '" + name + "': not enough arguments"); }
    }
    function copyCommands(commands) {
      var copy = [];
      for (var i = 0; i < commands.length; i++) {
        copy.push({ op: commands[i].op, n: commands[i].n.slice(), matrix: commands[i].matrix.slice() });
      }
      return copy;
    }
    function command(op, values, matrix) { return { op: op, n: values, matrix: matrix || [1, 0, 0, 1, 0, 0] }; }
    function composeMatrix(outer, inner) {
      return [outer[0] * inner[0] + outer[2] * inner[1], outer[1] * inner[0] + outer[3] * inner[1],
        outer[0] * inner[2] + outer[2] * inner[3], outer[1] * inner[2] + outer[3] * inner[3],
        outer[0] * inner[4] + outer[2] * inner[5] + outer[4], outer[1] * inner[4] + outer[3] * inner[5] + outer[5]];
    }
    function matrixValues(matrix) {
      if (matrix === undefined || matrix === null) { return [1, 0, 0, 1, 0, 0]; }
      var a = matrix.a !== undefined ? matrix.a : matrix.m11;
      var b = matrix.b !== undefined ? matrix.b : matrix.m12;
      var c = matrix.c !== undefined ? matrix.c : matrix.m21;
      var d = matrix.d !== undefined ? matrix.d : matrix.m22;
      var e = matrix.e !== undefined ? matrix.e : matrix.m41;
      var f = matrix.f !== undefined ? matrix.f : matrix.m42;
      var values = [a === undefined ? 1 : a, b === undefined ? 0 : b, c === undefined ? 0 : c,
        d === undefined ? 1 : d, e === undefined ? 0 : e, f === undefined ? 0 : f];
      for (var i = 0; i < values.length; i++) { values[i] = finiteNumber(values[i]); }
      return values;
    }
    function Path2D(path) {
      if (!(this instanceof Path2D)) { throw new g.TypeError("Failed to construct 'Path2D': use new"); }
      this._commands = [];
      if (path === undefined) { return; }
      if (path instanceof Path2D) { this._commands = copyCommands(path._commands); return; }
      if (typeof path === 'string') {
        if (typeof g.__chuzzCanvas2D !== 'function') { throw new g.DOMException('SVG path parsing is unavailable.', 'NotSupportedError'); }
        var parsed = g.__chuzzCanvas2D({ action: 'path', value: path });
        if (!Array.isArray(parsed)) { throw new g.SyntaxError('The SVG path data is invalid.'); }
        this._commands = copyCommands(parsed);
        return;
      }
      throw new g.TypeError('Path2D argument must be a Path2D or SVG path string');
    }
    Path2D.prototype.addPath = function (path, transform) {
      requireArgs(arguments, 1, 'addPath');
      if (!(path instanceof Path2D)) { throw new g.TypeError("Failed to execute 'addPath': argument is not a Path2D"); }
      var matrix = matrixValues(transform);
      var commands = copyCommands(path._commands);
      for (var i = 0; i < commands.length; i++) { commands[i].matrix = composeMatrix(matrix, commands[i].matrix); }
      for (var j = 0; j < commands.length; j++) { this._commands.push(commands[j]); }
    };
    Path2D.prototype.moveTo = function (x, y) { requireArgs(arguments, 2, 'moveTo'); this._commands.push(command('moveTo', [finiteNumber(x), finiteNumber(y)])); };
    Path2D.prototype.lineTo = function (x, y) { requireArgs(arguments, 2, 'lineTo'); this._commands.push(command('lineTo', [finiteNumber(x), finiteNumber(y)])); };
    Path2D.prototype.bezierCurveTo = function (a, b, c, d, e, f) {
      requireArgs(arguments, 6, 'bezierCurveTo');
      this._commands.push(command('bezierCurveTo', [finiteNumber(a), finiteNumber(b), finiteNumber(c), finiteNumber(d), finiteNumber(e), finiteNumber(f)]));
    };
    Path2D.prototype.quadraticCurveTo = function (a, b, c, d) {
      requireArgs(arguments, 4, 'quadraticCurveTo');
      this._commands.push(command('quadraticCurveTo', [finiteNumber(a), finiteNumber(b), finiteNumber(c), finiteNumber(d)]));
    };
    Path2D.prototype.arc = function (x, y, radius, start, end, counterclockwise) {
      requireArgs(arguments, 5, 'arc');
      x = finiteNumber(x); y = finiteNumber(y); radius = finiteNumber(radius); start = finiteNumber(start); end = finiteNumber(end);
      if (radius < 0) { throw new g.DOMException('The radius provided is negative.', 'IndexSizeError'); }
      this._commands.push(command('arc', [x, y, radius, start, end, !!counterclockwise ? 1 : 0]));
    };
    Path2D.prototype.arcTo = function (x1, y1, x2, y2, radius) {
      requireArgs(arguments, 5, 'arcTo');
      x1 = finiteNumber(x1); y1 = finiteNumber(y1); x2 = finiteNumber(x2); y2 = finiteNumber(y2); radius = finiteNumber(radius);
      if (radius < 0) { throw new g.DOMException('The radius provided is negative.', 'IndexSizeError'); }
      this._commands.push(command('arcTo', [x1, y1, x2, y2, radius]));
    };
    Path2D.prototype.ellipse = function (x, y, rx, ry, rotation, start, end, counterclockwise) {
      requireArgs(arguments, 7, 'ellipse');
      x = finiteNumber(x); y = finiteNumber(y); rx = finiteNumber(rx); ry = finiteNumber(ry);
      rotation = finiteNumber(rotation); start = finiteNumber(start); end = finiteNumber(end);
      if (rx < 0 || ry < 0) { throw new g.DOMException('The radii provided are negative.', 'IndexSizeError'); }
      this._commands.push(command('ellipse', [x, y, rx, ry, rotation, start, end, !!counterclockwise ? 1 : 0]));
    };
    Path2D.prototype.rect = function (x, y, width, height) {
      requireArgs(arguments, 4, 'rect');
      this._commands.push(command('rect', [finiteNumber(x), finiteNumber(y), finiteNumber(width), finiteNumber(height)]));
    };
    Path2D.prototype.roundRect = function (x, y, width, height, radii) {
      requireArgs(arguments, 4, 'roundRect');
      x = finiteNumber(x); y = finiteNumber(y); width = finiteNumber(width); height = finiteNumber(height);
      var list = arguments.length < 5 || radii === undefined ? [0] : Array.isArray(radii) ? radii.slice() : [radii];
      if (list.length < 1 || list.length > 4) { throw new g.RangeError('roundRect radii must contain between one and four values'); }
      for (var i = 0; i < list.length; i++) {
        var radius = list[i], pair = Array.isArray(radius) ? radius.slice() :
          radius && typeof radius === 'object' ? [radius.x === undefined ? 0 : radius.x, radius.y === undefined ? 0 : radius.y] : [radius, radius];
        if (pair.length !== 2) { throw new g.TypeError('Each roundRect radius must be a number or pair'); }
        pair[0] = finiteNumber(pair[0]); pair[1] = finiteNumber(pair[1]);
        if (pair[0] < 0 || pair[1] < 0) { throw new g.DOMException('The radii provided are negative.', 'IndexSizeError'); }
        list[i] = pair;
      }
      var expanded = list.length === 1 ? [list[0], list[0], list[0], list[0]] :
        list.length === 2 ? [list[0], list[1], list[0], list[1]] :
        list.length === 3 ? [list[0], list[1], list[2], list[1]] : list;
      if (width < 0) { x += width; width = -width; expanded = [expanded[1], expanded[0], expanded[3], expanded[2]]; }
      if (height < 0) { y += height; height = -height; expanded = [expanded[3], expanded[2], expanded[1], expanded[0]]; }
      var values = [x, y, width, height];
      for (var j = 0; j < expanded.length; j++) { values.push(expanded[j][0], expanded[j][1]); }
      this._commands.push(command('roundRect', values));
    };
    Path2D.prototype.closePath = function () { this._commands.push(command('closePath', [])); };
    define('Path2D', Path2D);
  }

  if (typeof g.escape === 'undefined') {
    g.escape = function (value) {
      var input = String(value), output = '', hex = '0123456789ABCDEF';
      for (var i = 0; i < input.length; i++) {
        var code = input.charCodeAt(i);
        if ((code >= 65 && code <= 90) || (code >= 97 && code <= 122) ||
            (code >= 48 && code <= 57) || code === 64 || code === 42 ||
            code === 95 || code === 43 || code === 45 || code === 46 || code === 47) {
          output += input.charAt(i);
        } else if (code < 256) {
          output += '%' + hex.charAt((code >> 4) & 15) + hex.charAt(code & 15);
        } else {
          output += '%u' + hex.charAt((code >> 12) & 15) + hex.charAt((code >> 8) & 15) +
            hex.charAt((code >> 4) & 15) + hex.charAt(code & 15);
        }
      }
      return output;
    };
  }
  if (typeof g.unescape === 'undefined') {
    g.unescape = function (value) {
      var input = String(value), output = '', i = 0;
      while (i < input.length) {
        if (input.charAt(i) === '%' && input.charAt(i + 1) === 'u' && i + 5 < input.length) {
          var unicode = parseInt(input.substring(i + 2, i + 6), 16);
          if (!isNaN(unicode)) { output += String.fromCharCode(unicode); i += 6; continue; }
        }
        if (input.charAt(i) === '%' && i + 2 < input.length) {
          var byte = parseInt(input.substring(i + 1, i + 3), 16);
          if (!isNaN(byte)) { output += String.fromCharCode(byte); i += 3; continue; }
        }
        output += input.charAt(i++);
      }
      return output;
    };
  }

  if (typeof String.prototype.charAt === "undefined") {
    String.prototype.charAt = function (position) {
      var text = String(this), index = Number(position) || 0;
      index = index < 0 ? Math.ceil(index) : Math.floor(index);
      return index < 0 || index >= text.length ? "" : text.slice(index, index + 1);
    };
  }
  if (typeof String.prototype.substr === 'undefined') {
    String.prototype.substr = function (start, length) {
      var text = String(this), size = text.length;
      var first = Number(start) || 0;
      first = first < 0 ? Math.max(size + Math.ceil(first), 0) : Math.min(Math.floor(first), size);
      var count = arguments.length < 2 ? size : Math.max(Number(length) || 0, 0);
      return text.substring(first, Math.min(first + Math.floor(count), size));
    };
  }

  if (typeof g.atob === 'undefined' || typeof g.btoa === 'undefined') {
    var alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
    function btoa(value) {
      var input = String(value), output = '';
      for (var i = 0; i < input.length; i += 3) {
        var a = input.charCodeAt(i), hasB = i + 1 < input.length, hasC = i + 2 < input.length;
        var b = hasB ? input.charCodeAt(i + 1) : 0;
        var c = hasC ? input.charCodeAt(i + 2) : 0;
        if (a > 255 || b > 255 || c > 255) { throw new g.DOMException('The string contains characters outside of the Latin1 range.', 'InvalidCharacterError'); }
        output += alphabet.charAt(a >> 2);
        output += alphabet.charAt(((a & 3) << 4) | (b >> 4));
        output += hasB ? alphabet.charAt(((b & 15) << 2) | (c >> 6)) : '=';
        output += hasC ? alphabet.charAt(c & 63) : '=';
      }
      return output;
    }
    function atob(value) {
      var source = String(value), input = '';
      for (var sourceIndex = 0; sourceIndex < source.length; sourceIndex++) {
        var whitespace = source.charAt(sourceIndex);
        if (whitespace !== ' ' && whitespace !== '\t' && whitespace !== '\n' &&
            whitespace !== '\r' && whitespace !== '\f') { input += whitespace; }
      }
      if (input.length % 4 === 1) { throw new g.DOMException('The string to be decoded is not correctly encoded.', 'InvalidCharacterError'); }
      var output = '', buffer = 0, bits = 0, padding = false;
      for (var i = 0; i < input.length; i++) {
        var character = input.charAt(i);
        if (character === '=') {
          padding = true;
          if (i < input.length - 2 || (input.length - i) > 2) {
            throw new g.DOMException('The string to be decoded is not correctly encoded.', 'InvalidCharacterError');
          }
          continue;
        }
        if (padding) { throw new g.DOMException('The string to be decoded is not correctly encoded.', 'InvalidCharacterError'); }
        var digit = alphabet.indexOf(character);
        if (digit < 0) { throw new g.DOMException('The string to be decoded is not correctly encoded.', 'InvalidCharacterError'); }
        buffer = (buffer << 6) | digit;
        bits += 6;
        if (bits >= 8) { bits -= 8; output += String.fromCharCode((buffer >> bits) & 255); }
      }
      if (bits === 6 || (bits && (buffer & ((1 << bits) - 1)) !== 0)) {
        throw new g.DOMException('The string to be decoded is not correctly encoded.', 'InvalidCharacterError');
      }
      return output;
    }
    define('btoa', btoa);
    define('atob', atob);
  }

  if (typeof g.TextEncoder === 'undefined' && typeof __chuzzEncodeUtf8 === 'function') {
    function TextEncoder() { this.encoding = 'utf-8'; }
    TextEncoder.prototype.encode = function (text) { return __chuzzEncodeUtf8(text === undefined ? '' : String(text)); };
    g.TextEncoder = TextEncoder;
  }
  if (typeof g.TextDecoder === 'undefined' && typeof __chuzzDecodeUtf8 === 'function') {
    function TextDecoder(label) {
      var normalized = label === undefined ? 'utf-8' : String(label).toLowerCase();
      if (normalized !== 'utf-8' && normalized !== 'utf8') { throw new RangeError('The encoding label is unsupported.'); }
      this.encoding = 'utf-8';
    }
    TextDecoder.prototype.decode = function (input) {
      if (input === undefined) { return ''; }
      return __chuzzDecodeUtf8(__chuzzCopyBytes(input));
    };
    g.TextDecoder = TextDecoder;
  }

  if (typeof g.Blob === 'undefined' && typeof __chuzzEncodeUtf8 === 'function') {
    function Blob(parts, options) {
      var pieces = [], total = 0, list = parts || [];
      for (var i = 0; i < list.length; i++) {
        var part = list[i], bytes;
        if (typeof part === 'string') { bytes = __chuzzEncodeUtf8(part); }
        else { bytes = __chuzzCopyBytes(part); }
        pieces.push(bytes); total += bytes.length;
      }
      var joined = new Uint8Array(total), offset = 0;
      for (var p = 0; p < pieces.length; p++) { joined.set(pieces[p], offset); offset += pieces[p].length; }
      this._bytes = joined;
      this.size = total;
      this.type = options && options.type ? String(options.type).toLowerCase() : '';
    }
    Blob.prototype.arrayBuffer = function () { return Promise.resolve(this._bytes.buffer); };
    Blob.prototype.text = function () { return Promise.resolve(__chuzzDecodeUtf8(this._bytes)); };
    g.Blob = Blob;
  }

  if (g.URL && typeof g.URL.createObjectURL === 'undefined' && typeof g.btoa === 'function') {
    g.URL.createObjectURL = function (blob) {
      if (!blob || !blob._bytes) { throw new TypeError('createObjectURL requires a Blob'); }
      var binary = '';
      for (var i = 0; i < blob._bytes.length; i++) { binary += String.fromCharCode(blob._bytes[i]); }
      return 'data:' + (blob.type || 'application/octet-stream') + ';base64,' + g.btoa(binary);
    };
    g.URL.revokeObjectURL = function () {};
  }

  if (typeof g.MessageChannel === 'undefined') {
    function MessagePort() { this.onmessage = null; this._listeners = []; this._closed = false; this._peer = null; }
    MessagePort.prototype.addEventListener = function (type, callback) {
      if (type === 'message' && typeof callback === 'function') { this._listeners.push(callback); }
    };
    MessagePort.prototype.removeEventListener = function (type, callback) {
      if (type !== 'message') { return; }
      var kept = [];
      for (var i = 0; i < this._listeners.length; i++) { if (this._listeners[i] !== callback) { kept.push(this._listeners[i]); } }
      this._listeners = kept;
    };
    MessagePort.prototype.close = function () { this._closed = true; };
    MessagePort.prototype.start = function () {};
    MessagePort.prototype.postMessage = function (data) {
      var target = this._peer;
      if (!target || target._closed) { return; }
      setTimeout(function () {
        if (target._closed) { return; }
        var event = { type: 'message', data: data, target: target, currentTarget: target };
        if (typeof target.onmessage === 'function') { target.onmessage.call(target, event); }
        var listeners = target._listeners.slice();
        for (var i = 0; i < listeners.length; i++) { listeners[i].call(target, event); }
      }, 0);
    };
    function MessageChannel() {
      this.port1 = new MessagePort(); this.port2 = new MessagePort();
      this.port1._peer = this.port2; this.port2._peer = this.port1;
    }
    g.MessageChannel = MessageChannel;
  }

  // Image is deliberately left to the native DOM. A timer-only replacement
  // would report successful loads for missing or undecodable resources.

  (function () {
    if (!g.Intl && typeof __chuzzFormatDecimal === 'function') {
      function NumberFormat(locales, options) {
        if (!(this instanceof NumberFormat)) return new NumberFormat(locales, options);
        var locale = Array.isArray(locales) ? locales[0] : locales;
        locale = locale === undefined ? 'en-US' : String(locale);
        options = options || {};
        if (options.style && options.style !== 'decimal') throw new RangeError('number style is unavailable');
        var allowed = ['style'];
        var names = Object.keys(options);
        for (var i = 0; i < names.length; i++) if (allowed.indexOf(names[i]) < 0) throw new RangeError('number format option is unavailable');
        __chuzzFormatDecimal(locale, '0');
        this.format = function (value) {
          var number = Number(value);
          if (!Number.isFinite(number)) throw new RangeError('non-finite number formatting is unavailable');
          var decimal = number.toFixed(3);
          if (decimal.indexOf('.') >= 0) {
            while (decimal.charAt(decimal.length - 1) === '0') decimal = decimal.substring(0, decimal.length - 1);
            if (decimal.charAt(decimal.length - 1) === '.') decimal = decimal.substring(0, decimal.length - 1);
          }
          if (number === 0 && 1 / number < 0) decimal = '-0';
          return __chuzzFormatDecimal(locale, decimal);
        };
        this.resolvedOptions = function () { return { locale: locale, style: 'decimal', useGrouping: true }; };
      }
      g.Intl = { NumberFormat: NumberFormat };
    }
    var registry = g.customElements || {}, definitions = {}, waiters = {};
    var nativeDefine = registry.define, nativeGet = registry.get;
    try {
      registry.define = function (name, constructor) {
        name = String(name).toLowerCase();
        if (definitions[name] || (typeof nativeGet === 'function' && nativeGet.call(registry, name))) {
          throw new g.DOMException('The name has already been used.', 'NotSupportedError');
        }
        if (typeof nativeDefine === 'function') { nativeDefine.call(registry, name, constructor); }
        definitions[name] = constructor;
        var pending = waiters[name] || [];
        waiters[name] = [];
        for (var i = 0; i < pending.length; i++) { pending[i](constructor); }
      };
      registry.get = function (name) {
        name = String(name).toLowerCase();
        return definitions[name] || (typeof nativeGet === 'function' ? nativeGet.call(registry, name) : undefined);
      };
      registry.whenDefined = function (name) {
        name = String(name).toLowerCase();
        var defined = registry.get(name);
        if (defined) { return Promise.resolve(defined); }
        return new Promise(function (resolve) {
          if (!waiters[name]) { waiters[name] = []; }
          waiters[name].push(resolve);
        });
      };
      if (typeof registry.upgrade !== 'function') { registry.upgrade = function () {}; }
      g.customElements = registry;
    } catch (error) { /* A native registry may be read-only; leave its APIs intact. */ }
  })();

  if (typeof g.HTMLMediaElement === 'undefined') {
    g.HTMLMediaElement = {
      HAVE_NOTHING: 0, HAVE_METADATA: 1, HAVE_CURRENT_DATA: 2,
      HAVE_FUTURE_DATA: 3, HAVE_ENOUGH_DATA: 4
    };
  }

})(globalThis);"#;

const SCHEDULING: &str = r#"(function (g) {
  'use strict';
  if (typeof g.setInterval === 'undefined') {
    var intervals = Object.create(null), intervalId = 1;
    g.setInterval = function (callback, delay) {
      if (typeof callback !== 'function') { throw new DOMException('Interval callback must be callable', 'TypeError'); }
      var id = intervalId++, args = Array.from(arguments).slice(2);
      var state = { active: true, timer: 0 }; intervals[id] = state;
      var period = Math.max(4, Number(delay) || 0);
      function tick() {
        if (!state.active) { return; }
        state.timer = setTimeout(tick, period);
        callback.apply(g, args);
      }
      state.timer = setTimeout(tick, period);
      return id;
    };
    g.clearInterval = function (id) {
      var state = intervals[id];
      if (state) { state.active = false; clearTimeout(state.timer); intervals[id] = null; }
    };
  }
  if (typeof g.performance === 'undefined') {
    var entries = [];
    function add(entry) {
      if (entries.length >= 256) { throw new DOMException('Performance entry limit', 'QuotaExceededError'); }
      entries.push(entry); return entry;
    }
    function find(name) {
      for (var i = entries.length - 1; i >= 0; i--) { if (entries[i].name === name && entries[i].entryType === 'mark') { return entries[i].startTime; } }
      throw new DOMException('Unknown performance mark', 'SyntaxError');
    }
    g.performance = {
      timeOrigin: __chuzzTimeOrigin(), now: __chuzzMonotonicNow,
      mark: function (name, options) { return add({ name: String(name), entryType: 'mark', startTime: options && options.startTime !== undefined ? Number(options.startTime) : __chuzzMonotonicNow(), duration: 0, detail: options && options.detail !== undefined ? options.detail : null }); },
      measure: function (name, start, end) { var from = start === undefined ? 0 : find(String(start)); var to = end === undefined ? __chuzzMonotonicNow() : find(String(end)); return add({ name: String(name), entryType: 'measure', startTime: from, duration: to - from, detail: null }); },
      getEntries: function () { return entries.slice(); },
      getEntriesByType: function (type) { return entries.filter(function (entry) { return entry.entryType === type; }); },
      getEntriesByName: function (name, type) { return entries.filter(function (entry) { return entry.name === name && (type === undefined || entry.entryType === type); }); },
      clearMarks: function (name) { entries = entries.filter(function (entry) { return entry.entryType !== 'mark' || (name !== undefined && entry.name !== String(name)); }); },
      clearMeasures: function (name) { entries = entries.filter(function (entry) { return entry.entryType !== 'measure' || (name !== undefined && entry.name !== String(name)); }); }
    };
  }
  if (typeof g.PerformanceObserver === 'undefined') {
    function PerformanceObserver(callback) { if (typeof callback !== 'function') { throw new DOMException('Observer callback must be callable', 'TypeError'); } }
    PerformanceObserver.supportedEntryTypes = [];
    PerformanceObserver.prototype.observe = function (options) {
      if (!options || options.type || !Array.isArray(options.entryTypes) || options.entryTypes.length) { throw new DOMException('Performance observer entry type is unsupported', 'NotSupportedError'); }
    };
    PerformanceObserver.prototype.disconnect = function () {};
    PerformanceObserver.prototype.takeRecords = function () { return []; };
    g.PerformanceObserver = PerformanceObserver;
  }
  if (typeof g.requestIdleCallback === 'undefined') {
    g.requestIdleCallback = function (callback, options) {
      var requested = performance.now(), timeout = options && options.timeout;
      return setTimeout(function () {
        var started = performance.now();
        callback({ didTimeout: timeout !== undefined && started - requested >= Number(timeout), timeRemaining: function () { return Math.max(0, 50 - (performance.now() - started)); } });
      }, 1);
    };
    g.cancelIdleCallback = clearTimeout;
  }
  if (g.navigator && typeof g.navigator.sendBeacon === 'undefined') {
    g.navigator.sendBeacon = function (url, data) {
      try {
        var resolved = new URL(String(url), location.href).href;
        var body = data == null ? null : typeof data === 'string' ? __chuzzEncodeUtf8(data) : __chuzzCopyBytes(data._bytes || data);
        __chuzzNetAsync({ url: resolved, method: 'POST', headers: [], body: body, credentials: 'include' }, function () {});
        return true;
      } catch (error) { return false; }
    };
  }
})(globalThis);"#;

const CRYPTO: &str = r#"(function (g) {
  'use strict';
  if (typeof g.crypto === 'undefined') { g.crypto = {}; }
  if (typeof g.crypto.getRandomValues === 'undefined' && typeof __chuzzRandomBytes === 'function') {
    g.crypto.getRandomValues = function (view) {
      if (!ArrayBuffer.isView(view) || view instanceof DataView ||
          (typeof Float32Array === 'function' && view instanceof Float32Array) ||
          (typeof Float64Array === 'function' && view instanceof Float64Array)) {
        throw new DOMException('The argument must be an integer typed array', 'TypeMismatchError');
      }
      if (view.byteLength > 65536) {
        throw new DOMException('The requested byte length exceeds 65,536', 'QuotaExceededError');
      }
      new Uint8Array(view.buffer, view.byteOffset, view.byteLength).set(__chuzzCopyBytes(__chuzzRandomBytes(view.byteLength)));
      return view;
    };
  }
  if (typeof g.crypto.randomUUID === 'undefined' && typeof __chuzzRandomBytes === 'function') {
    g.crypto.randomUUID = function () {
      var bytes = __chuzzCopyBytes(__chuzzRandomBytes(16));
      bytes[6] = (bytes[6] & 15) | 64;
      bytes[8] = (bytes[8] & 63) | 128;
      var hex = [], digits = '0123456789abcdef';
      for (var i = 0; i < bytes.length; i++) {
        hex.push(digits.charAt(bytes[i] >> 4) + digits.charAt(bytes[i] & 15));
      }
      return hex.slice(0, 4).join('') + '-' + hex.slice(4, 6).join('') + '-' +
        hex.slice(6, 8).join('') + '-' + hex.slice(8, 10).join('') + '-' + hex.slice(10).join('');
    };
  }
  if (typeof g.crypto.subtle === 'undefined') { g.crypto.subtle = {}; }
  if (typeof g.crypto.subtle.digest === 'undefined' && typeof __chuzzDigest === 'function') {
    g.crypto.subtle.digest = function (algorithm, data) {
      var name = typeof algorithm === 'string' ? algorithm : algorithm && algorithm.name;
      var bytes = data instanceof ArrayBuffer ? new Uint8Array(data) :
        ArrayBuffer.isView(data) ? new Uint8Array(data.buffer, data.byteOffset, data.byteLength) : null;
      if (!bytes) { return Promise.reject(new TypeError('digest input must be a BufferSource')); }
      try { var result = __chuzzCopyBytes(__chuzzDigest(String(name), bytes)); return Promise.resolve(result.buffer); }
      catch (error) { return Promise.reject(error); }
    };
  }
  var subtleMethods = ['encrypt', 'decrypt', 'sign', 'verify', 'deriveKey', 'deriveBits',
    'importKey', 'exportKey', 'wrapKey', 'unwrapKey', 'generateKey'];
  if (typeof g.crypto.subtle !== 'undefined') {
    for (var i = 0; i < subtleMethods.length; i++) {
      if (typeof g.crypto.subtle[subtleMethods[i]] === 'undefined') {
        g.crypto.subtle[subtleMethods[i]] = function () {
          return Promise.reject(new DOMException('This SubtleCrypto method is unsupported', 'NotSupportedError'));
        };
      }
    }
  }
})(globalThis);"#;

/// Install browser compatibility APIs in the document's own JavaScript realm.
pub fn install(document: &mut EkojitDocument) -> Result<(), String> {
    document
        .eval_source(Arc::new(PageSource {
            text: Arc::from(CRYPTO),
            url: Arc::from("chuzz:page-compat/crypto-session"),
        }))
        .map_err(|error| format!("could not install Ekojit crypto and session APIs: {error}"))?;
    document
        .eval_source(Arc::new(PageSource {
            text: Arc::from(COMPAT),
            url: Arc::from("chuzz:page-compat/browser"),
        }))
        .map_err(|error| format!("could not install Ekojit browser compatibility APIs: {error}"))?;
    document
        .eval_source(Arc::new(PageSource {
            text: Arc::from(SCHEDULING),
            url: Arc::from("chuzz:page-compat/scheduling"),
        }))
        .map_err(|error| format!("could not install Ekojit scheduling APIs: {error}"))?;
    Ok(())
}
