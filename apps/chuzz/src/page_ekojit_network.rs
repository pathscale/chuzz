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
      var request;
      try { request = requestRecord(input, init); } catch (error) { return Promise.reject(error); }
      return new Promise(function (resolve, reject) {
        try { __chuzzNetAsync(request, function (reply) { try { resolve(makeResponse(reply)); } catch (error) { reject(error); } }); }
        catch (error) { reject(error); }
      });
    };
  }
  function XMLHttpRequest() {
    if (!(this instanceof XMLHttpRequest)) throw new TypeError("XMLHttpRequest must be constructed");
    eventTarget(this); this.readyState = 0; this.responseType = ''; this.response = null; this.responseText = '';
    this.responseURL = ''; this.status = 0; this.statusText = ''; this.timeout = 0; this.withCredentials = false;
    this._headers = new Headers(); this._responseHeaders = new Headers(); this._async = true; this._sent = false; this._sequence = 0;
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
    var self = this; if (this.readyState !== 1 || this._sent) throw new TypeError('XHR is not opened or is already sending');
    this._sent = true;
    var sequence = this._sequence;
    var request = { url: this._url, method: this._method, headers: [], body: body == null ? null : bytesOf(body), credentials: this.withCredentials ? 'include' : 'same-origin' };
    this._headers.forEach(function (value, name) { request.headers.push([name, value]); }); this.dispatchEvent({ type: 'loadstart' });
    function finish(reply) {
      if (self._sequence !== sequence) return;
      self._sent = false;
      if (!reply || !reply.ok) { self.readyState = 4; self.dispatchEvent({ type: 'readystatechange' }); self.dispatchEvent({ type: 'error', message: reply && reply.error }); self.dispatchEvent({ type: 'loadend' }); return; }
      self.status = reply.status; self.statusText = reply.statusText || ''; self.responseURL = reply.responseURL || reply.url || '';
      self._responseHeaders = new Headers(reply.headers || []); self.readyState = 2; self.dispatchEvent({ type: 'readystatechange' });
      self.readyState = 3; self.dispatchEvent({ type: 'readystatechange' }); self.dispatchEvent({ type: 'progress', loaded: (reply.bytes || []).length, lengthComputable: false });
      var text = __chuzzDecodeUtf8(bytesOf(reply.bytes)); self.responseText = text;
      if (self.responseType === 'document') { var error = new Error('XHR responseType document is unsupported'); error.name = 'NotSupportedError'; self.response = null; self.dispatchEvent({ type: 'error', error: error }); }
      else if (self.responseType === 'json') { try { self.response = JSON.parse(text); } catch (_) { self.response = null; } }
      else if (self.responseType === 'arraybuffer') self.response = bytesOf(reply.bytes).buffer;
      else if (self.responseType === 'blob') self.response = new Blob([bytesOf(reply.bytes)], { type: self._responseHeaders.get('content-type') || '' });
      else self.response = text;
      self.readyState = 4; self.dispatchEvent({ type: 'readystatechange' }); self.dispatchEvent({ type: 'load' }); self.dispatchEvent({ type: 'loadend' });
    }
    try { if (this._async) __chuzzNetAsync(request, finish); else finish(__chuzzNetSync(request)); }
    catch (error) { this.dispatchEvent({ type: 'error', error: error }); this.dispatchEvent({ type: 'loadend' }); }
  };
  XMLHttpRequest.prototype.abort = function () { this._sequence += 1; var active = this._sent; this._sent = false; this.status = 0; this.statusText = ''; this.response = null; this.responseText = ''; this.responseURL = ''; if (active) { this.readyState = 4; this.dispatchEvent({ type: 'readystatechange' }); this.dispatchEvent({ type: 'abort' }); this.dispatchEvent({ type: 'loadend' }); } this.readyState = 0; };
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
