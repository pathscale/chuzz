use std::sync::Arc;

use blitz_script_ekojit::{EkojitDocument, PageSource};

const SOURCE: &str = r#"(function (g) {
  'use strict';
  function eventTarget(target) {
    target._listeners = Object.create(null);
    target.addEventListener = function (type, callback) {
      if (callback == null) return;
      if (callback && typeof callback !== 'function' && typeof callback.handleEvent !== 'function') return;
      var list = this._listeners[type] || (this._listeners[type] = []);
      if (list.indexOf(callback) < 0) list.push(callback);
    };
    target.removeEventListener = function (type, callback) {
      var list = this._listeners[type] || [];
      this._listeners[type] = list.filter(function (item) { return item !== callback; });
    };
    target.dispatchEvent = function (event) {
      event.target = this;
      (this._listeners[event.type] || []).slice().forEach(function (callback) {
        if (typeof callback === 'function') callback.call(this, event);
        else callback.handleEvent(event);
      }, this);
      var handler = this['on' + event.type];
      if (typeof handler === 'function') handler.call(this, event);
      return !event.defaultPrevented;
    };
    return target;
  }
  function Headers(init) {
    this._entries = Object.create(null);
    if (Array.isArray(init)) init.forEach(function (pair) { if (pair.length !== 2) throw new TypeError('Invalid header pair'); this.append(pair[0], pair[1]); }, this);
    else if (init && typeof init.forEach === 'function') init.forEach(function (v, k) { this.append(k, v); }, this);
    else if (init) Object.keys(init).forEach(function (key) { this.append(key, init[key]); }, this);
  }
  Headers.prototype.append = function (name, value) {
    var key = String(name).toLowerCase(), val = String(value).trim();
    if (!/^[!#$%&'*+.^_`|~0-9a-z-]+$/.test(key) || /[\r\n]/.test(val)) throw new TypeError('Invalid header');
    this._entries[key] = Object.prototype.hasOwnProperty.call(this._entries, key) ? this._entries[key] + ', ' + val : val;
  };
  Headers.prototype.set = function (name, value) { this.delete(name); this.append(name, value); };
  Headers.prototype.get = function (name) { var key = String(name).toLowerCase(); return Object.prototype.hasOwnProperty.call(this._entries, key) ? this._entries[key] : null; };
  Headers.prototype.has = function (name) { return Object.prototype.hasOwnProperty.call(this._entries, String(name).toLowerCase()); };
  Headers.prototype.delete = function (name) { delete this._entries[String(name).toLowerCase()]; };
  Headers.prototype.forEach = function (callback, receiver) { var self = this; Object.keys(this._entries).sort().forEach(function (key) { callback.call(receiver, self._entries[key], key, self); }); };
  Headers.prototype.entries = function () { var rows = []; this.forEach(function (v, k) { rows.push([k, v]); }); return rows[Symbol.iterator](); };
  Headers.prototype.keys = function () { var keys = []; this.forEach(function (value, key) { keys.push(key); }); return keys[Symbol.iterator](); };
  Headers.prototype.values = function () { var values = []; this.forEach(function (v) { values.push(v); }); return values[Symbol.iterator](); };
  Headers.prototype.getSetCookie = function () { var value = this.get('set-cookie'); return value == null ? [] : [value]; };
  if (typeof Symbol === 'function' && Symbol.iterator) Headers.prototype[Symbol.iterator] = Headers.prototype.entries;
  var blobBytes = new WeakMap(), blobMeta = new WeakMap();
  function copyPart(part) {
    if (typeof part === 'string') return __chuzzEncodeUtf8(part);
    if (part && blobBytes.has(part)) return __chuzzCopyBytes(blobBytes.get(part));
    if (part instanceof ArrayBuffer) return __chuzzCopyBytes(new Uint8Array(part));
    if (ArrayBuffer.isView(part)) return __chuzzCopyBytes(new Uint8Array(part.buffer, part.byteOffset, part.byteLength));
    return __chuzzEncodeUtf8(String(part));
  }
  function Blob(parts, options) {
    if (!(this instanceof Blob)) throw new TypeError("Failed to construct 'Blob': use new");
    var pieces = [], total = 0;
    if (parts !== undefined) {
      for (var part of parts) { var piece = copyPart(part); pieces.push(piece); total += piece.length; }
    }
    var bytes = new Uint8Array(total), offset = 0;
    for (var p = 0; p < pieces.length; p++) { bytes.set(pieces[p], offset); offset += pieces[p].length; }
    blobBytes.set(this, bytes);
    var type = options && options.type != null ? String(options.type) : '';
    blobMeta.set(this, { size: total, type: type.split('').every(function (ch) { var code = ch.charCodeAt(0); return code >= 32 && code <= 126; }) ? type.toLowerCase() : '' });
  }
  Object.defineProperties(Blob.prototype, {
    size: { get: function () { var meta = blobMeta.get(this); if (!meta) throw new TypeError('incompatible Blob receiver'); return meta.size; }, enumerable: true },
    type: { get: function () { var meta = blobMeta.get(this); if (!meta) throw new TypeError('incompatible Blob receiver'); return meta.type; }, enumerable: true },
    _bytes: { get: function () {
    var bytes = blobBytes.get(this); if (!bytes) throw new TypeError('incompatible Blob receiver');
    return __chuzzCopyBytes(bytes);
  } }
  });
  Blob.prototype.slice = function (start, end, type) {
    var bytes = blobBytes.get(this); if (!bytes) throw new TypeError('incompatible Blob receiver');
    function index(value, fallback) { if (value === undefined) return fallback; var n = Number(value); if (Number.isNaN(n)) return 0; if (!Number.isFinite(n)) return n < 0 ? 0 : bytes.length; n = Math.trunc(n); return n < 0 ? Math.max(bytes.length + n, 0) : Math.min(n, bytes.length); }
    var first = index(start, 0), last = index(end, bytes.length), finish = Math.max(last, first);
    return new Blob([bytes.subarray(first, finish)], { type: type == null ? '' : type });
  };
  Blob.prototype.arrayBuffer = function () { var bytes = blobBytes.get(this); if (!bytes) return Promise.reject(new TypeError('incompatible Blob receiver')); var copy = __chuzzCopyBytes(bytes); return Promise.resolve(copy.buffer); };
  Blob.prototype.text = function () { var bytes = blobBytes.get(this); if (!bytes) return Promise.reject(new TypeError('incompatible Blob receiver')); return Promise.resolve(__chuzzDecodeUtf8(__chuzzCopyBytes(bytes))); };
  g.Blob = Blob;
  function File(parts, name, options) {
    if (!(this instanceof File)) throw new TypeError("Failed to construct 'File': use new");
    Blob.call(this, parts, options);
    var normalized = String(name).replace(/\//g, ':');
    Object.defineProperties(this, {
      name: { value: normalized, enumerable: true },
      lastModified: { value: options && options.lastModified != null ? Number(options.lastModified) : Math.trunc(__chuzzTimeOrigin() + __chuzzMonotonicNow()), enumerable: true }
    });
  }
  File.prototype = Object.create(Blob.prototype);
  Object.defineProperty(File.prototype, 'constructor', { value: File, writable: true, configurable: true });
  g.File = File;
  function base64(bytes) {
    var alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/', encoded = '';
    for (var i = 0; i < bytes.length; i += 3) {
      var a = bytes[i], b = i + 1 < bytes.length ? bytes[i + 1] : 0, c = i + 2 < bytes.length ? bytes[i + 2] : 0;
      encoded += alphabet[a >> 2] + alphabet[((a & 3) << 4) | (b >> 4)] +
        (i + 1 < bytes.length ? alphabet[((b & 15) << 2) | (c >> 6)] : '=') +
        (i + 2 < bytes.length ? alphabet[c & 63] : '=');
    }
    return encoded;
  }
  function FileReader() {
    if (!(this instanceof FileReader)) throw new TypeError("Failed to construct 'FileReader': use new");
    eventTarget(this); this.readyState = 0; this.result = null; this.error = null;
    this.onloadstart = this.onprogress = this.onload = this.onabort = this.onerror = this.onloadend = null;
  }
  FileReader.EMPTY = FileReader.prototype.EMPTY = 0;
  FileReader.LOADING = FileReader.prototype.LOADING = 1;
  FileReader.DONE = FileReader.prototype.DONE = 2;
  var fileReads = new WeakMap();
  function readAs(reader, blob, kind) {
    if (reader.readyState === FileReader.LOADING) throw new DOMException('The object is already busy reading Blobs.','InvalidStateError');
    var bytes = blobBytes.get(blob);
    if (!bytes && blob && blob._bytes) bytes = blob._bytes;
    if (!bytes) throw new TypeError('FileReader requires a Blob');
    bytes = __chuzzCopyBytes(bytes); reader.readyState = FileReader.LOADING; reader.result = null; reader.error = null;
    var read = {}; fileReads.set(reader,read);
    reader.dispatchEvent({type:'loadstart',target:reader});
    var self = reader;
    setTimeout(function(){
      if (fileReads.get(self) !== read || self.readyState !== FileReader.LOADING) return;
      self.readyState = FileReader.DONE;
      self.result = kind === 'dataurl' ? 'data:' + (blob.type || 'application/octet-stream') + ';base64,' + base64(bytes) :
        kind === 'text' ? __chuzzDecodeUtf8(bytes) : __chuzzCopyBytes(bytes).buffer;
      self.dispatchEvent({type:'load',target:self});
      if (fileReads.get(self) === read) { fileReads.delete(self); self.dispatchEvent({type:'loadend',target:self}); }
    },0);
  }
  FileReader.prototype.readAsDataURL = function(blob){readAs(this,blob,'dataurl');};
  FileReader.prototype.readAsText = function(blob){readAs(this,blob,'text');};
  FileReader.prototype.readAsArrayBuffer = function(blob){readAs(this,blob,'arraybuffer');};
  FileReader.prototype.abort = function(){if(this.readyState!==FileReader.LOADING)return;var read=fileReads.get(this);this.readyState=FileReader.DONE;this.result=null;this.dispatchEvent({type:'abort',target:this});if(fileReads.get(this)===read){fileReads.delete(this);this.dispatchEvent({type:'loadend',target:this});}};
  g.FileReader = FileReader;
  var formEntries = new WeakMap();
  function formSnapshot(form) { var entries = formEntries.get(form); if (!entries) throw new TypeError('incompatible FormData receiver'); return entries.slice(); }
  function formValue(value, filename) {
    if (value && blobBytes.has(value)) return filename === undefined && value instanceof File ? value : new File([value], filename === undefined ? 'blob' : filename, { type: value.type, lastModified: value.lastModified });
    return String(value);
  }
  function FormData(form) { if (!(this instanceof FormData)) throw new TypeError("Failed to construct 'FormData': use new"); if (form !== undefined) throw new TypeError('HTML form extraction is unsupported'); formEntries.set(this, []); }
  FormData.prototype.append = function (name, value, filename) { var rows = formSnapshot(this); rows.push([String(name), formValue(value, filename)]); formEntries.set(this, rows); };
  FormData.prototype.set = function (name, value, filename) {
    name = String(name); var rows = formSnapshot(this), next = [], inserted = false, item = formValue(value, filename);
    for (var i = 0; i < rows.length; i++) { if (rows[i][0] !== name) next.push(rows[i]); else if (!inserted) { next.push([name, item]); inserted = true; } }
    if (!inserted) next.push([name, item]); formEntries.set(this, next);
  };
  FormData.prototype.delete = function (name) { name = String(name); formEntries.set(this, formSnapshot(this).filter(function (row) { return row[0] !== name; })); };
  FormData.prototype.get = function (name) { var rows = formSnapshot(this), key = String(name); for (var i = 0; i < rows.length; i++) if (rows[i][0] === key) return rows[i][1]; return null; };
  FormData.prototype.getAll = function (name) { var key = String(name); return formSnapshot(this).filter(function (row) { return row[0] === key; }).map(function (row) { return row[1]; }); };
  FormData.prototype.has = function (name) { return this.get(name) !== null; };
  FormData.prototype.forEach = function (callback, receiver) { formSnapshot(this).forEach(function (row) { callback.call(receiver, row[1], row[0], this); }, this); };
  FormData.prototype.entries = function () { return formSnapshot(this)[Symbol.iterator](); };
  FormData.prototype.keys = function () { return formSnapshot(this).map(function (row) { return row[0]; })[Symbol.iterator](); };
  FormData.prototype.values = function () { return formSnapshot(this).map(function (row) { return row[1]; })[Symbol.iterator](); };
  if (typeof Symbol === 'function' && Symbol.iterator) FormData.prototype[Symbol.iterator] = FormData.prototype.entries;
  g.FormData = FormData;
  function ascii(text) { return __chuzzEncodeUtf8(text); }
  function joinBytes(chunks) { var size = chunks.reduce(function (sum, chunk) { return sum + chunk.length; }, 0), output = new Uint8Array(size), at = 0; chunks.forEach(function (chunk) { output.set(chunk, at); at += chunk.length; }); return output; }
  function quoteHeader(value) { return String(value).replace(/%/g, '%25').replace(/[\r\n]/g, function (ch) { return ch === '\r' ? '%0D' : '%0A'; }).replace(/\\/g, '\\\\').replace(/"/g, '\\"'); }
  function encodeFormData(form, boundary) {
    var chunks = [], rows = formSnapshot(form);
    rows.forEach(function (row) {
      var name = quoteHeader(row[0]), value = row[1], file = value && blobBytes.has(value), head = '--' + boundary + '\r\nContent-Disposition: form-data; name="' + name + '"';
      if (file) { head += '; filename="' + quoteHeader(value.name) + '"\r\nContent-Type: ' + (value.type || 'application/octet-stream'); }
      chunks.push(ascii(head + '\r\n\r\n'), file ? __chuzzCopyBytes(blobBytes.get(value)) : ascii(value), ascii('\r\n'));
    });
    chunks.push(ascii('--' + boundary + '--\r\n')); return joinBytes(chunks);
  }
  function bytesOf(body) {
    if (body == null) return new Uint8Array(0);
    if (typeof body === 'string') return __chuzzEncodeUtf8(body);
    if (Array.isArray(body)) return __chuzzCopyBytes(body);
    if (body && blobBytes.has(body)) return __chuzzCopyBytes(blobBytes.get(body));
    return copyPart(body);
  }
  function byteIndex(source, needle, from) {
    outer: for (var i = from || 0; i <= source.length - needle.length; i++) { for (var j = 0; j < needle.length; j++) if (source[i + j] !== needle[j]) continue outer; return i; }
    return -1;
  }
  function multipartDelimiter(source, delimiter, from) {
    var at = from || 0;
    while ((at = byteIndex(source, delimiter, at)) >= 0) {
      var after = at + delimiter.length;
      if ((source[after] === 45 && source[after + 1] === 45) || (source[after] === 13 && source[after + 1] === 10)) return at;
      at += 1;
    }
    return -1;
  }
  function decodeFormHeader(value) {
    return value.replace(/\\(["\\])/g, '$1').replace(/%(25|0D|0A)/gi, function (escape) {
      var code = escape.toUpperCase(); return code === '%25' ? '%' : code === '%0D' ? '\r' : '\n';
    });
  }
  function responseFormData(contentType, source) {
    var form = new FormData();
    if (/^application\/x-www-form-urlencoded(?:\s*;|$)/i.test(contentType)) {
      for (var row of new URLSearchParams(__chuzzDecodeUtf8(source)).entries()) form.append(row[0], row[1]);
      return form;
    }
    var match = /^multipart\/form-data\s*;\s*boundary=(?:"([^"]+)"|([^;\s]+))(?:\s*;|$)/i.exec(contentType);
    if (!match) throw new TypeError('unsupported FormData response content type');
    var boundary = match[1] || match[2], marker = ascii('--' + boundary), delimiter = ascii('\r\n--' + boundary), headerEnd = ascii('\r\n\r\n'), cursor = marker.length;
    if (byteIndex(source, marker, 0) !== 0) throw new TypeError('invalid multipart opening boundary');
    if (source[cursor] === 45 && source[cursor + 1] === 45) {
      cursor += 2;
      if (cursor === source.length || (source[cursor] === 13 && source[cursor + 1] === 10 && cursor + 2 === source.length)) return form;
      throw new TypeError('invalid empty multipart body');
    }
    if (source[cursor] !== 13 || source[cursor + 1] !== 10) throw new TypeError('invalid multipart opening boundary');
    cursor += 2;
    while (true) {
      var headersAt = byteIndex(source, headerEnd, cursor);
      if (headersAt < 0) throw new TypeError('invalid multipart headers');
      var lines = __chuzzDecodeUtf8(source.subarray(cursor, headersAt)).split('\r\n'), disposition = null, fileType = '';
      for (var lineIndex = 0; lineIndex < lines.length; lineIndex++) {
        var line = lines[lineIndex], colon = line.indexOf(':');
        if (colon < 1) throw new TypeError('invalid multipart header');
        var headerName = line.slice(0, colon).trim().toLowerCase(), headerValue = line.slice(colon + 1).trim();
        if (headerName === 'content-disposition') {
          if (disposition) throw new TypeError('duplicate multipart disposition');
          disposition = /form-data;\s*name="((?:\\.|[^"])*)"(?:;\s*filename="((?:\\.|[^"])*)")?/i.exec(headerValue);
          if (!disposition || disposition[0] !== headerValue) throw new TypeError('invalid multipart disposition');
        } else if (headerName === 'content-type') fileType = headerValue;
      }
      if (!disposition) throw new TypeError('invalid multipart disposition');
      var bodyAt = headersAt + headerEnd.length, boundaryAt = multipartDelimiter(source, delimiter, bodyAt);
      if (boundaryAt < 0) throw new TypeError('invalid multipart closing boundary');
      var payload = source.subarray(bodyAt, boundaryAt), name = decodeFormHeader(disposition[1]);
      if (disposition[2] !== undefined) {
        var filename = decodeFormHeader(disposition[2]);
        form.append(name, new File([payload], filename, { type: fileType }));
      } else form.append(name, __chuzzDecodeUtf8(payload));
      cursor = boundaryAt + delimiter.length;
      if (source[cursor] === 45 && source[cursor + 1] === 45) {
        cursor += 2; if (cursor === source.length) return form;
        if (source[cursor] === 13 && source[cursor + 1] === 10 && cursor + 2 === source.length) return form;
        throw new TypeError('invalid multipart epilogue');
      }
      if (source[cursor] !== 13 || source[cursor + 1] !== 10) throw new TypeError('invalid multipart boundary terminator');
      cursor += 2;
    }
  }
  function makeResponse(reply) {
    if (!reply || !reply.ok) throw new TypeError(reply && reply.error || 'Network request failed');
    var data = bytesOf(reply.bytes), consumed = false, response = Object.create(Response.prototype);
    response.ok = reply.status >= 200 && reply.status < 300;
    response.status = reply.status; response.statusText = reply.statusText || '';
    response.url = reply.responseURL || reply.url || ''; response.redirected = !!reply.redirected;
    response.type = 'basic'; response.headers = new Headers(reply.headers || []);
    response.bodyUsed = false; response.body = null;
    function consume(converter) {
      if (consumed) return Promise.reject(new TypeError('body already used'));
      consumed = true; response.bodyUsed = true;
      return Promise.resolve().then(function () { return converter(data); });
    }
    response.arrayBuffer = function () { return consume(function (source) { var copy = new Uint8Array(source.length); copy.set(source); return copy.buffer; }); };
    response.bytes = function () { return consume(function (source) { var copy = new Uint8Array(source.length); copy.set(source); return copy; }); };
    response.text = function () { return consume(function (source) { return __chuzzDecodeUtf8(source); }); };
    response.json = function () { return response.text().then(function (text) { return JSON.parse(text); }); };
    response.blob = function () { return consume(function (source) { return new Blob([source], { type: response.headers.get('content-type') || '' }); }); };
    response.formData = function () { return consume(function (source) { return responseFormData(response.headers.get('content-type') || '', source); }); };
    response.clone = function () {
      if (consumed) throw new TypeError('body already used');
      var copy = new Uint8Array(data.length); copy.set(data);
      return makeResponse({ ok: true, status: response.status, statusText: response.statusText, url: response.url, responseURL: response.url, redirected: response.redirected, headers: response.headers, bytes: copy });
    };
    return response;
  }
  function requestRecord(input, init) {
    init = init || {};
    var url = input && typeof input === 'object' && input.url !== undefined ? input.url : String(input);
    url = new URL(url, g.location && g.location.href || undefined).href;
    var headers = new Headers(init.headers || (input && input.headers));
    var body = init.body === undefined ? input && input._body : init.body, bodyBytes = null;
    if (body != null) {
      if (formEntries.has(body)) {
        var nonce = __chuzzRandomBytes(18), boundary = '----chuzz-';
        for (var index = 0; index < nonce.length; index++) boundary += ('0' + nonce[index].toString(16)).slice(-2);
        bodyBytes = encodeFormData(body, boundary);
        if (!headers.has('content-type')) headers.set('content-type', 'multipart/form-data; boundary=' + boundary);
      } else bodyBytes = bytesOf(body);
    }
    var entries = []; headers.forEach(function (value, name) { entries.push([name, value]); });
    return { url: url, method: String(init.method || (input && input.method) || 'GET').toUpperCase(), headers: entries, body: bodyBytes, credentials: init.credentials || (input && input.credentials) || 'same-origin' };
  }
  function Request(input, init) { var record = requestRecord(input, init); this.url = record.url; this.method = record.method; this.headers = new Headers(record.headers); this.credentials = record.credentials; this._body = record.body; }
  function Response(body, init) {
    init = init || {}; var data = bytesOf(body), reply = { ok: true, status: init.status == null ? 200 : init.status, statusText: init.statusText || '', url: '', headers: init.headers || [], bytes: data };
    return makeResponse(reply);
  }
  Response.json = function (value, init) { init = init || {}; var headers = new Headers(init.headers); if (!headers.has('content-type')) headers.set('content-type', 'application/json'); return new Response(JSON.stringify(value), { status: init.status, statusText: init.statusText, headers: headers }); };
  Response.error = function () { var response = new Response(null, { status: 0 }); response.type = 'error'; response.ok = false; return response; };
  Response.redirect = function (url, status) { if ([301, 302, 303, 307, 308].indexOf(status || 302) < 0) throw new RangeError('Invalid redirect status'); var response = new Response(null, { status: status || 302, headers: { location: new URL(url, g.location.href).href } }); response.type = 'opaqueredirect'; return response; };
  g.Headers = Headers; g.Request = Request; g.Response = Response;
  if (typeof __chuzzNetAsync === 'function') {
    g.fetch = function (input, init) {
      init = init || {};
      return new Promise(function (resolve, reject) {
        var signal;
        try { signal = init.signal; } catch (error) { reject(error); return; }
        var settled = false, aborted = false, id = null, idAssigned = false, listening = false;
        function cleanup() {
          if (!listening) return;
          listening = false;
          try { signal.removeEventListener('abort', onAbort); } catch (_) {}
        }
        function releaseCallback() {
          if (idAssigned) { try { __chuzzNetForget(id); } catch (_) {} }
        }
        function abortReason() {
          return signal.reason === undefined
            ? new DOMException('The operation was aborted.', 'AbortError') : signal.reason;
        }
        function onAbort() {
          if (settled) return;
          settled = true;
          aborted = true;
          cleanup();
          if (idAssigned) releaseCallback();
          try { reject(abortReason()); } catch (error) { reject(error); }
        }
        if (signal && signal.aborted) {
          settled = true;
          reject(abortReason());
          return;
        }
        if (signal) {
          if (typeof signal.addEventListener !== 'function' || typeof signal.removeEventListener !== 'function') {
            settled = true;
            reject(new TypeError('signal must be an AbortSignal'));
            return;
          }
          try {
            listening = true;
            signal.addEventListener('abort', onAbort);
            if (signal.aborted) { onAbort(); return; }
          } catch (error) {
            settled = true;
            cleanup();
            reject(error);
            return;
          }
        }
        var request;
        try { request = requestRecord(input, init); }
        catch (error) { settled = true; cleanup(); reject(error); return; }
        // Request getters can synchronously abort while normalization runs.
        if (settled) return;
        try {
          id = __chuzzNetAsync(request, function (reply) {
            if (settled) return;
            settled = true;
            cleanup();
            try { resolve(makeResponse(reply)); } catch (error) { reject(error); }
          });
          idAssigned = true;
          if (aborted) releaseCallback();
        } catch (error) {
          if (settled) return;
          settled = true;
          cleanup();
          reject(error);
        }
      });
    };
  }
  function XMLHttpRequest() {
    if (!(this instanceof XMLHttpRequest)) throw new TypeError("XMLHttpRequest must be constructed");
    eventTarget(this); this.readyState = 0; this.responseType = ''; this.response = null; this.responseText = '';
    this.responseURL = ''; this.status = 0; this.statusText = ''; this.timeout = 0; this.withCredentials = false;
    this._headers = new Headers(); this._responseHeaders = new Headers(); this._async = true; this._sent = false; this._sequence = 0; this._send_generation = 0;
  }
  XMLHttpRequest.UNSENT = 0; XMLHttpRequest.OPENED = 1; XMLHttpRequest.HEADERS_RECEIVED = 2; XMLHttpRequest.LOADING = 3; XMLHttpRequest.DONE = 4;
  XMLHttpRequest.prototype.open = function (method, url, async) {
    this._method = String(method).toUpperCase(); this._url = new URL(String(url), g.location && g.location.href || undefined).href;
    this._async = async !== false; this._sent = false; this._sequence += 1;
    this.status = 0; this.statusText = ''; this.response = null; this.responseText = ''; this.responseURL = '';
    this._headers = new Headers(); this._responseHeaders = new Headers();
    this.readyState = 1; this.dispatchEvent({ type: 'readystatechange' });
  };
  XMLHttpRequest.prototype.setRequestHeader = function (name, value) { if (this.readyState !== 1) throw new TypeError('XHR is not opened'); this._headers.append(name, value); };
  XMLHttpRequest.prototype.getResponseHeader = function (name) { return this.readyState < 2 ? null : this._responseHeaders.get(name); };
  XMLHttpRequest.prototype.getAllResponseHeaders = function () { var rows = []; this._responseHeaders.forEach(function (value, name) { rows.push(name + ': ' + value + '\r\n'); }); return this.readyState < 2 ? '' : rows.join(''); };
  XMLHttpRequest.prototype.send = function (body) {
    var self = this;
    if (this.readyState !== 1 || this._sent) throw new TypeError('XHR is not opened or is already sending');
    var sequence = this._sequence, generation = ++this._send_generation, async = this._async;
    this._sent = true;
    var request = { url: this._url, method: this._method, headers: [], body: body == null ? null : bytesOf(body), credentials: this.withCredentials ? 'include' : 'same-origin' };
    this._headers.forEach(function (value, name) { request.headers.push([name, value]); });
    function sameSend() { return self._sequence === sequence && self._send_generation === generation; }
    function active() { return sameSend() && self._sent; }
    function dispatch(event) {
      try { self.dispatchEvent(event); }
      catch (error) {
        if (active()) self._sent = false;
        throw error;
      }
    }
    function sendException(error) {
      if (!sameSend()) return;
      self._sent = false;
      dispatch({ type: 'error', error: error });
      if (!sameSend()) return;
      dispatch({ type: 'loadend' });
    }
    function failedReply(reply) {
      self._sent = false; self.readyState = 4;
      dispatch({ type: 'readystatechange' });
      if (self._sequence !== sequence) return;
      dispatch({ type: 'error', message: reply && reply.error });
      if (self._sequence !== sequence) return;
      dispatch({ type: 'loadend' });
    }
    function finish(reply) {
      if (!active()) return;
      if (!reply || !reply.ok) { failedReply(reply); return; }
      self.status = reply.status; self.statusText = reply.statusText || ''; self.responseURL = reply.responseURL || reply.url || '';
      self._responseHeaders = new Headers(reply.headers || []); self.readyState = 2;
      dispatch({ type: 'readystatechange' });
      if (!active()) return;
      self.readyState = 3; dispatch({ type: 'readystatechange' });
      if (!active()) return;
      dispatch({ type: 'progress', loaded: (reply.bytes || []).length, lengthComputable: false });
      if (!active()) return;
      var responseBytes = bytesOf(reply.bytes), text = __chuzzDecodeUtf8(responseBytes);
      self.responseText = text;
      if (self.responseType === 'document') {
        var error = new Error('XHR responseType document is unsupported'); error.name = 'NotSupportedError'; self.response = null;
        dispatch({ type: 'error', error: error });
        if (!active()) return;
      } else if (self.responseType === 'json') { try { self.response = JSON.parse(text); } catch (_) { self.response = null; } }
      else if (self.responseType === 'arraybuffer') self.response = responseBytes.buffer;
      else if (self.responseType === 'blob') self.response = new Blob([responseBytes], { type: self._responseHeaders.get('content-type') || '' });
      else self.response = text;
      if (!active()) return;
      self._sent = false; self.readyState = 4; dispatch({ type: 'readystatechange' });
      if (self._sequence !== sequence) return;
      dispatch({ type: 'load' });
      if (self._sequence !== sequence) return;
      dispatch({ type: 'loadend' });
    }
    dispatch({ type: 'loadstart' });
    if (!active()) return;
    if (async) {
      try {
        __chuzzNetAsync(request, finish);
      } catch (error) { sendException(error); }
    } else {
      var reply;
      try { reply = __chuzzNetSync(request); }
      catch (error) { sendException(error); return; }
      try { finish(reply); }
      catch (error) { sendException(error); }
    }
  };
  XMLHttpRequest.prototype.abort = function () {
    var active = this._sent, sequence = ++this._sequence;
    this._sent = false; this.status = 0; this.statusText = ''; this.response = null; this.responseText = ''; this.responseURL = '';
    this._responseHeaders = new Headers();
    if (active) {
      this.readyState = 4; this.dispatchEvent({ type: 'readystatechange' });
      if (this._sequence !== sequence) return;
      this.dispatchEvent({ type: 'abort' });
      if (this._sequence !== sequence) return;
      this.dispatchEvent({ type: 'loadend' });
      if (this._sequence !== sequence) return;
    }
    this.readyState = 0;
  };
  g.XMLHttpRequest = XMLHttpRequest;
})(globalThis);"#;

pub(crate) fn install(document: &mut EkojitDocument) -> Result<(), String> {
    document
        .eval_source(Arc::new(PageSource {
            text: Arc::from(SOURCE),
            url: Arc::from("chuzz:page-bootstrap/network"),
        }))
        .map(|_| ())
        .map_err(|error| format!("could not install Ekojit network bootstrap: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use blitz_dom::Document as _;
    use serde_json::json;

    fn page() -> crate::page_document::PageDocument {
        crate::install_crypto_provider();
        let base = url::Url::parse("https://example.invalid/").unwrap();
        let provider = crate::document_loader::NetProvider::new(None);
        let initiator = provider.browser_initiator(base.clone());
        let net = Arc::new(provider.for_document(base.clone(), &initiator));
        let lifetime = blitz_dom::release::Teardown::new();
        let signal = blitz_traits::net::AbortSignal::default();
        crate::page_document::PageDocument::from_html(
            Arc::from("<!doctype html><html><head></head><body></body></html>"),
            blitz_dom::DocumentConfig {
                base_url: Some(base.to_string()),
                ..Default::default()
            },
            crate::script_host::HostInstallContext {
                net,
                base: Some(base),
                deadline: std::time::Duration::from_secs(2),
                wake: crate::net_bridge::PageWake::default(),
                lifetime,
                signal,
                session_backing: None,
            },
        )
        .expect("construct page with live network and crypto intrinsics")
    }

    #[test]
    fn page_ekojit_blob_and_form_data_preserve_owned_bytes() {
        let mut page = page();
        page.eval(r#"
var results = [], failures = [], formStage = 'waiting';
function check(value, message) { if (!value) throw new Error(message); }
function failed(error) { failures.push(String(error.stack || error)); }
var source = new Uint8Array([9, 0, 255, 65, 8]);
var blob = new Blob([new Uint8Array(source.buffer, 1, 3)], { type: 'APPLICATION/OCTET-STREAM' });
source[1] = 99;
var exposed = blob._bytes; exposed[0] = 77;
check(blob.size === 3 && blob.type === 'application/octet-stream', 'Blob metadata');
check(blob.slice(Infinity).size === 0 && blob.slice(-2).size === 2 && blob.slice(0, NaN).size === 0, 'slice limits');
blob.arrayBuffer().then(function (buffer) {
  var bytes = new Uint8Array(buffer);
  check(bytes.length === 3 && bytes[0] === 0 && bytes[1] === 255 && bytes[2] === 65, 'owned offset bytes');
  results.push('blob');
}).catch(failed);
var file = new File([blob], 'folder/name.bin', { lastModified: 123 });
check(file instanceof Blob && file.name === 'folder:name.bin' && file.lastModified === 123, 'File metadata');
var form = new FormData();
form.append('field', 'old'); form.append('other', 'kept'); form.append('field', 'duplicate');
form.set('field', 'new');
var keys = form.keys();
check(form.getAll('field').join(',') === 'new' && keys.next().value === 'field' && keys.next().value === 'other' && keys.next().done, 'set ordering');
form.append('100%0D', file, 'x%0A.bin');
var request = new Request('/upload', { method: 'POST', body: form });
form.set('field', 'mutated');
var response = new Response(request._body, { headers: request.headers });
var cloned = response.clone();
response.formData().then(function (parsed) {
  formStage = 'parsed';
  check(parsed.get('field') === 'new' && parsed.get('other') === 'kept', 'request snapshot');
  var upload = parsed.get('100%0D');
  check(upload instanceof File && upload.name === 'x%0A.bin', 'multipart metadata');
  formStage = 'reading'; return upload.arrayBuffer();
}).then(function (buffer) {
  formStage = 'read';
  var bytes = new Uint8Array(buffer);
  check(bytes[0] === 0 && bytes[1] === 255 && bytes[2] === 65, 'binary multipart');
  results.push('form');
}).catch(failed);
check(response.bodyUsed, 'shared body consumption');
response.text().then(function () { failed('second read succeeded'); }, function () { results.push('used'); });
cloned.blob().then(function (copy) { check(copy.size > 3, 'clone bytes'); results.push('clone'); }).catch(failed);
new Response('a=one+two&a=%FF&empty=&bad=%&', { headers: { 'content-type': 'application/x-www-form-urlencoded' } }).formData().then(function (parsed) {
  check(parsed.getAll('a').length === 2 && parsed.get('a') === 'one two' && parsed.get('bad') === '%' && !parsed.has(''), 'URL encoded fields');
  results.push('urlencoded');
}).catch(failed);
new Response('broken', { headers: { 'content-type': 'multipart/form-data; boundary=x' } }).formData().then(function () { failed('parse succeeded'); }, function () { results.push('rejected'); });
fetch('data:application/octet-stream,%00%FF%41').then(function (reply) { return reply.bytes(); }).then(function (bytes) {
  check(bytes.length === 3 && bytes[0] === 0 && bytes[1] === 255 && bytes[2] === 65, 'native reply bytes');
  results.push('network');
}).catch(failed);
"#).expect("evaluate Blob and FormData on the live page");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            page.poll(None);
            if page
                .eval_json("results.length + failures.length")
                .unwrap()
                .as_u64()
                .unwrap()
                >= 7
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let diagnostics = page.diagnostic_batch(0, 0, 100).unwrap();
        assert!(diagnostics.errors.entries.is_empty(), "{diagnostics:?}");
        assert_eq!(
            page.eval_json("failures").unwrap(),
            json!([]),
            "completed tasks: {}",
            page.eval_json("results").unwrap()
        );
        assert_eq!(
            page.eval_json("results.sort()").unwrap(),
            json!([
                "blob",
                "clone",
                "form",
                "network",
                "rejected",
                "urlencoded",
                "used"
            ]),
            "multipart stage: {}",
            page.eval_json("formStage").unwrap()
        );
    }

    #[test]
    fn page_ekojit_xhr_reentrant_callbacks_are_generation_isolated() {
        let mut page = page();
        page.eval(r#"
function check(value, message) { if (!value) throw new Error(message); }
function reply(body, ok) {
  var bytes = new Uint8Array(body.length);
  for (var i = 0; i < body.length; i++) bytes[i] = body.charCodeAt(i);
  return { ok: ok !== false, status: ok === false ? 0 : 200, statusText: ok === false ? '' : 'OK', url: '', headers: [['x-reply', body]], bytes: bytes, error: ok === false ? 'offline' : undefined };
}
var nativeAsync = __chuzzNetAsync, nativeSync = __chuzzNetSync;
var asyncCalls = [], pendingReplies = [];
globalThis.__chuzzNetAsync = function (request, callback) { asyncCalls.push(request.url); pendingReplies.push({ request: request, callback: callback }); };
globalThis.__chuzzNetSync = function (request) { if (request.url.indexOf('/failure') >= 0) return reply('offline', false); return request.url.indexOf('/sync-b') >= 0 ? reply('sync-B') : reply('sync-A'); };
function deliver(index, body, ok) { pendingReplies[index].callback(reply(body, ok)); }
function observe(label, xhr, events) {
  ['loadstart', 'readystatechange', 'progress', 'error', 'abort', 'load', 'loadend'].forEach(function (type) {
    xhr.addEventListener(type, function () { events.push([label, type, xhr.readyState, xhr.responseText, xhr.status, xhr.getResponseHeader('x-reply')]); });
  });
}
var events = [], x = new XMLHttpRequest(), bOpened = false, headBoundary = -1;
observe('head', x, events);
x.addEventListener('readystatechange', function () {
  if (x.readyState === 2 && !bOpened) { bOpened = true; x.open('GET', '/head-b'); x.send(); headBoundary = events.length; }
});
x.open('GET', '/head-a'); x.send();
check(asyncCalls.length === 1 && asyncCalls[0].indexOf('/head-a') >= 0, 'HEADERS case dispatches A first');
deliver(0, 'stale-A');
check(events.length === headBoundary, 'superseded A emits no further event after HEADERS callback');
check(asyncCalls.length === 2 && asyncCalls[1].indexOf('/head-b') >= 0, 'HEADERS callback dispatches B');
check(x.readyState === 1 && x.responseText === '', 'A cannot advance B after HEADERS callback');
deliver(1, 'fresh-B');
check(x.readyState === 4 && x.responseText === 'fresh-B' && x.getResponseHeader('x-reply') === 'fresh-B', 'B response survives A completion');
check(events.filter(function (e) { return e[1] === 'loadend'; }).length === 1, 'only B receives loadend after HEADERS reentrancy');

var abortEvents = [], y = new XMLHttpRequest(), abortReopened = false;
observe('abort', y, abortEvents);
y.addEventListener('progress', function () { if (!abortReopened) y.abort(); });
y.addEventListener('abort', function () { if (!abortReopened) { abortReopened = true; y.open('GET', '/abort-b'); y.send(); } });
y.open('GET', '/abort-a'); y.send(); deliver(2, 'partial-A');
check(abortEvents.filter(function (e) { return e[1] === 'loadend'; }).length === 0, 'reopening from abort suppresses old loadend');
check(asyncCalls.length === 4 && asyncCalls[2].indexOf('/abort-a') >= 0 && asyncCalls[3].indexOf('/abort-b') >= 0, 'abort callback starts B after A');
check(y.readyState === 1 && y.responseText === '', 'old abort reset cannot clear B');
check(abortEvents.filter(function (e) { return e[1] === 'abort'; }).length === 1, 'A aborts exactly once');
check(abortEvents.some(function (e) { return e[1] === 'abort' && e[2] === 4; }), 'abort event is delivered before supersession');
deliver(3, 'abort-B');
check(y.responseText === 'abort-B' && abortEvents.filter(function (e) { return e[1] === 'loadend'; }).length === 1, 'abort B response and its loadend survive');

var startEvents = [], z = new XMLHttpRequest(), startReopened = false;
observe('start', z, startEvents);
z.addEventListener('loadstart', function () { if (!startReopened) { startReopened = true; z.open('GET', '/start-b'); z.send(); } });
z.open('GET', '/start-a'); z.send();
check(asyncCalls.length === 5 && asyncCalls[4].indexOf('/start-b') >= 0, 'loadstart supersession suppresses A host call');
deliver(4, 'start-B');
check(z.responseText === 'start-B', 'loadstart B completes');

var syncEvents = [], s = new XMLHttpRequest(), syncReopened = false;
observe('sync', s, syncEvents);
s.addEventListener('readystatechange', function () { if (s.readyState === 2 && !syncReopened) { syncReopened = true; s.open('GET', '/sync-b', false); s.send(); } });
s.open('GET', '/sync-a', false); s.send();
check(s.responseText === 'sync-B' && s.status === 200, 'synchronous reentrant B owns final response');
check(syncEvents.filter(function (e) { return e[1] === 'loadend'; }).length === 1, 'superseded synchronous A stops terminal events');
var ordinarySyncEvents = [], ordinarySync = new XMLHttpRequest(); observe('ordinary-sync', ordinarySync, ordinarySyncEvents);
ordinarySync.open('GET', '/sync-ordinary', false); ordinarySync.send();
check(ordinarySync.responseText === 'sync-A' && ordinarySyncEvents.filter(function (e) { return ['loadstart', 'progress', 'load', 'loadend'].indexOf(e[1]) >= 0; }).map(function (e) { return e[1]; }).join(',') === 'loadstart,progress,load,loadend', 'ordinary synchronous send event order');

var terminalEvents = [], t = new XMLHttpRequest(), terminalReopened = false, terminalBoundary = -1;
observe('done', t, terminalEvents);
t.addEventListener('readystatechange', function () { if (t.readyState === 4 && !terminalReopened) { terminalReopened = true; t.open('GET', '/done-b'); t.send(); terminalBoundary = terminalEvents.length; } });
t.open('GET', '/done-a'); t.send(); deliver(5, 'done-A');
check(terminalEvents.length === terminalBoundary, 'DONE reopen suppresses old load and loadend');
deliver(6, 'done-B');
check(t.responseText === 'done-B' && terminalEvents.filter(function (e) { return e[1] === 'loadend'; }).length === 1, 'DONE B receives its own terminal sequence');

var loadEvents = [], l = new XMLHttpRequest(), loadReopened = false, loadBoundary = -1;
observe('load', l, loadEvents);
l.addEventListener('load', function () {
  if (!loadReopened) { loadReopened = true; l.open('GET', '/load-b'); l.send(); loadBoundary = loadEvents.length; }
});
l.open('GET', '/load-a'); l.send(); deliver(7, 'load-A');
check(loadEvents.length === loadBoundary, 'load reopen suppresses old loadend');
deliver(8, 'load-B');
check(l.readyState === 4 && l.responseText === 'load-B' && l.getResponseHeader('x-reply') === 'load-B', 'load B keeps its response');
check(loadEvents.filter(function (e) { return e[1] === 'loadend'; }).length === 1, 'load B alone receives loadend');

var errorEvents = [], e = new XMLHttpRequest(), errorReopened = false, errorBoundary = -1;
observe('error-reopen', e, errorEvents);
e.addEventListener('error', function () {
  if (!errorReopened) { errorReopened = true; e.open('GET', '/error-b'); e.send(); errorBoundary = errorEvents.length; }
});
e.open('GET', '/error-a'); e.send(); deliver(9, 'failed-A', false);
check(errorEvents.length === errorBoundary, 'error reopen suppresses old loadend');
deliver(10, 'error-B');
check(e.readyState === 4 && e.responseText === 'error-B' && e.getResponseHeader('x-reply') === 'error-B', 'error B keeps its response');
check(errorEvents.filter(function (event) { return event[1] === 'loadend'; }).length === 1, 'error B alone receives loadend');

var failedEvents = [], f = new XMLHttpRequest(); observe('failed', f, failedEvents);
var failedMessage;
f.addEventListener('error', function (event) { failedMessage = event.message; });
f.open('GET', '/failure', false); f.send();
check(failedEvents.map(function (e) { return e[1]; }).filter(function (type) { return ['loadstart', 'readystatechange', 'error', 'loadend'].indexOf(type) >= 0; }).join(',') === 'readystatechange,loadstart,readystatechange,error,loadend', 'failed transport terminal event order');
check(failedMessage === 'offline', 'failed reply retains its transport message');

var throwing = new XMLHttpRequest(), listenerError = { marker: 'listener failure' }, thrownEvents = [], sendReturned = false;
observe('throwing', throwing, thrownEvents);
throwing.addEventListener('readystatechange', function () { if (throwing.readyState === 2) throw listenerError; });
throwing.open('GET', '/throw-sync', false);
throwing.send(); sendReturned = true;
check(sendReturned, 'synchronous listener exception is handled by send');
check(throwing.readyState === 2 && throwing.responseText === '' && throwing._sent === false, 'intermediate listener failure preserves partial state and releases send');
check(thrownEvents.filter(function (e) { return e[1] === 'error' || e[1] === 'loadend'; }).map(function (e) { return e[1]; }).join(',') === 'error,loadend', 'intermediate listener failure dispatches error then loadend');

var terminalThrow = new XMLHttpRequest(), terminalThrowEvents = [], loadError = { marker: 'load listener failure' };
observe('terminal-throw', terminalThrow, terminalThrowEvents);
terminalThrow.addEventListener('load', function () { throw loadError; });
terminalThrow.open('GET', '/throw-load', false); terminalThrow.send();
check(terminalThrow.readyState === 4 && terminalThrow.responseText === 'sync-A', 'load listener failure retains completed response');
check(terminalThrowEvents.filter(function (e) { return ['load', 'error', 'loadend'].indexOf(e[1]) >= 0; }).map(function (e) { return e[1]; }).join(',') === 'load,error,loadend', 'terminal listener failure dispatches error then loadend');

var reentrantThrow = new XMLHttpRequest(), reopenError = { marker: 'reentrant listener failure' }, reopenedFromThrow = false;
var reentrantThrowEvents = [];
observe('reentrant-throw', reentrantThrow, reentrantThrowEvents);
reentrantThrow.addEventListener('readystatechange', function () {
  if (reentrantThrow.readyState === 2 && !reopenedFromThrow) {
    reopenedFromThrow = true;
    reentrantThrow.open('GET', '/pending-b');
    reentrantThrow.send();
    throw reopenError;
  }
});
reentrantThrow.open('GET', '/throw-reopen-a', false);
reentrantThrow.send();
check(reentrantThrow.readyState === 1 && reentrantThrow._sent === true && reentrantThrow.responseText === '', 'old listener exception leaves pending B active');
check(reentrantThrowEvents.every(function (e) { return e[1] !== 'error' && e[1] !== 'loadend'; }), 'old listener exception emits no terminal event while B is pending');
var pendingB = asyncCalls.length - 1;
check(pendingReplies[pendingB].request.url.indexOf('/pending-b') >= 0, 'reentrant listener starts asynchronous B');
deliver(pendingB, 'pending-B');
check(reentrantThrow.readyState === 4 && reentrantThrow.responseText === 'pending-B' && reentrantThrow._sent === false, 'pending B completes after A listener exception');

var asyncThrow = new XMLHttpRequest(), asyncThrowEvents = [], asyncListenerError = { marker: 'deferred listener failure' };
observe('async-throw', asyncThrow, asyncThrowEvents);
asyncThrow.addEventListener('readystatechange', function () { if (asyncThrow.readyState === 2) throw asyncListenerError; });
asyncThrow.open('GET', '/async-throw'); asyncThrow.send();
var asyncCaught;
try { deliver(asyncCalls.length - 1, 'async-throw-body'); } catch (error) { asyncCaught = error; }
check(asyncCaught === asyncListenerError, 'deferred listener exception propagates to callback caller');
check(asyncThrow._sent === false && asyncThrowEvents.filter(function (e) { return e[1] === 'error' || e[1] === 'loadend'; }).length === 0, 'deferred listener exception is not converted to XHR failure events');

var accessorThrow = new XMLHttpRequest(), accessorEvents = [], accessorReads = 0, accessorError = {};
Object.defineProperty(accessorError, 'message', { get: function () { accessorReads++; throw new Error('message getter must not run'); } });
observe('accessor-throw', accessorThrow, accessorEvents);
var recoveredValue;
accessorThrow.addEventListener('error', function (event) { recoveredValue = event.error; });
accessorThrow.addEventListener('readystatechange', function () { if (accessorThrow.readyState === 2) throw accessorError; });
accessorThrow.open('GET', '/accessor-throw', false);
var accessorSendReturned = false; accessorThrow.send(); accessorSendReturned = true;
check(accessorSendReturned && accessorReads === 0, 'synchronous recovery does not inspect thrown value properties');
check(recoveredValue === accessorError && accessorThrow._sent === false, 'synchronous recovery forwards thrown value and releases send');
check(accessorEvents.filter(function (e) { return e[1] === 'error' || e[1] === 'loadend'; }).map(function (e) { return e[1]; }).join(',') === 'error,loadend', 'synchronous recovery delivers error then loadend');

var asyncFailed = new XMLHttpRequest(), asyncFailedEvents = [], asyncError = { marker: 'deferred error listener failure' };
observe('async-failed', asyncFailed, asyncFailedEvents);
var asyncErrorCalls = 0;
asyncFailed.addEventListener('error', function () { if (++asyncErrorCalls === 1) throw asyncError; });
asyncFailed.open('GET', '/async-failure'); asyncFailed.send();
var asyncFailureCaught;
try { deliver(asyncCalls.length - 1, 'offline', false); } catch (error) { asyncFailureCaught = error; }
check(asyncFailureCaught === asyncError, 'deferred error listener exception propagates');
check(asyncFailedEvents.filter(function (e) { return e[1] === 'error'; }).length === 1 && asyncFailedEvents.filter(function (e) { return e[1] === 'loadend'; }).length === 0, 'deferred failure does not redispatch error or synthesize loadend');

var deferredReopen = new XMLHttpRequest(), deferredEvents = [], deferredError = {};
observe('deferred-reopen', deferredReopen, deferredEvents);
deferredReopen.addEventListener('readystatechange', function () {
  if (deferredReopen.readyState === 2 && deferredReopen.getResponseHeader('x-reply') === 'deferred-A') {
    deferredReopen.open('GET', '/deferred-b');
    deferredReopen.send();
    throw deferredError;
  }
});
deferredReopen.open('GET', '/deferred-a'); deferredReopen.send();
var deferredCaught;
try { deliver(asyncCalls.length - 1, 'deferred-A'); } catch (error) { deferredCaught = error; }
check(deferredCaught === deferredError && deferredReopen.readyState === 1 && deferredReopen._sent, 'superseded deferred exception propagates and leaves B active');
check(deferredEvents.every(function (e) { return e[1] !== 'error' && e[1] !== 'loadend'; }), 'superseded deferred exception emits no terminal events for A');
deliver(asyncCalls.length - 1, 'deferred-B');
check(deferredReopen.readyState === 4 && deferredReopen.responseText === 'deferred-B', 'B completes after superseded deferred exception');

var ordinaryAbortEvents = [], q = new XMLHttpRequest(); observe('ordinary-abort', q, ordinaryAbortEvents);
q.open('GET', '/ordinary-abort'); q.send(); q.abort();
check(ordinaryAbortEvents.filter(function (e) { return ['abort', 'loadend'].indexOf(e[1]) >= 0; }).map(function (e) { return e[1]; }).join(',') === 'abort,loadend', 'ordinary abort delivers abort then loadend');

var hostException = {}, hostMessageReads = 0;
Object.defineProperty(hostException, 'message', { get: function () { hostMessageReads++; throw new Error('host message getter must not run'); } });
globalThis.__chuzzNetAsync = globalThis.__chuzzNetSync = function () { throw hostException; };
[false, true].forEach(function (async) {
  var xhr = new XMLHttpRequest(), events = [], seenError;
  observe('host-throw', xhr, events);
  xhr.addEventListener('error', function (event) { seenError = event.error; });
  xhr.open('GET', '/host-throw', async); xhr.send();
  check(xhr.readyState === 1 && !xhr._sent && seenError === hostException, 'host invocation throw releases send and forwards the original value');
  check(events.map(function (e) { return e[1]; }).join(',') === 'readystatechange,loadstart,error,loadend', 'host invocation throw keeps its terminal sequence');
});
check(hostMessageReads === 0, 'host invocation throw does not inspect thrown value properties');

var asyncRetry = new XMLHttpRequest(), asyncRetryEvents = [], asyncRetryCalls = 0, asyncRetryStarted = false, asyncRetryBoundary = -1;
observe('host-async-retry', asyncRetry, asyncRetryEvents);
asyncRetry.addEventListener('error', function () { if (!asyncRetryStarted) { asyncRetryStarted = true; asyncRetry.send(); asyncRetryBoundary = asyncRetryEvents.length; } });
globalThis.__chuzzNetAsync = function (request, callback) {
  if (asyncRetryCalls++ === 0) throw hostException;
  asyncCalls.push(request.url); pendingReplies.push({ request: request, callback: callback });
};
asyncRetry.open('GET', '/host-async-retry'); asyncRetry.send();
check(asyncRetryCalls === 2 && asyncCalls[asyncCalls.length - 1].indexOf('/host-async-retry') >= 0, 'async host throw error handler accepts same-open retry');
check(asyncRetryEvents.length === asyncRetryBoundary && asyncRetryEvents.filter(function (e) { return e[1] === 'loadend'; }).length === 0, 'superseded async host throw emits no A loadend while B is pending');
deliver(pendingReplies.length - 1, 'async-retry-B');
check(asyncRetry.responseText === 'async-retry-B' && asyncRetryEvents.filter(function (e) { return e[1] === 'loadend'; }).length === 1, 'async retry B completes with exactly its own loadend');

var syncRetry = new XMLHttpRequest(), syncRetryEvents = [], syncRetryCalls = 0, syncRetryStarted = false;
observe('host-sync-retry', syncRetry, syncRetryEvents);
syncRetry.addEventListener('error', function () { if (!syncRetryStarted) { syncRetryStarted = true; syncRetry.send(); } });
globalThis.__chuzzNetSync = function (request) {
  if (syncRetryCalls++ === 0) throw hostException;
  return reply('sync-retry-B');
};
syncRetry.open('GET', '/host-sync-retry', false); syncRetry.send();
check(syncRetryCalls === 2 && syncRetry.readyState === 4 && syncRetry.responseText === 'sync-retry-B', 'sync host throw error handler completes same-open retry inline');
check(syncRetryEvents.filter(function (e) { return e[1] === 'loadend'; }).length === 1, 'inline sync retry receives exactly one loadend without stale A loadend');

globalThis.__chuzzNetAsync = nativeAsync; globalThis.__chuzzNetSync = nativeSync;
var live = new XMLHttpRequest(), liveDone = false;
live.addEventListener('loadend', function () { liveDone = true; });
live.open('GET', 'data:text/plain,ordinary-async'); live.send();
"#).expect("evaluate native XHR reentrancy witnesses");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            page.poll(None);
            if page.eval_json("liveDone").unwrap() == json!(true) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(
            page.eval_json("liveDone").unwrap(),
            json!(true),
            "native async XHR must finish within bounded polling"
        );
        assert_eq!(
            page.eval_json("live.responseText").unwrap(),
            json!("ordinary-async")
        );
        let diagnostics = page.diagnostic_batch(0, 0, 100).unwrap();
        assert!(diagnostics.errors.entries.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn page_ekojit_network_and_crypto_use_live_page_realm() {
        let mut page = page();
        page.eval(r#"var stage = 'headers'; try {
var headers = new Headers([['X-Empty', ''], ['x-empty', 'tail'], ['X-Other', 'value']]);
if (headers.get('x-empty') !== ', tail' || headers.get('x-other') !== 'value') throw new Error('header pairs');
stage = 'response'; var response = new Response('payload', { headers: headers });
response.headers.set('x-other', 'changed');
var cloned = response.clone();
response.headers.set('x-other', 'original-only');
if (cloned.headers.get('x-other') !== 'changed') throw new Error('clone current headers independently');
var received = [];
response.text().then(function (text) { received.push(text); });
cloned.blob().then(function (blob) { return blob.text(); }).then(function (text) { received.push(text); });
var cloneRejected = false;
try { response.clone(); } catch (error) { cloneRejected = true; }
if (!cloneRejected) throw new Error('consumed response clone');
stage = 'random'; var backing = new Uint8Array(12);
backing[0] = 91; backing[11] = 93;
var view = new Uint8Array(backing.buffer, 2, 8);
if (crypto.getRandomValues(view) !== view || backing[0] !== 91 || backing[11] !== 93) throw new Error('random view boundaries');
stage = 'uuid'; var uuid = crypto.randomUUID();
if (uuid.length !== 36 || uuid.charAt(14) !== '4' || '89ab'.indexOf(uuid.charAt(19)) < 0) throw new Error('UUID format');
stage = 'view-type'; var wrongType = false;
try { crypto.getRandomValues(new DataView(backing.buffer)); } catch (error) { wrongType = error.name === 'TypeMismatchError'; }
if (!wrongType) throw new Error('random view type');
stage = 'digest'; crypto.subtle.digest('SHA-256', new Uint8Array(0)).then(function (buffer) {
  var data = new Uint8Array(buffer);
  if (!(buffer instanceof ArrayBuffer) || data.length !== 32 || data[0] !== 227 || data[31] !== 85) throw new Error('digest bytes');
  received.push('digest');
});
stage = 'fetch'; fetch('data:text/plain,network').then(function (reply) { return reply.text(); }).then(function (text) { received.push(text); });
stage = 'xhr'; var xhr = new XMLHttpRequest();
xhr.addEventListener('load', null);
xhr.open('GET', 'data:text/plain,old');
xhr.send();
xhr.abort();
xhr.open('GET', 'data:text/plain,new', false);
xhr.send();
if (xhr.status !== 200 || xhr.responseText !== 'new') throw new Error('synchronous XHR');
stage = 'session'; sessionStorage.setItem('private', 'kept-live');
sessionStorage.markSensitive('private');
if (sessionStorage.getItem('private') !== 'kept-live') throw new Error('sensitive live value');
} catch (error) { throw stage + ': ' + String(error.name) + ': ' + String(error.message) + ': ' + String(error); }
"#).expect("evaluate page APIs");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            page.poll(None);
            if page.eval_json("received.length").unwrap() == json!(4) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(
            page.eval_json("received.sort()").unwrap(),
            json!(["digest", "network", "payload", "payload"])
        );
        assert_eq!(page.eval_json("xhr.responseText").unwrap(), json!("new"));
        let snapshot = page
            .capture_session(&crate::tab_stash::SleepingBudget::new(64 * 1024))
            .unwrap()
            .expect("capture the live session without sensitive values");
        assert!(
            snapshot.pairs().is_empty(),
            "sensitive key leaked into the snapshot"
        );
    }
}
