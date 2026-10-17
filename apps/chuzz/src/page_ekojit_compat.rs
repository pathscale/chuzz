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

/// Install browser compatibility APIs in the document's own JavaScript realm.
pub fn install(document: &mut EkojitDocument) -> Result<(), String> {
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
