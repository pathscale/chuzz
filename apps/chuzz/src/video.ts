// This source deliberately uses JavaScript syntax so Boa can evaluate it
// directly. It is installed after the shared DOM compatibility shim.
(function () {
  var prototype = Object.getPrototypeOf(document.createElement('video'));
  var originalSrc = Object.getOwnPropertyDescriptor(prototype, 'src');
  var states = new Map();
  var byElement = new WeakMap();
  var commands = [];
  var nextId = 1;
  var activated = false;
  var heartbeat = null;

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

  function later(state, name) {
    var generation = state.generation;
    setTimeout(function () {
      if (state.generation === generation) emit(state.element, name);
    }, 0);
  }

  function rejectPending(state, error) {
    var pending = state.pending.splice(0);
    pending.forEach(function (promise) { promise.reject(error); });
  }

  function wake() {
    if (heartbeat !== null) return;
    heartbeat = setTimeout(function tick() {
      heartbeat = null;
      // Timers keep native readiness, stalls and seek completions polling
      // even when no DOM work or network-provider callback wakes the page.
      if (states.size) wake();
    }, 33);
  }

  function activate(event) {
    if (!event.isTrusted) return;
    if (event.type === 'keydown'
        && ['Escape', 'Shift', 'Control', 'Alt', 'Meta'].indexOf(event.key) >= 0) {
      return;
    }
    activated = true;
  }

  ['pointerdown', 'mousedown', 'touchend', 'keydown', 'click'].forEach(function (name) {
    document.addEventListener(name, activate, true);
  });

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
        seeking: false,
        playing: false,
        readyState: 0,
        networkState: 0,
        width: 0,
        height: 0,
        hasFrame: false,
        frameInk: false,
        error: null,
        muted: false,
        mutedExplicit: false,
        volume: 1,
        rate: 1,
        defaultRate: 1,
        buffered: [],
        seekable: [],
        played: [],
        playedStart: null,
        configuration: '',
        autoAttempted: false,
        pending: [],
        posterImage: null,
        bar: null,
        button: null,
        clock: null
      };
      byElement.set(element, state);
    }
    states.set(state.id, state);
    wake();
    if (element.getAttribute('data-chuzz-video') !== String(state.id)) {
      element.setAttribute('data-chuzz-video', String(state.id));
    }
    return state;
  }

  function isMuted(state) {
    return state.mutedExplicit
      ? state.muted
      : state.element.hasAttribute('muted');
  }

  function preloadFor(state) {
    var value = (state.element.getAttribute('preload') || '').toLowerCase();
    return value === 'none' || value === 'metadata' ? value : 'auto';
  }

  function allowed(state) {
    return isMuted(state) || state.volume === 0 || activated;
  }

  function send(state, op, extra) {
    var command = {
      id: state.id,
      generation: state.generation,
      op: op,
      looping: state.element.hasAttribute('loop'),
      muted: isMuted(state),
      volume: state.volume,
      rate: state.rate,
      activated: activated,
      preload: preloadFor(state)
    };
    if (extra) {
      Object.keys(extra).forEach(function (key) { command[key] = extra[key]; });
    }
    commands.push(command);
  }

  function rawSource(element) {
    if (globalThis.__chuzzMseSource) {
      var objectSource = globalThis.__chuzzMseSource(element);
      if (objectSource !== null) return objectSource;
    }
    var src = element.getAttribute('src');
    if (src !== null) return src;
    var sources = element.querySelectorAll('source[src]');
    for (var index = 0; index < sources.length; index++) {
      var type = sources[index].getAttribute('type');
      if (!type || element.canPlayType(type)) {
        return sources[index].getAttribute('src') || '';
      }
    }
    return '';
  }

  function resolvedSource(src) {
    if (!src) return '';
    try { return new URL(src, location.href).href; }
    catch (error) { return src; }
  }

  function sync(state, force) {
    var src = resolvedSource(rawSource(state.element));
    if (!force && src === state.requested) return;
    var previous = state.requested;
    rejectPending(state, exception('The media source changed', 'AbortError'));
    state.generation++;
    state.requested = src;
    state.selected = src;
    state.time = 0;
    state.duration = NaN;
    state.paused = true;
    state.ended = false;
    state.seeking = false;
    state.playing = false;
    state.readyState = 0;
    state.networkState = src ? 1 : 0;
    state.width = 0;
    state.height = 0;
    state.hasFrame = false;
    state.frameInk = false;
    state.error = null;
    state.buffered = [];
    state.seekable = [];
    state.played = [];
    state.playedStart = null;
    state.rate = state.defaultRate;
    state.configuration = '';
    state.autoAttempted = false;
    if (previous) {
      later(state, 'abort');
      later(state, 'emptied');
    }
    var load = { src: src, force: Boolean(force) };
    if (globalThis.__chuzzMseAttach) {
      var mediaSource = globalThis.__chuzzMseAttach(state.element, src, Boolean(force));
      if (mediaSource !== null) load.mse = mediaSource;
    }
    send(state, 'load', load);
    updateUi(state);
  }

  function property(name, get, set) {
    Object.defineProperty(prototype, name, {
      configurable: true,
      enumerable: true,
      get: function () { return get(stateFor(this)); },
      set: set ? function (value) { set(stateFor(this), value); } : undefined
    });
  }

  function timeRanges(ranges) {
    var values = ranges.map(function (range) { return range.slice(); });
    function at(index, side) {
      index = Number(index);
      if (!Number.isInteger(index) || index < 0 || index >= values.length) {
        throw exception('The TimeRanges index is out of bounds', 'IndexSizeError');
      }
      return values[index][side];
    }
    return Object.freeze({
      length: values.length,
      start: function (index) { return at(index, 0); },
      end: function (index) { return at(index, 1); }
    });
  }

  function recordPlayed(state) {
    if (state.playedStart === null || state.time <= state.playedStart) return;
    state.played.push([state.playedStart, state.time]);
    state.played.sort(function (left, right) { return left[0] - right[0]; });
    var merged = [];
    state.played.forEach(function (range) {
      var previous = merged[merged.length - 1];
      if (previous && range[0] <= previous[1]) {
        previous[1] = Math.max(previous[1], range[1]);
      } else {
        merged.push(range.slice());
      }
    });
    state.played = merged;
    state.playedStart = state.time;
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
  property('seeking', function (state) { return state.seeking; });
  property('duration', function (state) { return state.duration; });
  property('readyState', function (state) { return state.readyState; });
  property('networkState', function (state) { return state.networkState; });
  property('error', function (state) { return state.error; });
  property('videoWidth', function (state) { return state.width; });
  property('videoHeight', function (state) { return state.height; });
  property('buffered', function (state) { return timeRanges(state.buffered); });
  property('seekable', function (state) { return timeRanges(state.seekable); });
  property('played', function (state) {
    recordPlayed(state);
    return timeRanges(state.played);
  });

  property('currentTime', function (state) {
    return state.time;
  }, function (state, value) {
    sync(state, false);
    var time = Number(value);
    if (!Number.isFinite(time)) throw new TypeError('currentTime must be finite');
    recordPlayed(state);
    state.playedStart = null;
    state.time = Number.isFinite(state.duration)
      ? Math.min(state.duration, Math.max(0, time))
      : Math.max(0, time);
    state.ended = false;
    state.seeking = true;
    state.playing = false;
    send(state, 'seek', { time: state.time });
  });

  property('muted', isMuted, function (state, value) {
    var previous = isMuted(state);
    state.mutedExplicit = true;
    state.muted = Boolean(value);
    if (previous !== state.muted) later(state, 'volumechange');
    send(state, 'configure');
  });

  property('volume', function (state) {
    return state.volume;
  }, function (state, value) {
    var volume = Number(value);
    if (!Number.isFinite(volume) || volume < 0 || volume > 1) {
      throw exception('volume must be between zero and one', 'IndexSizeError');
    }
    if (state.volume !== volume) {
      state.volume = volume;
      later(state, 'volumechange');
      send(state, 'configure');
    }
  });

  function setRate(state, value, defaultValue) {
    var rate = Number(value);
    if (!Number.isFinite(rate)) throw new TypeError('playbackRate must be finite');
    if (rate < 0 || rate > 16) {
      throw exception('This playback rate is unsupported', 'NotSupportedError');
    }
    var name = defaultValue ? 'defaultRate' : 'rate';
    if (state[name] === rate) return;
    state[name] = rate;
    later(state, 'ratechange');
    if (!defaultValue) send(state, 'configure');
  }

  property('playbackRate', function (state) {
    return state.rate;
  }, function (state, value) {
    setRate(state, value, false);
  });
  property('defaultPlaybackRate', function (state) {
    return state.defaultRate;
  }, function (state, value) {
    setRate(state, value, true);
  });
  property('preload', preloadFor, function (state, value) {
    state.element.setAttribute('preload', String(value));
    send(state, 'configure');
  });
  property('poster', function (state) {
    return resolvedSource(state.element.getAttribute('poster') || '');
  }, function (state, value) {
    state.element.setAttribute('poster', String(value));
    updateUi(state);
  });

  ['autoplay', 'loop', 'controls', 'defaultMuted'].forEach(function (name) {
    var attribute = name === 'defaultMuted' ? 'muted' : name;
    property(name, function (state) {
      return state.element.hasAttribute(attribute);
    }, function (state, value) {
      var previous = isMuted(state);
      if (value) state.element.setAttribute(attribute, '');
      else state.element.removeAttribute(attribute);
      if (isMuted(state) !== previous) later(state, 'volumechange');
      send(state, 'configure');
      updateUi(state);
    });
  });

  [
    ['HAVE_NOTHING', 0], ['HAVE_METADATA', 1], ['HAVE_CURRENT_DATA', 2],
    ['HAVE_FUTURE_DATA', 3], ['HAVE_ENOUGH_DATA', 4],
    ['NETWORK_EMPTY', 0], ['NETWORK_IDLE', 1],
    ['NETWORK_LOADING', 2], ['NETWORK_NO_SOURCE', 3]
  ].forEach(function (constant) {
    if (typeof prototype[constant[0]] === 'undefined') {
      Object.defineProperty(prototype, constant[0], { value: constant[1] });
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
          'Audio elements are unavailable', 'NotSupportedError'
        ));
      }
      var state = stateFor(this);
      sync(state, false);
      if (!state.requested) {
        state.error = { code: 4, message: 'The video has no source' };
        later(state, 'error');
      }
      if (state.error) {
        return Promise.reject(exception(state.error.message, 'NotSupportedError'));
      }
      if (!allowed(state)) {
        return Promise.reject(exception(
          'Audible playback requires user activation', 'NotAllowedError'
        ));
      }
      state.autoAttempted = true;
      if (!state.paused && state.playing && !state.ended) {
        return Promise.resolve();
      }
      if (state.ended) {
        state.time = 0;
        state.ended = false;
        state.seeking = true;
        send(state, 'seek', { time: 0 });
      }
      state.paused = false;
      var promise = new Promise(function (resolve, reject) {
        state.pending.push({ resolve: resolve, reject: reject });
      });
      send(state, 'play');
      updateUi(state);
      return promise;
    }
  });
  Object.defineProperty(prototype, 'pause', {
    configurable: true,
    writable: true,
    value: function () {
      var state = stateFor(this);
      sync(state, false);
      recordPlayed(state);
      state.playedStart = null;
      state.autoAttempted = true;
      state.paused = true;
      state.playing = false;
      rejectPending(state, exception('Playback was paused', 'AbortError'));
      send(state, 'pause');
      updateUi(state);
    }
  });
  Object.defineProperty(prototype, 'fastSeek', {
    configurable: true,
    writable: true,
    value: function (time) { this.currentTime = time; }
  });
  Object.defineProperty(prototype, 'canPlayType', {
    configurable: true,
    writable: true,
    value: function (type) {
      type = String(type);
      if (!globalThis.__chuzzVideoSupported
          || !globalThis.__chuzzMseSupported
          || !globalThis.__chuzzMseSupported(type)) return '';
      return /\bcodecs\s*=/i.test(type) ? 'probably' : 'maybe';
    }
  });

  function remove(node) {
    if (node && node.parentNode) node.parentNode.removeChild(node);
  }

  function setAttribute(element, name, value) {
    if (element.getAttribute(name) !== value) element.setAttribute(name, value);
  }

  function text(element, value) {
    if (element.textContent !== value) element.textContent = value;
  }

  function formatTime(value) {
    if (!Number.isFinite(value)) return '--:--';
    var seconds = Math.max(0, Math.floor(value));
    var remainder = seconds % 60;
    return Math.floor(seconds / 60) + ':' + (remainder < 10 ? '0' : '') + remainder;
  }

  function updateUi(state) {
    var element = state.element;
    var poster = resolvedSource(element.getAttribute('poster') || '');
    var controls = element.hasAttribute('controls');
    if (!document.body || !element.parentNode) return;
    if (!poster || state.hasFrame) {
      remove(state.posterImage);
      state.posterImage = null;
    } else if (!state.posterImage) {
      state.posterImage = document.createElement('img');
      state.posterImage.setAttribute('alt', '');
      state.posterImage.setAttribute('aria-hidden', 'true');
      state.posterImage.setAttribute('data-chuzz-media-ui', 'poster');
      document.body.appendChild(state.posterImage);
    }
    if (!controls) {
      remove(state.bar);
      state.bar = null;
      state.button = null;
      state.clock = null;
    } else if (!state.bar) {
      state.bar = document.createElement('div');
      state.bar.setAttribute('role', 'group');
      state.bar.setAttribute('aria-label', 'Video controls');
      state.bar.setAttribute('data-chuzz-media-ui', 'controls');
      state.button = document.createElement('button');
      state.button.setAttribute('type', 'button');
      state.button.addEventListener('click', function () {
        if (state.paused || state.ended) element.play().catch(function () {});
        else element.pause();
      });
      state.clock = document.createElement('span');
      state.bar.appendChild(state.button);
      state.bar.appendChild(state.clock);
      document.body.appendChild(state.bar);
    }
    if (!state.posterImage && !state.bar) return;
    var rect = element.getBoundingClientRect();
    var visible = rect.width > 0 && rect.height > 0;
    if (state.posterImage) {
      setAttribute(state.posterImage, 'src', poster);
      setAttribute(state.posterImage, 'style',
        'position:fixed;pointer-events:none;object-fit:contain;z-index:2147483645;'
        + 'display:' + (visible ? 'block' : 'none') + ';'
        + 'left:' + rect.left + 'px;top:' + rect.top + 'px;'
        + 'width:' + rect.width + 'px;height:' + rect.height + 'px;');
    }
    if (state.bar) {
      setAttribute(state.bar, 'style',
        'position:fixed;z-index:2147483646;box-sizing:border-box;'
        + 'background:#202020;color:white;font:12px sans-serif;'
        + 'display:' + (visible ? 'flex' : 'none') + ';'
        + 'align-items:center;gap:8px;padding:2px 6px;height:28px;'
        + 'left:' + rect.left + 'px;top:' + (rect.top + Math.max(0, rect.height - 28)) + 'px;'
        + 'width:' + rect.width + 'px;');
      var label = state.paused || state.ended ? 'Play video' : 'Pause video';
      setAttribute(state.button, 'aria-label', label);
      text(state.button, state.paused || state.ended ? 'Play' : 'Pause');
      text(state.clock, formatTime(state.time) + ' / ' + formatTime(state.duration));
    }
  }

  globalThis.__chuzzVideoDrain = function () {
    var seen = new Set();
    var elements = document.querySelectorAll('video');
    for (var index = 0; index < elements.length; index++) {
      var state = stateFor(elements[index]);
      seen.add(state.id);
      sync(state, false);
      var configuration = [
        state.element.hasAttribute('loop'), isMuted(state), state.volume,
        state.rate, activated, preloadFor(state)
      ].join(':');
      if (state.configuration !== configuration) {
        state.configuration = configuration;
        send(state, 'configure');
      }
      if (!state.autoAttempted && state.requested
          && state.element.hasAttribute('autoplay') && allowed(state)) {
        state.autoAttempted = true;
        state.element.play().catch(function () {});
      }
      updateUi(state);
    }
    states.forEach(function (state, id) {
      if (seen.has(id)) return;
      rejectPending(state, exception('The video was removed', 'AbortError'));
      send(state, 'release');
      if (globalThis.__chuzzMseDetach) globalThis.__chuzzMseDetach(state.element);
      remove(state.posterImage);
      remove(state.bar);
      state.posterImage = null;
      state.bar = null;
      state.button = null;
      state.clock = null;
      state.generation++;
      state.requested = null;
      state.paused = true;
      state.seeking = false;
      state.playing = false;
      states.delete(id);
    });
    var result = commands;
    commands = [];
    return result;
  };

  globalThis.__chuzzVideoHasColour = function (element) {
    var state = byElement.get(element);
    return Boolean(state && state.hasFrame && state.frameInk);
  };

  globalThis.__chuzzVideoUpdate = function (id, generation, update) {
    var state = states.get(id);
    if (!state || state.generation !== generation) return;
    if (!state.seeking) recordPlayed(state);
    state.selected = update.src;
    state.time = update.time;
    state.duration = update.durationInfinite
      ? Infinity : (update.duration === null ? NaN : update.duration);
    state.frameInk = Boolean(update.frameInk);
    state.paused = update.paused;
    state.ended = update.ended;
    state.seeking = update.seeking;
    state.readyState = update.readyState;
    state.networkState = update.networkState;
    state.buffered = update.buffered;
    state.seekable = update.seekable;
    state.hasFrame = update.hasFrame;
    state.width = update.width;
    state.height = update.height;
    state.error = update.error;
    if (state.paused || state.seeking) {
      state.playing = false;
      state.playedStart = null;
    }
    updateUi(state);

    for (var index = 0; index < update.events.length; index++) {
      if (state.generation !== generation) break;
      var name = update.events[index];
      if (name === 'blocked') {
        state.paused = true;
        state.playing = false;
        rejectPending(state, exception(
          'Audible playback requires user activation', 'NotAllowedError'
        ));
        continue;
      }
      if (name === 'error') {
        var errorName = state.error && state.error.code === 4
          ? 'NotSupportedError' : 'AbortError';
        rejectPending(state, exception(
          state.error ? state.error.message : 'Video playback failed', errorName
        ));
      }
      if (name === 'waiting') {
        state.playing = false;
        state.playedStart = null;
      }
      if (name === 'playing') {
        if (state.paused || state.seeking) continue;
        state.playing = true;
        state.playedStart = state.time;
        emit(state.element, name);
        if (state.generation !== generation || state.paused) continue;
        var pending = state.pending.splice(0);
        pending.forEach(function (promise) { promise.resolve(); });
      } else {
        emit(state.element, name);
      }
    }
  };
})();
