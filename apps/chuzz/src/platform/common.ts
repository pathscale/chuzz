// Executed in both document and worker realms.
(function (g) {
  "use strict";
  const native = g.__chuzzPlatformCall;
  const call = value => native(JSON.stringify(value));
  const info = call({ op: "info" });
  function exception(name, message) {
    if (typeof g.DOMException === "function") return new g.DOMException(message, name);
    const error = new Error(message);
    error.name = name;
    return error;
  }
  const listeners = new WeakMap();
  class Target {
    constructor() { listeners.set(this, new Map()); }
    addEventListener(type, listener, options = {}) {
      if (listener == null) return;
      type = String(type);
      const capture = typeof options === "boolean" ? options : !!(options && options.capture);
      const entries = listeners.get(this).get(type) || [];
      if (entries.some(entry => entry.listener === listener && entry.capture === capture)) return;
      if (options && options.signal && options.signal.aborted) return;
      const entry = { listener, capture, once: !!(options && options.once) };
      entries.push(entry);
      listeners.get(this).set(type, entries);
      if (options && options.signal) {
        options.signal.addEventListener("abort",
          () => this.removeEventListener(type, listener, capture), { once: true });
      }
    }
    removeEventListener(type, listener, options = {}) {
      const capture = typeof options === "boolean" ? options : !!(options && options.capture);
      const map = listeners.get(this);
      type = String(type);
      map.set(type, (map.get(type) || []).filter(entry =>
        entry.listener !== listener || entry.capture !== capture));
    }
    dispatchEvent(event) {
      if (!event || !event.type) throw new TypeError("An event is required");
      Object.defineProperties(event, {
        target: { configurable: true, value: this },
        currentTarget: { configurable: true, value: this }
      });
      const report = error => {
        if (g.console) g.console.error(error);
      };
      const direct = this["on" + event.type];
      if (typeof direct === "function") {
        try { direct.call(this, event); } catch (error) { report(error); }
      }
      const entries = listeners.get(this).get(event.type) || [];
      for (const entry of entries.slice()) {
        if (!(listeners.get(this).get(event.type) || []).includes(entry)) continue;
        if (entry.once) this.removeEventListener(event.type, entry.listener, entry.capture);
        try {
          if (typeof entry.listener === "function") entry.listener.call(this, event);
          else entry.listener.handleEvent(event);
        } catch (error) { report(error); }
      }
      Object.defineProperty(event, "currentTarget", { configurable: true, value: null });
      return !event.defaultPrevented;
    }
  }
  if (typeof g.EventTarget === "function") {
    Object.setPrototypeOf(Target.prototype, g.EventTarget.prototype);
  }
  function event(type, values = {}) {
    const result = new g.Event(type);
    for (const name of Object.keys(values)) {
      Object.defineProperty(result, name, {
        configurable: true, enumerable: true, value: values[name]
      });
    }
    return result;
  }
  function message(data, origin, source) {
    const result = new g.MessageEvent("message", { data });
    Object.defineProperties(result, {
      origin: { configurable: true, enumerable: true, value: origin },
      source: { configurable: true, enumerable: true, value: source }
    });
    return result;
  }
  const channels = new Map();
  let broadcastTimer = null;
  function readBroadcasts() {
    for (const delivery of call({ op: "broadcast-read" })) {
      const channel = channels.get(delivery.id);
      if (!channel) continue;
      try {
        const envelope = JSON.parse(delivery.wire);
        const buffers = envelope[1].map(entry => {
          const buffer = new Uint8Array(entry[0]).buffer;
          return entry[1] < 0 ? buffer
            : buffer.transferToFixedLength
              ? new ArrayBuffer(buffer.byteLength, { maxByteLength: entry[1] })
              : buffer;
        });
        envelope[1].forEach((entry, index) => {
          new Uint8Array(buffers[index]).set(entry[0]);
        });
        const data = g.__chuzzCloneDecode(envelope[0], buffers);
        channel.dispatchEvent(message(data, info.origin, null));
      } catch (error) {
        channel.dispatchEvent(new g.MessageEvent("messageerror"));
      }
    }
  }
  const channelState = new WeakMap();
  class BroadcastChannel extends Target {
    constructor(name) {
      super();
      name = String(name);
      Object.defineProperty(this, "name", { enumerable: true, value: name });
      const id = call({ op: "broadcast-open", name });
      channelState.set(this, id);
      channels.set(id, this);
      this.onmessage = this.onmessageerror = null;
      if (broadcastTimer === null) broadcastTimer = g.setInterval(readBroadcasts, 20);
    }
    postMessage(value) {
      const id = channelState.get(this);
      if (id === null) throw exception("InvalidStateError", "Broadcast channel is closed");
      const [graph, buffers] = g.__chuzzCloneEncode(value, []);
      const envelope = [graph, buffers.map(buffer => [
        Array.from(new Uint8Array(buffer)), g.__chuzzBufferMaximum(buffer)
      ])];
      call({ op: "broadcast-post", id, wire: JSON.stringify(envelope) });
    }
    close() {
      const id = channelState.get(this);
      if (id === null) return;
      call({ op: "broadcast-close", id });
      channels.delete(id);
      channelState.set(this, null);
      if (!channels.size && broadcastTimer !== null) {
        g.clearInterval(broadcastTimer);
        broadcastTimer = null;
      }
    }
  }
  Object.defineProperty(BroadcastChannel.prototype, Symbol.toStringTag, {
    value: "BroadcastChannel"
  });
  g.BroadcastChannel = BroadcastChannel;

  const navigator = g.navigator || (g.navigator = {});
  const properties = {
    platform: info.platform, cookieEnabled: true, onLine: true,
    hardwareConcurrency: info.concurrency, maxTouchPoints: 0, vendor: "Google Inc.",
    webdriver: false, pdfViewerEnabled: false, doNotTrack: null
  };
  for (const name of Object.keys(properties)) {
    Object.defineProperty(navigator, name, {
      configurable: true, enumerable: true, get: () => properties[name]
    });
  }
  function emptyList() {
    const result = [];
    Object.defineProperties(result, {
      item: { value: () => null },
      namedItem: { value: () => null },
      refresh: { value: () => {} }
    });
    return Object.freeze(result);
  }
  navigator.plugins = emptyList();
  navigator.mimeTypes = emptyList();
  navigator.javaEnabled = () => false;
  const brands = [{ brand: "Chuzz", version: info.version.split(".")[0] }];
  navigator.userAgentData = {
    get brands() { return brands.map(brand => ({ ...brand })); },
    mobile: false,
    platform: info.uaPlatform,
    toJSON() { return { brands: this.brands, mobile: false, platform: info.uaPlatform }; },
    getHighEntropyValues(hints) {
      return Promise.resolve().then(() => {
        const result = this.toJSON();
        const values = {
          architecture: info.architecture, bitness: info.bitness,
          fullVersionList: [{ brand: "Chuzz", version: info.version }],
          uaFullVersion: info.version, wow64: false
        };
        for (const hint of hints) {
          if (Object.prototype.hasOwnProperty.call(values, String(hint))) {
            result[String(hint)] = values[String(hint)];
          }
        }
        return result;
      });
    }
  };
  navigator.locks = {
    query() { return Promise.resolve(call({ op: "lock-query" })); },
    request(name, options, callback) {
      if (typeof options === "function") { callback = options; options = {}; }
      options = options || {};
      return new Promise((resolve, reject) => {
        name = String(name);
        if (typeof callback !== "function") throw new TypeError("A lock callback is required");
        if (name.startsWith("-")) throw exception("NotSupportedError", "Reserved lock name");
        const mode = options.mode === undefined ? "exclusive" : String(options.mode);
        if (mode !== "exclusive" && mode !== "shared") throw new TypeError("Invalid lock mode");
        if (options.steal) throw exception("NotSupportedError", "Lock stealing is unavailable");
        const signal = options.signal;
        if (signal && options.ifAvailable) throw exception("NotSupportedError", "Invalid lock options");
        if (signal && signal.aborted) { reject(signal.reason); return; }
        const id = call({
          op: "lock-request", name, shared: mode === "shared", available: !!options.ifAvailable
        });
        if (id === null) {
          Promise.resolve().then(() => callback(null)).then(resolve, reject);
          return;
        }
        let timer = null;
        let started = false;
        function release() {
          if (timer !== null) g.clearTimeout(timer);
          if (signal) signal.removeEventListener("abort", abort);
          call({ op: "lock-release", id });
        }
        function abort() {
          if (started) return;
          release();
          reject(signal.reason);
        }
        function check() {
          if (signal && signal.aborted) { abort(); return; }
          if (!call({ op: "lock-status", id })) {
            timer = g.setTimeout(check, 10);
            return;
          }
          started = true;
          if (signal) signal.removeEventListener("abort", abort);
          const lease = Object.freeze({ name, mode });
          Promise.resolve().then(() => callback(lease)).then(
            value => { release(); resolve(value); },
            error => { release(); reject(error); }
          );
        }
        if (signal) signal.addEventListener("abort", abort, { once: true });
        check();
      });
    }
  };
  if (!g.crypto) g.crypto = {};
  if (typeof g.crypto.getRandomValues !== "function") {
    g.crypto.getRandomValues = function (array) {
      if (!ArrayBuffer.isView(array)
          || !/^(?:Uint|Int|BigInt|BigUint)(?:8|16|32|64)(?:Clamped)?Array$/.test(
            array.constructor.name)) {
        throw new TypeError("An integer TypedArray is required");
      }
      if (array.byteLength > 65536) throw exception("QuotaExceededError", "Random quota exceeded");
      const bytes = new Uint8Array(array.buffer, array.byteOffset, array.byteLength);
      for (let offset = 0; offset < bytes.length; offset += 16) {
        bytes.set(call({ op: "random" }).slice(0, Math.min(16, bytes.length - offset)), offset);
      }
      return array;
    };
  }
  if (info.secure && typeof g.crypto.randomUUID !== "function") {
    g.crypto.randomUUID = function () {
      const bytes = g.crypto.getRandomValues(new Uint8Array(16));
      bytes[6] = (bytes[6] & 15) | 64;
      bytes[8] = (bytes[8] & 63) | 128;
      const hex = Array.from(bytes, value => value.toString(16).padStart(2, "0")).join("");
      return [hex.slice(0, 8), hex.slice(8, 12), hex.slice(12, 16),
        hex.slice(16, 20), hex.slice(20)].join("-");
    };
  }
  Object.defineProperty(g, "__chuzzPlatform", {
    value: Object.freeze({ Target, exception, event, message, info })
  });
})(globalThis);

