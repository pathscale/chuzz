(function (g) {
  "use strict";

  const contexts = new Map();
  const trackIds = new WeakMap();

  function exception(name, message) {
    if (typeof g.DOMException === "function" && name !== "RangeError" && name !== "TypeError") {
      return new g.DOMException(message, name);
    }
    const error = name === "RangeError" ? new RangeError(message)
      : name === "TypeError" ? new TypeError(message) : new Error(message);
    error.name = name;
    return error;
  }

  function call(command) {
    try {
      return g.__chuzzAudioCall(JSON.stringify(command));
    } catch (error) {
      const match = /^([A-Za-z]+Error): (.*)$/.exec(String(error.message));
      if (match) throw exception(match[1], match[2]);
      throw error;
    }
  }

  function finite(value, name) {
    value = Number(value);
    if (!Number.isFinite(value)) throw new TypeError(name + " must be finite");
    return value;
  }

  function time(value) {
    value = finite(value, "time");
    if (value < 0) throw new RangeError("time must be nonnegative");
    return value;
  }

  class AudioEvents {
    constructor() {
      this._listeners = new Map();
    }
    addEventListener(type, callback, options = {}) {
      if (callback == null) return;
      type = String(type);
      const capture = typeof options === "boolean" ? options : !!options.capture;
      const entries = this._listeners.get(type) || [];
      if (entries.some(entry => entry.callback === callback && entry.capture === capture)) return;
      if (options && options.signal && options.signal.aborted) return;
      entries.push({ callback, capture, once: !!options.once });
      this._listeners.set(type, entries);
      if (options && options.signal) {
        options.signal.addEventListener("abort", () => {
          this.removeEventListener(type, callback, capture);
        }, { once: true });
      }
    }
    removeEventListener(type, callback, options = {}) {
      const capture = typeof options === "boolean" ? options : !!options.capture;
      const entries = this._listeners.get(String(type)) || [];
      this._listeners.set(String(type), entries.filter(entry =>
        entry.callback !== callback || entry.capture !== capture));
    }
    dispatchEvent(event) {
      if (!event || typeof event.type !== "string") throw new TypeError("expected an event");
      let stopped = false;
      try {
        Object.defineProperty(event, "target", { configurable: true, value: this });
        Object.defineProperty(event, "currentTarget", { configurable: true, value: this });
      } catch (_) {}
      const originalStop = event.stopImmediatePropagation;
      try {
        event.stopImmediatePropagation = function () {
          stopped = true;
          if (originalStop) originalStop.call(this);
        };
      } catch (_) {}
      const invoke = callback => {
        try {
          if (typeof callback === "function") callback.call(this, event);
          else if (callback && typeof callback.handleEvent === "function") callback.handleEvent(event);
        } catch (error) {
          setTimeout(() => { throw error; }, 0);
        }
      };
      const handler = this["on" + event.type];
      if (typeof handler === "function") invoke(handler);
      for (const entry of (this._listeners.get(event.type) || []).slice()) {
        if (stopped) break;
        if (!(this._listeners.get(event.type) || []).includes(entry)) continue;
        if (entry.once) this.removeEventListener(event.type, entry.callback, entry.capture);
        invoke(entry.callback);
      }
      try {
        Object.defineProperty(event, "currentTarget", { configurable: true, value: null });
      } catch (_) {}
      return !event.defaultPrevented;
    }
    _emit(type, fields = {}) {
      const event = typeof g.Event === "function" ? new g.Event(type) : { type };
      for (const key of Object.keys(fields)) {
        Object.defineProperty(event, key, { configurable: true, value: fields[key] });
      }
      this.dispatchEvent(event);
    }
  }

  g.__chuzzAudioAttachTrack = function (track, id) {
    if (!track || !Number.isSafeInteger(id) || id <= 0) throw new TypeError("invalid audio track");
    trackIds.set(track, id);
  };

  class AudioParam {
    constructor(node, name, defaultValue, minimum, maximum) {
      this._node = node;
      this._name = name;
      Object.defineProperties(this, {
        defaultValue: { value: defaultValue, enumerable: true },
        minValue: { value: minimum, enumerable: true },
        maxValue: { value: maximum, enumerable: true },
      });
      this.automationRate = "a-rate";
    }
    get value() {
      return this._node.context._call({
        op: "paramValue", node: this._node._id, name: this._name,
      });
    }
    set value(value) {
      this.setValueAtTime(value, this._node.context.currentTime);
    }
    _event(kind, value, at, constant = 0) {
      value = finite(value, "value");
      value = Math.max(this.minValue, Math.min(this.maxValue, value));
      this._node.context._call({
        op: "param", node: this._node._id, name: this._name,
        event: { kind, value, time: time(at), timeConstant: constant },
      });
      return this;
    }
    setValueAtTime(value, at) { return this._event("set", value, at); }
    linearRampToValueAtTime(value, at) { return this._event("linear", value, at); }
    exponentialRampToValueAtTime(value, at) {
      value = finite(value, "value");
      if (value <= 0) throw new RangeError("exponential ramp value must be positive");
      return this._event("exponential", value, at);
    }
    setTargetAtTime(value, at, constant) {
      constant = finite(constant, "timeConstant");
      if (constant < 0) throw new RangeError("timeConstant must be nonnegative");
      if (constant === 0) return this.setValueAtTime(value, at);
      return this._event("target", value, at, constant);
    }
    cancelScheduledValues(at) {
      this._node.context._call({
        op: "cancel", node: this._node._id, name: this._name, time: time(at), hold: false,
      });
      return this;
    }
    cancelAndHoldAtTime(at) {
      this._node.context._call({
        op: "cancel", node: this._node._id, name: this._name, time: time(at), hold: true,
      });
      return this;
    }
  }

  class AudioNode extends AudioEvents {
    constructor(context, id, inputs, outputs) {
      super();
      if (!(context instanceof AudioContext)) throw new TypeError("expected an AudioContext");
      Object.defineProperties(this, {
        context: { value: context, enumerable: true },
        numberOfInputs: { value: inputs, enumerable: true },
        numberOfOutputs: { value: outputs, enumerable: true },
      });
      this._id = id;
      context._nodes.set(id, this);
    }
    connect(destination, output = 0, input = 0) {
      if (Number(output) !== 0 || Number(input) !== 0) {
        throw exception("IndexSizeError", "only port zero is supported");
      }
      const parameter = destination instanceof AudioParam;
      const target = parameter ? destination._node : destination;
      if (!(target instanceof AudioNode) || target.context !== this.context) {
        throw exception("InvalidAccessError", "nodes must belong to the same context");
      }
      this.context._call({
        op: "connect", from: this._id, to: target._id,
        param: parameter ? destination._name : null,
      });
      return parameter ? undefined : destination;
    }
    disconnect(destination, output = 0, input = 0) {
      if (typeof destination === "number") {
        if (destination !== 0) throw exception("IndexSizeError", "invalid output port");
        destination = undefined;
      }
      if (Number(output) !== 0 || Number(input) !== 0) {
        throw exception("IndexSizeError", "only port zero is supported");
      }
      const parameter = destination instanceof AudioParam;
      const target = parameter ? destination._node : destination;
      if (target !== undefined && (!(target instanceof AudioNode) || target.context !== this.context)) {
        throw exception("InvalidAccessError", "nodes must belong to the same context");
      }
      this.context._call({
        op: "disconnect", from: this._id,
        to: target === undefined ? null : target._id,
        param: parameter ? destination._name : null,
      });
    }
  }

  class OscillatorNode extends AudioNode {
    constructor(context, options = {}) {
      const metadata = context._create("oscillator");
      super(context, metadata.node, 0, 1);
      this.frequency = new AudioParam(this, "frequency", 440, -context.sampleRate / 2, context.sampleRate / 2);
      this.detune = new AudioParam(this, "detune", 0, -153600, 153600);
      this._type = "sine";
      this.onended = null;
      if (options.type !== undefined) this.type = options.type;
      if (options.frequency !== undefined) this.frequency.value = options.frequency;
      if (options.detune !== undefined) this.detune.value = options.detune;
    }
    get type() { return this._type; }
    set type(value) {
      value = String(value);
      this.context._call({ op: "wave", node: this._id, wave: value });
      this._type = value;
    }
    start(at = 0) {
      this.context._call({ op: "schedule", node: this._id, start: true, time: time(at) });
    }
    stop(at = 0) {
      this.context._call({ op: "schedule", node: this._id, start: false, time: time(at) });
    }
  }

  class GainNode extends AudioNode {
    constructor(context, options = {}) {
      const metadata = context._create("gain");
      super(context, metadata.node, 1, 1);
      this.gain = new AudioParam(this, "gain", 1, -3.4028234663852886e38, 3.4028234663852886e38);
      if (options.gain !== undefined) this.gain.value = options.gain;
    }
  }

  class AnalyserNode extends AudioNode {
    constructor(context, options = {}) {
      const metadata = context._create("analyser");
      super(context, metadata.node, 1, 1);
      this._analysis = {
        fftSize: options.fftSize === undefined ? 2048 : Number(options.fftSize),
        minDecibels: options.minDecibels === undefined ? -100 : Number(options.minDecibels),
        maxDecibels: options.maxDecibels === undefined ? -30 : Number(options.maxDecibels),
        smoothing: options.smoothingTimeConstant === undefined ? 0.8 : Number(options.smoothingTimeConstant),
      };
      this._configure(this._analysis);
    }
    _configure(configuration) {
      if (!Number.isInteger(configuration.fftSize)) {
        throw exception("IndexSizeError", "fftSize must be an integer");
      }
      for (const key of ["minDecibels", "maxDecibels", "smoothing"]) {
        finite(configuration[key], key);
      }
      this.context._call(Object.assign({
        op: "configure", node: this._id,
      }, configuration));
      this._analysis = Object.assign({}, configuration);
    }
    get fftSize() { return this._analysis.fftSize; }
    set fftSize(value) {
      this._configure(Object.assign({}, this._analysis, { fftSize: Number(value) }));
    }
    get frequencyBinCount() { return this.fftSize / 2; }
    get minDecibels() { return this._analysis.minDecibels; }
    set minDecibels(value) {
      this._configure(Object.assign({}, this._analysis, { minDecibels: Number(value) }));
    }
    get maxDecibels() { return this._analysis.maxDecibels; }
    set maxDecibels(value) {
      this._configure(Object.assign({}, this._analysis, { maxDecibels: Number(value) }));
    }
    get smoothingTimeConstant() { return this._analysis.smoothing; }
    set smoothingTimeConstant(value) {
      this._configure(Object.assign({}, this._analysis, { smoothing: Number(value) }));
    }
    _read(target, kind, Constructor) {
      if (!(target instanceof Constructor)) throw new TypeError("incorrect analyser array type");
      const buffer = g.__chuzzAudioRead(this.context._id, this._id, kind);
      const values = new Constructor(buffer);
      target.set(values.subarray(0, target.length));
    }
    getByteFrequencyData(target) { this._read(target, "byteFrequency", Uint8Array); }
    getByteTimeDomainData(target) { this._read(target, "byteTime", Uint8Array); }
    getFloatFrequencyData(target) { this._read(target, "floatFrequency", Float32Array); }
    getFloatTimeDomainData(target) { this._read(target, "floatTime", Float32Array); }
  }

  class MediaStreamAudioSourceNode extends AudioNode {
    constructor(context, options = {}) {
      const stream = options.mediaStream;
      if (!stream || typeof stream.getAudioTracks !== "function") {
        throw new TypeError("expected a MediaStream");
      }
      const audio = stream.getAudioTracks().slice().sort((left, right) =>
        String(left.id) < String(right.id) ? -1 : String(left.id) > String(right.id) ? 1 : 0);
      if (!audio.length) throw exception("InvalidStateError", "stream has no audio track");
      if (trackIds.get(audio[0]) === undefined
          && typeof g.__chuzzMediaAttachAudioTrack === "function") {
        g.__chuzzMediaAttachAudioTrack(audio[0]);
      }
      const id = trackIds.get(audio[0]);
      if (id === undefined) {
        throw exception("NotSupportedError", "track has no native PCM registration");
      }
      const metadata = context._create("source", id);
      super(context, metadata.node, 0, 1);
      Object.defineProperty(this, "mediaStream", { value: stream, enumerable: true });
    }
  }

  class MediaStreamAudioDestinationNode extends AudioNode {
    constructor(context) {
      if (typeof g.__chuzzMediaCreateAudioStream !== "function") {
        throw exception("NotSupportedError", "native media adapter is unavailable");
      }
      const metadata = context._create("streamDestination");
      super(context, metadata.node, 1, 0);
      const stream = g.__chuzzMediaCreateAudioStream(metadata.audio);
      const audio = stream.getAudioTracks();
      if (audio.length !== 1 || trackIds.get(audio[0]) === undefined) {
        throw new TypeError("audio media adapter must produce one registered track");
      }
      Object.defineProperty(this, "stream", { value: stream, enumerable: true });
    }
  }

  class AudioContext extends AudioEvents {
    constructor(options = {}) {
      super();
      const sampleRate = options.sampleRate === undefined ? 48000 : finite(options.sampleRate, "sampleRate");
      if (!Number.isInteger(sampleRate)) throw exception("NotSupportedError", "sampleRate must be an integer");
      if (options.latencyHint !== undefined) {
        if (typeof options.latencyHint === "string") {
          if (!["interactive", "balanced", "playback"].includes(options.latencyHint)) {
            throw new TypeError("invalid latencyHint");
          }
        } else if (finite(options.latencyHint, "latencyHint") < 0) {
          throw new TypeError("invalid latencyHint");
        }
      }
      const metadata = call({ op: "createContext", sampleRate });
      this._id = metadata.context;
      this._nodes = new Map();
      this.onstatechange = null;
      this._sinkId = "";
      Object.defineProperties(this, {
        sampleRate: { value: metadata.sampleRate, enumerable: true },
        baseLatency: { value: 128 / metadata.sampleRate, enumerable: true },
        outputLatency: { value: 0, enumerable: true },
      });
      this.destination = new AudioNode(this, metadata.destination, 1, 0);
      contexts.set(this._id, this);
    }
    _call(command) {
      return call(Object.assign({ context: this._id }, command));
    }
    _create(kind, track = null) {
      return this._call({ op: "createNode", kind, track });
    }
    get state() { return this._call({ op: "inspect" }).state; }
    get currentTime() { return this._call({ op: "inspect" }).currentTime; }
    get sinkId() { return this._sinkId; }
    _state(state) {
      try {
        const previous = this.state;
        const result = this._call({ op: "state", state });
        if (previous !== result.state) {
          Promise.resolve().then(() => this._emit("statechange"));
        }
        if (state === "closed") {
          for (const id of result.endedTracks || []) {
            if (typeof g.__chuzzMediaAudioEnded === "function") g.__chuzzMediaAudioEnded(id);
          }
          contexts.delete(this._id);
          this._nodes.clear();
        }
        return Promise.resolve();
      } catch (error) {
        return Promise.reject(error);
      }
    }
    resume() { return this._state("running"); }
    suspend() { return this._state("suspended"); }
    close() { return this._state("closed"); }
    createOscillator() { return new OscillatorNode(this); }
    createGain() { return new GainNode(this); }
    createAnalyser() { return new AnalyserNode(this); }
    createMediaStreamSource(stream) {
      return new MediaStreamAudioSourceNode(this, { mediaStream: stream });
    }
    createMediaStreamDestination() { return new MediaStreamAudioDestinationNode(this); }
    setSinkId(id) {
      id = String(id);
      if (!["", "default", "chuzz-fake-output"].includes(id)) {
        return Promise.reject(exception("NotFoundError", "unknown audio output"));
      }
      if (this.state === "closed") {
        return Promise.reject(exception("InvalidStateError", "AudioContext is closed"));
      }
      this._sinkId = id;
      return Promise.resolve();
    }
  }

  g.__chuzzAudioEnded = function (context, node) {
    const owner = contexts.get(context);
    const oscillator = owner && owner._nodes.get(node);
    if (oscillator) oscillator._emit("ended");
  };
  g.__chuzzAudioInspect = function (context) {
    if (!(context instanceof AudioContext)) throw new TypeError("expected an AudioContext");
    return context._call({ op: "inspect" });
  };

  g.AudioContext = AudioContext;
  g.AudioNode = AudioNode;
  g.AudioParam = AudioParam;
  g.OscillatorNode = OscillatorNode;
  g.GainNode = GainNode;
  g.AnalyserNode = AnalyserNode;
  g.MediaStreamAudioSourceNode = MediaStreamAudioSourceNode;
  g.MediaStreamAudioDestinationNode = MediaStreamAudioDestinationNode;
})(globalThis);

