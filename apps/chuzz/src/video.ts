// This source deliberately uses JavaScript syntax so Boa can evaluate it
// directly. It is installed after the shared DOM compatibility shim.
(function () {
  var prototype = Object.getPrototypeOf(document.createElement('video'));
  var originalSrc = Object.getOwnPropertyDescriptor(prototype, 'src');
  var states = new Map();
  var byElement = new WeakMap();
  var commands = [];
  var nextId = 1;

  function exception(message, name) {
    return new DOMException(message, name);
  }

  function emit(element, name) {
    try {
      element.dispatchEvent(new Event(name));
    } catch (error) {
      // An exception in a page handler must not stop the media bridge.
    }
  }

  function rejectPending(state, error) {
    var pending = state.pending.splice(0);
    pending.forEach(function (promise) { promise.reject(error); });
  }

  function stateFor(element) {
    if (!element || element.tagName !== 'VIDEO') {
      throw new TypeError('This operation requires an HTMLVideoElement');
    }
    var state = byElement.get(element);
    if (!state) {
      state = {
        id: nextId++,
        generation: 0,
        element: element,
        requested: null,
        selected: '',
        time: 0,
        duration: NaN,
        paused: true,
        ended: false,
        playing: false,
        readyState: 0,
        width: 0,
        height: 0,
        error: null,
        muted: false,
        mutedExplicit: false,
        volume: 1,
        looping: element.hasAttribute('loop'),
        autoAttempted: false,
        pending: []
      };
      byElement.set(element, state);
    }
    states.set(state.id, state);
    if (element.getAttribute('data-chuzz-video') !== String(state.id)) {
      element.setAttribute('data-chuzz-video', String(state.id));
    }
    return state;
  }

  function send(state, op, extra) {
    var command = {
      id: state.id,
      generation: state.generation,
      op: op,
      looping: state.element.hasAttribute('loop')
    };
    if (extra) {
      Object.keys(extra).forEach(function (key) { command[key] = extra[key]; });
    }
    commands.push(command);
  }

  function rawSource(element) {
    var src = element.getAttribute('src');
    if (src !== null) return src;
    var source = element.querySelector('source[src]');
    return source ? (source.getAttribute('src') || '') : '';
  }

  function resolvedSource(src) {
    if (!src) return '';
    try { return new URL(src, location.href).href; }
    catch (error) { return src; }
  }

  function sync(state, force) {
    var src = rawSource(state.element);
    if (!force && src === state.requested) return;
    rejectPending(state, exception('The media source changed', 'AbortError'));
    state.generation++;
    state.requested = src;
    state.selected = '';
    state.time = 0;
    state.duration = NaN;
    state.paused = true;
    state.ended = false;
    state.playing = false;
    state.readyState = 0;
    state.width = 0;
    state.height = 0;
    state.error = null;
    state.autoAttempted = false;
    send(state, 'load', { src: src });
  }

  function property(name, get, set) {
    Object.defineProperty(prototype, name, {
      configurable: true,
      enumerable: true,
      get: function () { return get(stateFor(this)); },
      set: set ? function (value) { set(stateFor(this), value); } : undefined
    });
  }

  Object.defineProperty(prototype, 'src', {
    configurable: true,
    enumerable: true,
    get: function () {
      if (this.tagName === 'VIDEO') {
        return resolvedSource(this.getAttribute('src') || '');
      }
      return originalSrc && originalSrc.get
        ? originalSrc.get.call(this)
        : (this.getAttribute('src') || '');
    },
    set: function (value) {
      if (this.tagName !== 'VIDEO' && originalSrc && originalSrc.set) {
        originalSrc.set.call(this, value);
        return;
      }
      this.setAttribute('src', String(value));
      if (this.tagName === 'VIDEO') sync(stateFor(this), false);
    }
  });

  property('currentSrc', function (state) { return state.selected; });
  property('paused', function (state) { return state.paused; });
  property('ended', function (state) { return state.ended; });
  property('duration', function (state) { return state.duration; });
  property('readyState', function (state) { return state.readyState; });
  property('error', function (state) { return state.error; });
  property('videoWidth', function (state) { return state.width; });
  property('videoHeight', function (state) { return state.height; });
  property('currentTime', function (state) {
    return state.time;
  }, function (state, value) {
    sync(state, false);
    var time = Number(value);
    if (!Number.isFinite(time)) {
      throw new TypeError('currentTime must be finite');
    }
    state.time = Number.isFinite(state.duration)
      ? Math.min(state.duration, Math.max(0, time))
      : Math.max(0, time);
    state.ended = false;
    state.playing = false;
    send(state, 'seek', { time: state.time });
  });
  property('muted', function (state) {
    return state.mutedExplicit
      ? state.muted
      : state.element.hasAttribute('muted');
  }, function (state, value) {
    state.mutedExplicit = true;
    state.muted = Boolean(value);
  });
  property('volume', function (state) {
    return state.volume;
  }, function (state, value) {
    var volume = Number(value);
    if (!Number.isFinite(volume) || volume < 0 || volume > 1) {
      throw exception('volume must be between zero and one', 'IndexSizeError');
    }
    state.volume = volume;
  });

  ['autoplay', 'loop', 'defaultMuted'].forEach(function (name) {
    var attribute = name === 'defaultMuted' ? 'muted' : name;
    property(name, function (state) {
      return state.element.hasAttribute(attribute);
    }, function (state, value) {
      if (value) state.element.setAttribute(attribute, '');
      else state.element.removeAttribute(attribute);
    });
  });

  ['HAVE_NOTHING', 'HAVE_METADATA', 'HAVE_CURRENT_DATA',
   'HAVE_FUTURE_DATA', 'HAVE_ENOUGH_DATA'].forEach(function (name, value) {
    if (typeof prototype[name] === 'undefined') {
      Object.defineProperty(prototype, name, { value: value });
    }
  });

  Object.defineProperty(prototype, 'load', {
    configurable: true,
    writable: true,
    value: function () { sync(stateFor(this), true); }
  });
  Object.defineProperty(prototype, 'play', {
    configurable: true,
    writable: true,
    value: function () {
      if (this.tagName !== 'VIDEO') {
        return Promise.reject(exception(
          'Audio playback is unavailable', 'NotSupportedError'
        ));
      }
      var state = stateFor(this);
      sync(state, false);
      if (!state.requested) {
        state.error = { code: 4, message: 'The video has no source' };
        var generation = state.generation;
        setTimeout(function () {
          if (state.generation === generation) emit(state.element, 'error');
        }, 0);
      }
      if (state.error) {
        return Promise.reject(exception(state.error.message, 'NotSupportedError'));
      }
      if (!state.paused && state.playing && !state.ended) {
        return Promise.resolve();
      }
      if (state.ended) {
        state.time = 0;
        state.ended = false;
        send(state, 'seek', { time: 0 });
      }
      state.paused = false;
      var promise = new Promise(function (resolve, reject) {
        state.pending.push({ resolve: resolve, reject: reject });
      });
      send(state, 'play');
      return promise;
    }
  });
  Object.defineProperty(prototype, 'pause', {
    configurable: true,
    writable: true,
    value: function () {
      var state = stateFor(this);
      sync(state, false);
      state.autoAttempted = true;
      state.paused = true;
      state.playing = false;
      rejectPending(state, exception('Playback was paused', 'AbortError'));
      send(state, 'pause');
    }
  });
  Object.defineProperty(prototype, 'canPlayType', {
    configurable: true,
    writable: true,
    value: function (type) {
      stateFor(this);
      type = String(type).toLowerCase();
      if (!globalThis.__chuzzVideoSupported) return '';
      if (!/^video\/mp4(?:\s*;|$)/.test(type)) return '';
      if (type.indexOf('av01') >= 0 || type.indexOf('vp09') >= 0) return '';
      return 'maybe';
    }
  });

  globalThis.__chuzzVideoDrain = function () {
    var seen = new Set();
    var elements = document.querySelectorAll('video');
    for (var index = 0; index < elements.length; index++) {
      var state = stateFor(elements[index]);
      seen.add(state.id);
      sync(state, false);
      var looping = state.element.hasAttribute('loop');
      if (state.looping !== looping) {
        state.looping = looping;
        send(state, 'configure');
      }
      if (!state.autoAttempted && state.requested
          && state.element.hasAttribute('autoplay') && state.element.muted) {
        state.autoAttempted = true;
        state.element.play().catch(function () {});
      }
    }
    states.forEach(function (state, id) {
      if (seen.has(id)) return;
      rejectPending(state, exception('The video was removed', 'AbortError'));
      send(state, 'release');
      state.generation++;
      state.requested = null;
      state.paused = true;
      state.playing = false;
      states.delete(id);
    });
    var result = commands;
    commands = [];
    return result;
  };

  globalThis.__chuzzVideoUpdate = function (id, generation, update) {
    var state = states.get(id);
    if (!state || state.generation !== generation) return;
    state.selected = update.src;
    state.time = update.time;
    state.duration = update.duration === null ? NaN : update.duration;
    state.paused = update.paused;
    state.ended = update.ended;
    state.readyState = update.readyState;
    state.width = update.width;
    state.height = update.height;
    state.error = update.error;
    if (state.paused) state.playing = false;

    for (var index = 0; index < update.events.length; index++) {
      if (state.generation !== generation) break;
      var name = update.events[index];
      if (name === 'error') {
        var errorName = state.error && state.error.code === 4
          ? 'NotSupportedError' : 'AbortError';
        rejectPending(state, exception(
          state.error ? state.error.message : 'Video playback failed', errorName
        ));
      }
      if (name === 'playing') {
        if (state.paused) continue;
        state.playing = true;
        var pending = state.pending.splice(0);
        emit(state.element, name);
        pending.forEach(function (promise) { promise.resolve(); });
      } else {
        emit(state.element, name);
      }
    }
  };
})();
