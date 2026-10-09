// JavaScript syntax is intentional: Boa evaluates this source directly.
(function () {
  var serial = 0;
  var urlSerial = 0;
  var token = {};
  var urls = new Map();
  var attached = new WeakMap();
  var objects = new WeakMap();
  var prototype = Object.getPrototypeOf(document.createElement('video'));
  var createURL = URL.createObjectURL;
  var revokeURL = URL.revokeObjectURL;

  function fail(message, name) {
    throw new DOMException(message, name || 'InvalidStateError');
  }

  function call(source, op, fields) {
    var request = { source: source._id, op: op };
    if (fields) Object.keys(fields).forEach(function (key) { request[key] = fields[key]; });
    var reply = JSON.parse(globalThis.__chuzzMseControl(JSON.stringify(request)));
    if (!reply.ok) fail(reply.error.message, reply.error.name);
    return reply.value;
  }

  function Target() { this._listeners = new Map(); }
  Target.prototype.addEventListener = function (name, callback, options) {
    if (!callback) return;
    name = String(name);
    var listeners = this._listeners.get(name) || [];
    if (!listeners.some(function (entry) { return entry.callback === callback; })) {
      listeners.push({ callback: callback, once: Boolean(options && options.once) });
      this._listeners.set(name, listeners);
    }
  };
  Target.prototype.removeEventListener = function (name, callback) {
    var listeners = this._listeners.get(String(name));
    if (listeners) this._listeners.set(String(name), listeners.filter(function (entry) {
      return entry.callback !== callback;
    }));
  };
  Target.prototype.dispatchEvent = function (event) {
    var self = this;
    try {
      Object.defineProperty(event, 'target', { configurable: true, value: self });
      Object.defineProperty(event, 'currentTarget', { configurable: true, value: self });
    } catch (error) {}
    (this._listeners.get(event.type) || []).slice().forEach(function (entry) {
      if (entry.once) self.removeEventListener(event.type, entry.callback);
      try {
        if (typeof entry.callback === 'function') entry.callback.call(self, event);
        else entry.callback.handleEvent(event);
      } catch (error) {}
    });
    var handler = this['on' + event.type];
    if (typeof handler === 'function') {
      try { handler.call(this, event); } catch (error) {}
    }
    return !event.defaultPrevented;
  };
  function emit(target, name) { target.dispatchEvent(new Event(name)); }
  function later(target, name) { setTimeout(function () { emit(target, name); }, 0); }
  function inherit(child) {
    child.prototype = Object.create(Target.prototype);
    child.prototype.constructor = child;
  }

  function ranges(values) {
    values = (values || []).map(function (range) { return range.slice(); });
    function at(index, side) {
      index = Number(index);
      if (!Number.isInteger(index) || index < 0 || index >= values.length) {
        fail('TimeRanges index is out of bounds', 'IndexSizeError');
      }
      return values[index][side];
    }
    return Object.freeze({
      length: values.length,
      start: function (index) { return at(index, 0); },
      end: function (index) { return at(index, 1); }
    });
  }

  function supported(type) {
    if (!globalThis.__chuzzVideoSupported) return false;
    var fields = String(type).toLowerCase().split(';');
    var container = fields.shift().trim();
    if (container !== 'video/mp4' && container !== 'audio/mp4') return false;
    if (!fields.length) return true;
    if (fields.length !== 1) return false;
    var match = /^\s*codecs\s*=\s*(?:"([^"]+)"|([^"\s]+))\s*$/.exec(fields[0]);
    if (!match) return false;
    var values = (match[1] || match[2]).split(',');
    var video = false;
    var audio = false;
    for (var index = 0; index < values.length; index++) {
      var codec = values[index].trim();
      if (codec === 'mp4a.40.2' && !audio) {
        audio = true;
      } else if (/^avc1\.(42|4d|64)[0-9a-f]{4}$/.test(codec)
          && container !== 'audio/mp4' && !video) {
        video = true;
      } else {
        return false;
      }
    }
    return video || audio;
  }
  globalThis.__chuzzMseSupported = supported;

  function SourceBufferList(key) {
    if (key !== token) throw new TypeError('Illegal constructor');
    Target.call(this);
    this._items = [];
  }
  inherit(SourceBufferList);
  Object.defineProperty(SourceBufferList.prototype, 'length', {
    get: function () { return this._items.length; }
  });
  SourceBufferList.prototype.item = function (index) { return this._items[index] || null; };
  SourceBufferList.prototype[Symbol.iterator] = function () {
    return this._items[Symbol.iterator]();
  };
  SourceBufferList.prototype._set = function (items, event) {
    for (var index = 0; index < this._items.length; index++) delete this[index];
    this._items = items.slice();
    var self = this;
    items.forEach(function (item, index) {
      Object.defineProperty(self, index, { configurable: true, enumerable: true, value: item });
    });
    if (event) later(this, event);
  };

  function MediaSource() {
    Target.call(this);
    this._id = ++serial;
    this._state = 'closed';
    this._element = null;
    this._epoch = 0;
    this._buffers = new SourceBufferList(token);
    this._active = new SourceBufferList(token);
    this._direct = 'blob:chuzz-mse/source-' + this._id;
  }
  inherit(MediaSource);
  MediaSource.isTypeSupported = supported;
  Object.defineProperties(MediaSource.prototype, {
    readyState: { get: function () { return this._state; } },
    sourceBuffers: { get: function () { return this._buffers; } },
    activeSourceBuffers: { get: function () { return this._active; } },
    duration: {
      get: function () {
        if (this._state === 'closed') return NaN;
        var info = call(this, 'info');
        return info.durationKind === 'infinite' ? Infinity
          : (info.durationKind === 'unknown' ? NaN : info.duration);
      },
      set: function (value) {
        this._idle();
        value = Number(value);
        if (Number.isNaN(value) || value < 0) throw new TypeError('Invalid duration');
        call(this, 'duration', { value: Number.isFinite(value) ? value : null });
      }
    }
  });
  MediaSource.prototype._idle = function () {
    if (this._state !== 'open'
        || this._buffers._items.some(function (buffer) { return buffer.updating; })) {
      fail('MediaSource must be open with idle SourceBuffers');
    }
  };
  MediaSource.prototype._reopen = function () {
    if (this._state === 'closed') fail('MediaSource is closed');
    if (this._state === 'ended') {
      call(this, 'reopen');
      this._state = 'open';
      later(this, 'sourceopen');
    }
  };
  MediaSource.prototype.addSourceBuffer = function (type) {
    if (this._state !== 'open') fail('MediaSource is closed or ended');
    type = String(type);
    if (!supported(type)) fail('Unsupported MSE content type', 'NotSupportedError');
    var buffer = new SourceBuffer(token, this, ++serial);
    call(this, 'add', { buffer: buffer._id, mime: type });
    this._buffers._set(this._buffers._items.concat([buffer]), 'addsourcebuffer');
    this._active._set(this._buffers._items, 'addsourcebuffer');
    return buffer;
  };
  MediaSource.prototype.removeSourceBuffer = function (buffer) {
    if (this._buffers._items.indexOf(buffer) < 0) fail('SourceBuffer is absent', 'NotFoundError');
    buffer._cancel();
    call(this, 'delete', { buffer: buffer._id });
    buffer._removed = true;
    var items = this._buffers._items.filter(function (item) { return item !== buffer; });
    this._buffers._set(items, 'removesourcebuffer');
    this._active._set(items, 'removesourcebuffer');
  };
  MediaSource.prototype.endOfStream = function (error) {
    this._idle();
    if (error !== undefined && error !== 'network' && error !== 'decode') {
      throw new TypeError('Invalid endOfStream error');
    }
    call(this, 'end', { error: error || '' });
    this._state = 'ended';
    later(this, 'sourceended');
  };
  MediaSource.prototype.setLiveSeekableRange = function (start, end) {
    if (this._state !== 'open') fail('MediaSource is not open');
    start = Number(start);
    end = Number(end);
    if (!Number.isFinite(start) || !Number.isFinite(end) || start < 0 || end < start) {
      throw new TypeError('Invalid live seekable range');
    }
    call(this, 'live', { start: start, end: end });
  };
  MediaSource.prototype.clearLiveSeekableRange = function () {
    if (this._state !== 'open') fail('MediaSource is not open');
    call(this, 'clearLive');
  };

  function SourceBuffer(key, source, id) {
    if (key !== token) throw new TypeError('Illegal constructor');
    Target.call(this);
    this._source = source;
    this._id = id;
    this._removed = false;
    this._updating = false;
    this._removing = false;
    this._operation = 0;
    this._handle = null;
    this._offset = 0;
    this._windowStart = 0;
    this._windowEnd = Infinity;
    this._mode = 'segments';
  }
  inherit(SourceBuffer);
  SourceBuffer.prototype._idle = function () {
    if (this._removed || this._source._state === 'closed' || this._updating) {
      fail('SourceBuffer is removed, closed or updating');
    }
  };
  SourceBuffer.prototype._cancel = function () {
    this._operation++;
    if (this._handle !== null) {
      globalThis.__chuzzMseRelease(this._handle);
      this._handle = null;
    }
    if (this._updating) {
      this._updating = false;
      this._removing = false;
      later(this, 'abort');
      later(this, 'updateend');
    }
  };
  SourceBuffer.prototype._run = function (op, fields) {
    var self = this;
    this._updating = true;
    this._removing = op === 'remove';
    var operation = ++this._operation;
    setTimeout(function () {
      if (operation !== self._operation) return;
      emit(self, 'updatestart');
      if (operation !== self._operation) return;
      var event = 'update';
      try {
        var result = call(self._source, op, fields);
        self._handle = null;
        if (result.timestampOffset !== null && result.timestampOffset !== undefined) {
          self._offset = result.timestampOffset;
        }
      } catch (error) {
        if (self._handle !== null) globalThis.__chuzzMseRelease(self._handle);
        self._handle = null;
        event = 'error';
      }
      if (operation !== self._operation) return;
      self._updating = false;
      self._removing = false;
      emit(self, event);
      if (operation === self._operation) emit(self, 'updateend');
    }, 0);
  };
  SourceBuffer.prototype.appendBuffer = function (value) {
    this._idle();
    var buffer;
    var offset;
    var length;
    if (value instanceof ArrayBuffer) {
      buffer = value;
      offset = 0;
      length = value.byteLength;
    } else if (ArrayBuffer.isView(value)) {
      buffer = value.buffer;
      offset = value.byteOffset;
      length = value.byteLength;
    } else {
      throw new TypeError('appendBuffer requires an ArrayBuffer or view');
    }
    this._source._reopen();
    try {
      this._handle = globalThis.__chuzzMsePut(buffer, offset, length);
    } catch (error) {
      if (String(error.message).indexOf('quota') >= 0) {
        fail(error.message, 'QuotaExceededError');
      }
      throw error;
    }
    this._run('append', {
      buffer: this._id,
      handle: this._handle,
      timestampOffset: this._offset,
      appendWindowStart: this._windowStart,
      appendWindowEnd: Number.isFinite(this._windowEnd) ? this._windowEnd : null,
      sequence: this._mode === 'sequence'
    });
  };
  SourceBuffer.prototype.abort = function () {
    if (this._removed || this._source._state !== 'open' || this._removing) {
      fail('SourceBuffer cannot be aborted');
    }
    this._cancel();
    call(this._source, 'abort', { buffer: this._id });
    this._windowStart = 0;
    this._windowEnd = Infinity;
  };
  SourceBuffer.prototype.remove = function (start, end) {
    this._idle();
    start = Number(start);
    end = Number(end);
    if (!Number.isFinite(start) || Number.isNaN(end) || start < 0 || end <= start
        || Number.isNaN(this._source.duration) || start > this._source.duration) {
      throw new TypeError('Invalid SourceBuffer removal interval');
    }
    this._source._reopen();
    this._run('remove', { buffer: this._id, start: start, end: Number.isFinite(end) ? end : null });
  };
  Object.defineProperties(SourceBuffer.prototype, {
    updating: { get: function () { return this._updating; } },
    buffered: {
      get: function () {
        if (this._removed || this._source._state === 'closed') fail('SourceBuffer was removed');
        return ranges(call(this._source, 'info', { buffer: this._id }).buffered);
      }
    },
    timestampOffset: {
      get: function () { return this._offset; },
      set: function (value) {
        this._idle();
        value = Number(value);
        if (!Number.isFinite(value)) throw new TypeError('Invalid timestampOffset');
        this._source._reopen();
        this._offset = value;
      }
    },
    appendWindowStart: {
      get: function () { return this._windowStart; },
      set: function (value) {
        this._idle();
        value = Number(value);
        if (!Number.isFinite(value) || value < 0 || value >= this._windowEnd) {
          throw new TypeError('Invalid appendWindowStart');
        }
        this._windowStart = value;
      }
    },
    appendWindowEnd: {
      get: function () { return this._windowEnd; },
      set: function (value) {
        this._idle();
        value = Number(value);
        if (Number.isNaN(value) || value <= this._windowStart) {
          throw new TypeError('Invalid appendWindowEnd');
        }
        this._windowEnd = value;
      }
    },
    mode: {
      get: function () { return this._mode; },
      set: function (value) {
        this._idle();
        value = String(value);
        if (value !== 'segments' && value !== 'sequence') throw new TypeError('Invalid mode');
        this._source._reopen();
        if (value === 'sequence' && this._mode !== value) {
          var buffered = this.buffered;
          this._offset = buffered.length ? buffered.end(buffered.length - 1) : this._offset;
        }
        this._mode = value;
      }
    }
  });

  function detach(element) {
    var source = attached.get(element);
    if (!source) return;
    attached.delete(element);
    source._buffers._items.forEach(function (buffer) {
      buffer._cancel();
      buffer._removed = true;
    });
    source._buffers._set([], 'removesourcebuffer');
    source._active._set([], 'removesourcebuffer');
    call(source, 'close');
    source._state = 'closed';
    source._element = null;
    source._epoch++;
    later(source, 'sourceclose');
  }
  globalThis.__chuzzMseDetach = detach;
  globalThis.__chuzzMseSource = function (element) {
    var source = objects.get(element);
    return source ? source._direct : null;
  };
  globalThis.__chuzzMseAttach = function (element, url, force) {
    var source = objects.get(element) || urls.get(url);
    var previous = attached.get(element);
    if (previous === source && source && !force) return source._id;
    detach(element);
    if (!source || source._element && source._element !== element) return null;
    call(source, 'open');
    source._element = element;
    source._state = 'open';
    attached.set(element, source);
    var epoch = ++source._epoch;
    setTimeout(function () {
      if (source._epoch === epoch && source._state === 'open') emit(source, 'sourceopen');
    }, 0);
    return source._id;
  };
  Object.defineProperty(prototype, 'srcObject', {
    configurable: true,
    enumerable: true,
    get: function () { return objects.get(this) || null; },
    set: function (value) {
      if (value !== null && !(value instanceof MediaSource)) {
        throw new TypeError('Only MediaSource srcObject is supported');
      }
      if (value && value._element && value._element !== this) fail('MediaSource is already attached');
      if (value) objects.set(this, value);
      else objects.delete(this);
      this.load();
    }
  });
  URL.createObjectURL = function (value) {
    if (!(value instanceof MediaSource)) {
      if (createURL) return createURL.call(URL, value);
      throw new TypeError('Unsupported object URL source');
    }
    var url = 'blob:' + location.origin + '/chuzz-mse/' + (++urlSerial);
    urls.set(url, value);
    return url;
  };
  URL.revokeObjectURL = function (url) {
    url = String(url);
    if (urls.delete(url)) return;
    if (revokeURL) revokeURL.call(URL, url);
  };
  globalThis.MediaSource = MediaSource;
  globalThis.SourceBuffer = SourceBuffer;
  globalThis.SourceBufferList = SourceBufferList;
})();
