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
  function bytesOf(body) {
    if (body == null) return new Uint8Array(0);
    if (typeof body === 'string') return __chuzzEncodeUtf8(body);
    if (body && body._bytes) return __chuzzCopyBytes(body._bytes);
    return __chuzzCopyBytes(body);
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
      return Promise.resolve(converter(data));
    }
    response.arrayBuffer = function () { return consume(function (source) { var copy = new Uint8Array(source.length); copy.set(source); return copy.buffer; }); };
    response.bytes = function () { return consume(function (source) { var copy = new Uint8Array(source.length); copy.set(source); return copy; }); };
    response.text = function () { return consume(function (source) { return __chuzzDecodeUtf8(source); }); };
    response.json = function () { return response.text().then(function (text) { return JSON.parse(text); }); };
    response.blob = function () { return response.arrayBuffer().then(function (buffer) { return new Blob([buffer], { type: response.headers.get('content-type') || '' }); }); };
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
    var entries = []; headers.forEach(function (value, name) { entries.push([name, value]); });
    var body = init.body === undefined ? input && input._body : init.body;
    return { url: url, method: String(init.method || (input && input.method) || 'GET').toUpperCase(), headers: entries, body: body == null ? null : bytesOf(body), credentials: init.credentials || (input && input.credentials) || 'same-origin' };
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
