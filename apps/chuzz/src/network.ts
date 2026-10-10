// Directly executable JavaScript syntax, included by the Rust page host.
(function () {
  var pending = Object.create(null);
  var nextId = 1;

  function exception(name, message) {
    return new DOMException(message, name);
  }

  function report(error) {
    if (globalThis.console) console.error(error);
  }

  globalThis.__chuzzNetResolve = function (id, result) {
    var callback = pending[id];
    if (!callback) return;
    delete pending[id];
    callback(result);
  };

  Object.defineProperty(document, 'cookie', {
    configurable: true,
    enumerable: true,
    get: function () { return __chuzzCookieGet(); },
    set: function (value) { __chuzzCookieSet(String(value)); }
  });

  var HeaderBag = globalThis.Headers;

  function request(url, options) {
    return globalThis.__chuzzBodyApi.payloadSync(url, options);
  }

  function send(payload, callback) {
    var id = nextId++;
    payload.id = id;
    pending[id] = callback;
    try {
      window.ipc.postMessage(JSON.stringify(payload));
    } catch (error) {
      delete pending[id];
      callback({ ok: false, error: String(error) });
    }
    return id;
  }

  function forget(id) { delete pending[id]; }

  globalThis.__chuzzBodyApi.installFetch(send, forget);

  function EventTarget() {
    this._listeners = Object.create(null);
  }

  EventTarget.prototype.addEventListener = function (type, listener, options) {
    if (!listener) return;
    type = String(type);
    var list = this._listeners[type] || (this._listeners[type] = []);
    var capture = typeof options === 'boolean' ? options : !!(options && options.capture);
    if (list.some(function (entry) {
      return entry.listener === listener && entry.capture === capture;
    })) return;
    list.push({ listener: listener, capture: capture, once: !!(options && options.once) });
  };
  EventTarget.prototype.removeEventListener = function (type, listener, options) {
    var capture = typeof options === 'boolean' ? options : !!(options && options.capture);
    var list = this._listeners[String(type)] || [];
    this._listeners[String(type)] = list.filter(function (entry) {
      return entry.listener !== listener || entry.capture !== capture;
    });
  };
  EventTarget.prototype.dispatchEvent = function (event) {
    if (!event || !event.type) throw new TypeError('An event is required');
    Object.defineProperties(event, {
      target: { configurable: true, value: this },
      currentTarget: { configurable: true, value: this }
    });
    var direct = this['on' + event.type];
    if (typeof direct === 'function') {
      try { direct.call(this, event); } catch (error) { report(error); }
    }
    var self = this;
    var list = (this._listeners[event.type] || []).slice();
    list.forEach(function (entry) {
      if ((self._listeners[event.type] || []).indexOf(entry) < 0) return;
      if (entry.once) self.removeEventListener(event.type, entry.listener, entry.capture);
      try {
        if (typeof entry.listener === 'function') entry.listener.call(self, event);
        else entry.listener.handleEvent(event);
      } catch (error) { report(error); }
    });
    Object.defineProperty(event, 'currentTarget', { configurable: true, value: null });
    return !event.defaultPrevented;
  };

  function fire(target, type, length) {
    var event = new Event(type);
    if (type !== 'readystatechange') {
      event.loaded = length || 0;
      event.total = length || 0;
      event.lengthComputable = length !== undefined;
    }
    target.dispatchEvent(event);
  }

  if (typeof globalThis.XMLHttpRequest !== 'function') {
    function XHR() {
      EventTarget.call(this);
      this.upload = new EventTarget();
      this.readyState = 0;
      this.status = 0;
      this.statusText = '';
      this.responseURL = '';
      this.withCredentials = false;
      this.timeout = 0;
      this._type = '';
      this._text = '';
      this._response = null;
      this._xml = null;
      this._async = true;
      this._sent = false;
      this._id = 0;
      this._timer = null;
      this._generation = 0;
      this._requestHeaders = new HeaderBag();
      this._responseHeaders = new HeaderBag();
      ['readystatechange', 'loadstart', 'progress', 'load', 'error', 'abort', 'timeout', 'loadend']
        .forEach(function (type) { this['on' + type] = null; }, this);
    }
    XHR.prototype = Object.create(EventTarget.prototype);
    Object.defineProperty(XHR.prototype, 'constructor', { value: XHR, writable: true });

    Object.defineProperties(XHR.prototype, {
      responseType: {
        get: function () { return this._type; },
        set: function (value) {
          value = String(value);
          if (!['', 'text', 'json', 'arraybuffer', 'blob', 'document'].includes(value)) return;
          if (this.readyState >= 3) throw exception('InvalidStateError', 'Response is loading');
          if (!this._async && value !== '') {
            throw exception('InvalidAccessError', 'Synchronous window XHR requires text');
          }
          this._type = value;
        }
      },
      responseText: {
        get: function () {
          if (this._type !== '' && this._type !== 'text') {
            throw exception('InvalidStateError', 'Response is not text');
          }
          return this.readyState >= 3 ? this._text : '';
        }
      },
      response: {
        get: function () {
          if (this._type === '' || this._type === 'text') {
            return this.readyState >= 3 ? this._text : '';
          }
          return this.readyState === 4 ? this._response : null;
        }
      },
      responseXML: {
        get: function () {
          if (this._type !== '' && this._type !== 'document') {
            throw exception('InvalidStateError', 'Response is not a document');
          }
          return this.readyState === 4 ? this._xml : null;
        }
      }
    });

    XHR.prototype._clear = function () {
      if (this._id) forget(this._id);
      if (this._timer !== null) clearTimeout(this._timer);
      this._id = 0;
      this._timer = null;
      this._sent = false;
    };

    XHR.prototype.open = function (method, url, async) {
      var payload = request(url, { method: method });
      var resolved = new URL(payload.url, location.href);
      this._clear();
      this._generation++;
      this._method = payload.method;
      this._url = resolved.href;
      this._async = async !== false;
      if (!this._async && (this.timeout !== 0 || this._type !== '')) {
        throw exception('InvalidAccessError', 'Invalid synchronous window XHR options');
      }
      this.status = 0;
      this.statusText = '';
      this.responseURL = '';
      this._text = '';
      this._response = null;
      this._xml = null;
      this._mime = null;
      this._requestHeaders = new HeaderBag();
      this._responseHeaders = new HeaderBag();
      this.readyState = 1;
      fire(this, 'readystatechange');
    };

    XHR.prototype.setRequestHeader = function (name, value) {
      if (this.readyState !== 1 || this._sent) {
        throw exception('InvalidStateError', 'XHR is not opened');
      }
      this._requestHeaders.append(name, value);
    };
    XHR.prototype.getResponseHeader = function (name) {
      return this.readyState < 2 ? null : this._responseHeaders.get(name);
    };
    XHR.prototype.getAllResponseHeaders = function () {
      if (this.readyState < 2) return '';
      var text = '';
      this._responseHeaders.forEach(function (value, name) { text += name + ': ' + value + '\r\n'; });
      return text;
    };
    XHR.prototype.overrideMimeType = function (type) {
      if (this.readyState >= 3) throw exception('InvalidStateError', 'Response is loading');
      this._mime = String(type);
    };

    XHR.prototype._fail = function (type) {
      if (!this._sent) return;
      this._clear();
      this.status = 0;
      this.statusText = '';
      this.responseURL = '';
      this._text = '';
      this._response = null;
      this._xml = null;
      this._responseHeaders = new HeaderBag();
      this.readyState = 4;
      fire(this, 'readystatechange');
      fire(this.upload, type);
      fire(this.upload, 'loadend');
      fire(this, type);
      fire(this, 'loadend');
    };
    XHR.prototype.abort = function () {
      this._generation++;
      this._fail('abort');
      this._clear();
      this.readyState = 0;
      this.status = 0;
      this.statusText = '';
      this.responseURL = '';
      this._text = '';
      this._response = null;
      this._xml = null;
      this._responseHeaders = new HeaderBag();
    };

    XHR.prototype.send = function (body) {
      if (this.readyState !== 1 || this._sent) {
        throw exception('InvalidStateError', 'XHR is not opened');
      }
      if (!this._async && this.timeout !== 0) {
        throw exception('InvalidAccessError', 'Synchronous window XHR cannot time out');
      }
      var self = this;
      var generation = this._generation;
      var payload = request(this._url, {
        method: this._method,
        headers: Array.from(this._requestHeaders),
        body: body,
        credentials: this.withCredentials ? 'include' : 'same-origin'
      });
      this._sent = true;
      function current() { return self._generation === generation && self._sent; }
      function complete(result) {
        if (!current()) return;
        if (!result.ok) {
          if (!self._async) {
            self._clear();
            self.readyState = 4;
            throw exception('NetworkError', result.error || 'Network request failed');
          }
          self._fail('error');
          return;
        }
        self.status = result.status;
        self.statusText = result.statusText || '';
        self.responseURL = result.url || '';
        self._responseHeaders = new HeaderBag(result.headers);
        var bytes = new Uint8Array(result.bytes);
        if (self._async) {
          self.readyState = 2;
          fire(self, 'readystatechange');
          if (!current()) return;
          self.readyState = 3;
          self._text = result.body || '';
          fire(self, 'readystatechange');
          if (!current()) return;
          fire(self, 'progress', bytes.length);
          if (!current()) return;
        }
        self._text = result.body || '';
        if (self._type === 'json') {
          try { self._response = JSON.parse(self._text); }
          catch (error) { self._response = null; }
        } else if (self._type === 'arraybuffer') {
          self._response = bytes.buffer;
        } else if (self._type === 'blob') {
          self._response = new Blob([bytes], {
            type: self._mime || self._responseHeaders.get('content-type') || ''
          });
        }
        var mime = (self._mime || self._responseHeaders.get('content-type') || '')
          .split(';')[0].trim().toLowerCase();
        if (typeof DOMParser === 'function'
            && (mime === 'text/xml' || mime === 'application/xml' || mime.endsWith('+xml')
              || (self._type === 'document' && mime === 'text/html'))) {
          try {
            self._xml = new DOMParser().parseFromString(self._text,
              mime === 'text/html' ? 'text/html' : 'application/xml');
          } catch (error) { self._xml = null; }
        }
        if (self._type === 'document') self._response = self._xml;
        self._clear();
        self.readyState = 4;
        fire(self, 'readystatechange');
        if (self._generation !== generation || self.readyState !== 4) return;
        fire(self.upload, 'load', payload.body ? payload.body.length : 0);
        fire(self.upload, 'loadend', payload.body ? payload.body.length : 0);
        fire(self, 'load', bytes.length);
        if (self._generation === generation && self.readyState === 4) {
          fire(self, 'loadend', bytes.length);
        }
      }
      if (!this._async) {
        complete(__chuzzNetSync(JSON.stringify(payload)));
        return;
      }
      fire(this, 'loadstart');
      if (!current()) return;
      if (payload.body !== null) fire(this.upload, 'loadstart');
      if (!current()) return;
      var id = send(payload, complete);
      if (current()) this._id = id;
      if (current() && this.timeout > 0) {
        this._timer = setTimeout(function () {
          if (current()) self._fail('timeout');
        }, this.timeout);
      }
    };

    ['UNSENT', 'OPENED', 'HEADERS_RECEIVED', 'LOADING', 'DONE'].forEach(function (name, value) {
      Object.defineProperty(XHR, name, { value: value });
      Object.defineProperty(XHR.prototype, name, { value: value });
    });
    globalThis.XMLHttpRequest = XHR;
  }
})();

