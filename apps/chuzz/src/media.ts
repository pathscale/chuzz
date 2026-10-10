// JavaScript syntax is intentional: Boa evaluates this source directly.
(function (g) {
  'use strict';
  var secret = {};
  var trackState = new WeakMap();
  var streamState = new WeakMap();
  var deviceState = new WeakMap();
  var listeners = new WeakMap();
  var liveTracks = new Map();
  var devices = null;

  function copy(value) { return JSON.parse(JSON.stringify(value)); }
  function exception(name, message) {
    if (name === 'TypeError') return new TypeError(message);
    var error = typeof g.DOMException === 'function'
      ? new g.DOMException(message, name) : new Error(message);
    if (error.name !== name) error.name = name;
    return error;
  }
  function report(error) { setTimeout(function () { throw error; }, 0); }

  function Target() { listeners.set(this, new Map()); }
  if (typeof g.EventTarget === 'function') {
    Object.setPrototypeOf(Target.prototype, g.EventTarget.prototype);
  }
  Target.prototype.addEventListener = function (type, callback, options) {
    if (callback == null) return;
    type = String(type);
    var capture = typeof options === 'boolean' ? options : !!(options && options.capture);
    var signal = options && options.signal;
    if (signal && signal.aborted) return;
    var map = listeners.get(this);
    var entries = map.get(type) || [];
    if (entries.some(function (entry) {
      return entry.callback === callback && entry.capture === capture;
    })) return;
    var entry = { callback: callback, capture: capture, once: !!(options && options.once) };
    entries.push(entry);
    map.set(type, entries);
    if (signal) {
      var self = this;
      signal.addEventListener('abort', function () {
        self.removeEventListener(type, callback, capture);
      }, { once: true });
    }
  };
  Target.prototype.removeEventListener = function (type, callback, options) {
    var capture = typeof options === 'boolean' ? options : !!(options && options.capture);
    var map = listeners.get(this);
    var entries = map.get(String(type)) || [];
    map.set(String(type), entries.filter(function (entry) {
      return entry.callback !== callback || entry.capture !== capture;
    }));
  };
  Target.prototype.dispatchEvent = function (event) {
    if (!event || !event.type) throw new TypeError('dispatchEvent requires an event');
    var self = this;
    var stopped = false;
    var immediate = event.stopImmediatePropagation;
    try {
      Object.defineProperty(event, 'target', { configurable: true, value: self });
      Object.defineProperty(event, 'currentTarget', { configurable: true, value: self });
      Object.defineProperty(event, 'eventPhase', { configurable: true, value: 2 });
      event.stopImmediatePropagation = function () {
        stopped = true;
        if (typeof immediate === 'function') immediate.call(event);
      };
    } catch (error) {}
    var map = listeners.get(this);
    (map.get(event.type) || []).slice().forEach(function (entry) {
      if (stopped || !(map.get(event.type) || []).includes(entry)) return;
      if (entry.once) self.removeEventListener(event.type, entry.callback, entry.capture);
      try {
        if (typeof entry.callback === 'function') entry.callback.call(self, event);
        else if (typeof entry.callback.handleEvent === 'function') entry.callback.handleEvent(event);
      } catch (error) { report(error); }
    });
    var handler = this['on' + event.type];
    if (!stopped && typeof handler === 'function') {
      try { handler.call(this, event); } catch (error) { report(error); }
    }
    try {
      Object.defineProperty(event, 'currentTarget', { configurable: true, value: null });
      Object.defineProperty(event, 'eventPhase', { configurable: true, value: 0 });
      if (typeof immediate === 'function') event.stopImmediatePropagation = immediate;
    } catch (error) {}
    return !event.defaultPrevented;
  };
  function inherit(constructor) {
    constructor.prototype = Object.create(Target.prototype);
    constructor.prototype.constructor = constructor;
  }
  function later(target, type) {
    setTimeout(function () { target.dispatchEvent(new Event(type)); }, 0);
  }

  function update(updates) {
    if (!updates) return;
    (updates.tracks || []).forEach(function (descriptor) {
      var track = liveTracks.get(descriptor.handle);
      if (!track) return;
      var state = trackState.get(track);
      var previous = state.data;
      state.data = descriptor;
      if (previous.readyState === 'live' && descriptor.readyState === 'ended') {
        liveTracks.delete(descriptor.handle);
        later(track, 'ended');
      } else if (descriptor.readyState === 'live' && previous.muted !== descriptor.muted) {
        later(track, descriptor.muted ? 'mute' : 'unmute');
      }
    });
    if (updates.devicechange && devices) later(devices, 'devicechange');
  }
  Object.defineProperty(g, '__chuzzMediaUpdate', { value: update });

  function call(op, fields) {
    var request = { op: op };
    if (fields) Object.keys(fields).forEach(function (key) { request[key] = fields[key]; });
    var reply = JSON.parse(g.__chuzzMediaControl(JSON.stringify(request)));
    update(reply.updates);
    if (!reply.ok) {
      var error = exception(reply.error.name, reply.error.message);
      if (reply.error.constraint !== undefined) {
        Object.defineProperty(error, 'constraint', { value: reply.error.constraint, enumerable: true });
      }
      throw error;
    }
    return reply.value;
  }

  function stateOf(track) {
    var state = trackState.get(track);
    if (!state) throw new TypeError('expected a MediaStreamTrack');
    return state;
  }
  function attachAudio(track) {
    var state = stateOf(track);
    if (state.data.kind !== 'audio' || state.data.readyState !== 'live') return;
    if (typeof g.__chuzzAudioAttachTrack !== 'function') return;
    var id = call('audio', { handle: state.data.handle });
    g.__chuzzAudioAttachTrack(track, id);
  }
  Object.defineProperty(g, '__chuzzMediaAttachAudioTrack', { value: attachAudio });

  function MediaStreamTrack(token, descriptor) {
    if (token !== secret) throw new TypeError('Illegal constructor');
    Target.call(this);
    trackState.set(this, { data: descriptor });
    if (descriptor.readyState === 'live') liveTracks.set(descriptor.handle, this);
    this.onmute = null;
    this.onunmute = null;
    this.onended = null;
    try {
      attachAudio(this);
    } catch (error) {
      this.stop();
      throw error;
    }
  }
  inherit(MediaStreamTrack);
  ['id', 'kind', 'label', 'muted', 'readyState'].forEach(function (name) {
    Object.defineProperty(MediaStreamTrack.prototype, name, {
      enumerable: true,
      get: function () { return stateOf(this).data[name]; }
    });
  });
  Object.defineProperty(MediaStreamTrack.prototype, 'enabled', {
    enumerable: true,
    get: function () { return stateOf(this).data.enabled; },
    set: function (value) {
      var state = stateOf(this);
      value = Boolean(value);
      if (state.data.readyState === 'live') {
        call('enabled', { handle: state.data.handle, value: value });
      }
      state.data.enabled = value;
    }
  });
  MediaStreamTrack.prototype.stop = function () {
    var state = stateOf(this);
    if (state.data.readyState === 'ended') return;
    var handle = state.data.handle;
    // Explicit stop changes state without dispatching ended.
    state.data.readyState = 'ended';
    liveTracks.delete(handle);
    call('stop', { handle: handle });
  };
  MediaStreamTrack.prototype.clone = function () {
    var state = stateOf(this);
    var descriptor;
    if (state.data.readyState === 'ended') {
      descriptor = copy(state.data);
      descriptor.id = call('id');
      descriptor.handle = null;
    } else {
      descriptor = call('clone', { handle: state.data.handle });
    }
    return new MediaStreamTrack(secret, descriptor);
  };
  MediaStreamTrack.prototype.getConstraints = function () {
    return copy(stateOf(this).data.constraints);
  };
  MediaStreamTrack.prototype.getCapabilities = function () {
    return copy(stateOf(this).data.capabilities);
  };
  MediaStreamTrack.prototype.getSettings = function () {
    var state = stateOf(this);
    if (state.data.readyState === 'live') {
      state.data = call('describe', { handle: state.data.handle });
    }
    return copy(state.data.settings);
  };
  MediaStreamTrack.prototype.applyConstraints = function (constraints) {
    var self = this;
    return Promise.resolve().then(function () {
      var state = stateOf(self);
      if (state.data.readyState === 'ended') return;
      if (constraints == null) constraints = {};
      if (typeof constraints !== 'object' || Array.isArray(constraints)) {
        throw new TypeError('capture constraints must be a dictionary');
      }
      state.data = call('apply', {
        handle: state.data.handle,
        constraints: copy(constraints)
      });
    });
  };

  function MediaStream(value) {
    if (!(this instanceof MediaStream)) throw new TypeError('MediaStream requires new');
    Target.call(this);
    var tracks;
    if (value === undefined) tracks = [];
    else if (streamState.has(value)) tracks = value.getTracks();
    else {
      if (value == null || typeof value[Symbol.iterator] !== 'function') {
        throw new TypeError('MediaStream requires a stream or track sequence');
      }
      tracks = Array.from(value);
    }
    var unique = [];
    tracks.forEach(function (track) {
      stateOf(track);
      if (!unique.includes(track)) unique.push(track);
    });
    streamState.set(this, { id: call('id'), tracks: unique });
    this.onaddtrack = null;
    this.onremovetrack = null;
  }
  inherit(MediaStream);
  function streamOf(stream) {
    var state = streamState.get(stream);
    if (!state) throw new TypeError('expected a MediaStream');
    return state;
  }
  Object.defineProperty(MediaStream.prototype, 'id', {
    enumerable: true,
    get: function () { return streamOf(this).id; }
  });
  Object.defineProperty(MediaStream.prototype, 'active', {
    enumerable: true,
    get: function () {
      return streamOf(this).tracks.some(function (track) { return track.readyState === 'live'; });
    }
  });
  MediaStream.prototype.getTracks = function () { return streamOf(this).tracks.slice(); };
  MediaStream.prototype.getAudioTracks = function () {
    return streamOf(this).tracks.filter(function (track) { return track.kind === 'audio'; });
  };
  MediaStream.prototype.getVideoTracks = function () {
    return streamOf(this).tracks.filter(function (track) { return track.kind === 'video'; });
  };
  MediaStream.prototype.getTrackById = function (id) {
    id = String(id);
    return streamOf(this).tracks.find(function (track) { return track.id === id; }) || null;
  };
  MediaStream.prototype.addTrack = function (track) {
    stateOf(track);
    var tracks = streamOf(this).tracks;
    if (!tracks.includes(track)) tracks.push(track);
  };
  MediaStream.prototype.removeTrack = function (track) {
    stateOf(track);
    var state = streamOf(this);
    state.tracks = state.tracks.filter(function (candidate) { return candidate !== track; });
  };
  MediaStream.prototype.clone = function () {
    var cloned = [];
    try {
      this.getTracks().forEach(function (track) { cloned.push(track.clone()); });
      return new MediaStream(cloned);
    } catch (error) {
      cloned.forEach(function (track) { track.stop(); });
      throw error;
    }
  };

  Object.defineProperty(g, '__chuzzMediaCreateAudioStream', {
    value: function (metadata) {
      if (!metadata || !Number.isSafeInteger(metadata.track) || metadata.track <= 0) {
        throw new TypeError('invalid graph PCM metadata');
      }
      var descriptor = call('audioStream', { audio: metadata.track });
      var track = new MediaStreamTrack(secret, descriptor);
      try {
        return new MediaStream([track]);
      } catch (error) {
        track.stop();
        throw error;
      }
    }
  });
  Object.defineProperty(g, '__chuzzMediaAudioEnded', {
    value: function () {
      // Native source state ends every live clone sharing the closed graph.
      call('audioEnded');
    }
  });

  function MediaDeviceInfo(token, data) {
    if (token !== secret) throw new TypeError('Illegal constructor');
    deviceState.set(this, data);
  }
  ['deviceId', 'groupId', 'kind', 'label'].forEach(function (name) {
    Object.defineProperty(MediaDeviceInfo.prototype, name, {
      enumerable: true,
      get: function () {
        var data = deviceState.get(this);
        if (!data) throw new TypeError('expected a MediaDeviceInfo');
        return data[name];
      }
    });
  });
  MediaDeviceInfo.prototype.toJSON = function () {
    return { deviceId: this.deviceId, groupId: this.groupId, kind: this.kind, label: this.label };
  };
  function InputDeviceInfo(token, data) { MediaDeviceInfo.call(this, token, data); }
  InputDeviceInfo.prototype = Object.create(MediaDeviceInfo.prototype);
  InputDeviceInfo.prototype.constructor = InputDeviceInfo;
  InputDeviceInfo.prototype.getCapabilities = function () {
    var data = deviceState.get(this);
    if (!data) throw new TypeError('expected an InputDeviceInfo');
    return copy(data.capabilities || {});
  };

  function MediaDevices(token) {
    if (token !== secret) throw new TypeError('Illegal constructor');
    Target.call(this);
    this.ondevicechange = null;
  }
  inherit(MediaDevices);
  MediaDevices.prototype.getSupportedConstraints = function () {
    return {
      width: true, height: true, aspectRatio: true, frameRate: true,
      facingMode: true, resizeMode: true, deviceId: true, groupId: true,
      sampleRate: true, sampleSize: true, channelCount: true, latency: true,
      echoCancellation: true, noiseSuppression: true, autoGainControl: true
    };
  };
  MediaDevices.prototype.enumerateDevices = function () {
    return Promise.resolve().then(function () {
      return call('enumerate').map(function (data) {
        return data.kind === 'audiooutput'
          ? new MediaDeviceInfo(secret, data) : new InputDeviceInfo(secret, data);
      });
    });
  };
  MediaDevices.prototype.getUserMedia = function (constraints) {
    return Promise.resolve().then(function () {
      if (!constraints || typeof constraints !== 'object' || Array.isArray(constraints)) {
        throw new TypeError('getUserMedia requires a constraint dictionary');
      }
      var descriptors = call('acquire', { constraints: copy(constraints) });
      var tracks = [];
      try {
        descriptors.forEach(function (descriptor) {
          tracks.push(new MediaStreamTrack(secret, descriptor));
        });
        return new MediaStream(tracks);
      } catch (error) {
        tracks.forEach(function (track) { track.stop(); });
        descriptors.forEach(function (descriptor) {
          if (!liveTracks.has(descriptor.handle)) {
            try { call('stop', { handle: descriptor.handle }); } catch (ignored) {}
          }
        });
        throw error;
      }
    });
  };

  g.MediaStream = MediaStream;
  g.MediaStreamTrack = MediaStreamTrack;
  g.MediaDeviceInfo = MediaDeviceInfo;
  g.InputDeviceInfo = InputDeviceInfo;
  g.MediaDevices = MediaDevices;

  var policy = call('init');
  if (typeof g.isSecureContext === 'undefined') {
    Object.defineProperty(g, 'isSecureContext', { value: policy.secure, configurable: true });
  }
  if (policy.secure && g.isSecureContext !== false && g.navigator) {
    devices = new MediaDevices(secret);
    Object.defineProperty(g.navigator, 'mediaDevices', {
      configurable: true, enumerable: true, value: devices
    });
  }

  // Explicit fake-device controls and bounded native measurements for UI QA.
  // Payloads stay in Rust. Later renderer and codec consumers lease Host::track.
  Object.defineProperty(g, '__chuzzFakeMedia', {
    value: Object.freeze({
      setPresent: function (kind, present) {
        call('present', { kind: String(kind), value: Boolean(present) });
      },
      setMuted: function (kind, muted) {
        call('mute', { kind: String(kind), value: Boolean(muted) });
      },
      probe: function (track, micros) {
        return call('probe', { handle: stateOf(track).data.handle, micros: micros });
      },
      handle: function (track) { return stateOf(track).data.handle; }
    })
  });
})(globalThis);

