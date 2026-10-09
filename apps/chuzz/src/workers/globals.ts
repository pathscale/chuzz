(function (g) {
  "use strict";
  const events = g.__chuzzWorkerEvents;
  const nativeSend = g.__chuzzWorkerSend;
  const nativeClose = g.__chuzzWorkerClose;
  const nativeNow = g.__chuzzWorkerNow;
  const nativeImport = g.__chuzzWorkerImport;
  const nativeFetch = g.__chuzzWorkerFetch;
  const timers = new Map();
  const pending = new Map();
  let nextTimer = 1;
  let nextRequest = 1;
  function publish(name, value) {
    Object.defineProperty(g, name, { value });
  }
  events.attach(g);
  // Bound: a classic script calls these bare (`addEventListener(...)`), where `this` is undefined in
  // strict code; the global scope's methods always act on the global.
  for (const name of ["addEventListener", "removeEventListener", "dispatchEvent"]) {
    g[name] = events.Target.prototype[name].bind(g);
  }
  g.self = g;
  g.name = "";
  g.location = null;
  g.performance = { now: nativeNow, timeOrigin: Date.now() };
  g.postMessage = function (value, options) {
    const transfers = Array.isArray(options) ? options : options && options.transfer;
    nativeSend(value, Array.from(transfers || []));
  };
  g.close = nativeClose;
  g.importScripts = function (...urls) { return nativeImport(...urls.map(String)); };
  function WorkerGlobalScope() { throw new TypeError("Illegal constructor"); }
  function DedicatedWorkerGlobalScope() { throw new TypeError("Illegal constructor"); }
  Object.defineProperty(WorkerGlobalScope, Symbol.hasInstance, {
    value(value) { return value === g; },
  });
  Object.defineProperty(DedicatedWorkerGlobalScope, Symbol.hasInstance, {
    value(value) { return value === g; },
  });
  g.WorkerGlobalScope = WorkerGlobalScope;
  g.DedicatedWorkerGlobalScope = DedicatedWorkerGlobalScope;
  publish("__chuzzWorkerInitialize", function (name, address) {
    g.name = name;
    const url = new URL(address);
    g.location = Object.freeze({
      href: url.href,
      origin: url.origin,
      protocol: url.protocol,
      host: url.host,
      hostname: url.hostname,
      port: url.port,
      pathname: url.pathname,
      search: url.search,
      hash: url.hash,
      toString() { return this.href; },
    });
  });
  publish("__chuzzWorkerMessage", function (data) {
    g.dispatchEvent(new events.Message("message", { data }));
  });
  publish("__chuzzWorkerMessageError", function () {
    g.dispatchEvent(new events.Message("messageerror"));
  });
  function schedule(callback, milliseconds, interval, args) {
    const id = nextTimer++;
    const numeric = Number(milliseconds);
    const delay = Number.isFinite(numeric) ? Math.max(0, Math.min(numeric, 2147483647)) : 0;
    timers.set(id, {
      callback,
      args,
      deadline: nativeNow() + delay,
      interval: interval ? Math.max(1, delay) : null,
    });
    return id;
  }
  g.setTimeout = (callback, delay, ...args) => schedule(callback, delay, false, args);
  g.setInterval = (callback, delay, ...args) => schedule(callback, delay, true, args);
  g.clearTimeout = g.clearInterval = id => { timers.delete(Number(id)); };
  g.queueMicrotask = callback => {
    if (typeof callback !== "function") throw new TypeError("callback must be a function");
    Promise.resolve().then(() => {
      try { callback(); } catch (error) { g.__chuzzWorkerReport(error); }
    });
  };
  publish("__chuzzWorkerNextTimer", function () {
    let deadline = Infinity;
    for (const timer of timers.values()) deadline = Math.min(deadline, timer.deadline);
    return Math.max(0, deadline - nativeNow());
  });
  publish("__chuzzWorkerRunTimer", function () {
    let chosen;
    let deadline = Infinity;
    const now = nativeNow();
    for (const [id, timer] of timers) {
      if (timer.deadline <= now && timer.deadline < deadline) {
        chosen = id;
        deadline = timer.deadline;
      }
    }
    if (chosen === undefined) return;
    const timer = timers.get(chosen);
    if (timer.interval === null) timers.delete(chosen);
    else timer.deadline = now + timer.interval;
    if (typeof timer.callback === "function") timer.callback.apply(g, timer.args);
    else (0, eval)(String(timer.callback));
  });

  class Headers {
    constructor(initial = []) {
      this._entries = new Map();
      if (initial instanceof Headers) initial = Array.from(initial);
      if (initial && !Array.isArray(initial) && typeof initial[Symbol.iterator] !== "function") {
        initial = Object.entries(initial);
      }
      for (const [name, value] of initial) this.append(name, value);
    }
    _name(name) {
      name = String(name).toLowerCase();
      if (!/^[!#$%&'*+\-.^_`|~0-9a-z]+$/.test(name)) throw new TypeError("invalid header name");
      return name;
    }
    append(name, value) {
      name = this._name(name);
      value = String(value).trim();
      if (/[\r\n\0]/.test(value)) throw new TypeError("invalid header value");
      const previous = this._entries.get(name);
      this._entries.set(name, previous === undefined ? value : previous + ", " + value);
    }
    set(name, value) {
      name = this._name(name);
      this.delete(name);
      this.append(name, value);
    }
    get(name) { return this._entries.get(this._name(name)) ?? null; }
    has(name) { return this._entries.has(this._name(name)); }
    delete(name) { this._entries.delete(this._name(name)); }
    entries() { return this._entries.entries(); }
    keys() { return this._entries.keys(); }
    values() { return this._entries.values(); }
    [Symbol.iterator]() { return this.entries(); }
    forEach(callback, receiver) {
      for (const [name, value] of this) callback.call(receiver, value, name, this);
    }
  }
  class Response {
    constructor(body = null, options = {}) {
      this._body = body === null ? new ArrayBuffer(0) : bodyBytes(body).buffer;
      this.status = options.status === undefined ? 200 : Number(options.status);
      this.statusText = String(options.statusText || "");
      this.headers = new Headers(options.headers || []);
      this.url = String(options.url || "");
      this.redirected = !!options.redirected;
      this.type = "basic";
      this.bodyUsed = false;
      this.ok = this.status >= 200 && this.status < 300;
    }
    _consume() {
      if (this.bodyUsed) return Promise.reject(new TypeError("body already consumed"));
      this.bodyUsed = true;
      return Promise.resolve(this._body);
    }
    arrayBuffer() { return this._consume(); }
    text() { return this._consume().then(buffer => new TextDecoder().decode(buffer)); }
    json() { return this.text().then(JSON.parse); }
    clone() {
      if (this.bodyUsed) throw new TypeError("body already consumed");
      return new Response(this._body.slice(0), this);
    }
  }
  function bodyBytes(body) {
    if (body instanceof ArrayBuffer) return new Uint8Array(body.slice(0));
    if (ArrayBuffer.isView(body)) {
      return new Uint8Array(body.buffer, body.byteOffset, body.byteLength).slice();
    }
    return new TextEncoder().encode(String(body));
  }
  g.Headers = Headers;
  g.Response = Response;
  g.fetch = function (input, options = {}) {
    return new Promise((resolve, reject) => {
      let removeAbort = () => {};
      try {
        if (options == null) options = {};
        const id = nextRequest++;
        const address = input && input.url !== undefined ? input.url : String(input);
        const method = String(options.method || input && input.method || "GET").toUpperCase();
        const headers = new Headers(options.headers || input && input.headers || []);
        const body = options.body == null ? null : bodyBytes(options.body).buffer;
        if ((method === "GET" || method === "HEAD") && body !== null) {
          throw new TypeError("GET and HEAD requests cannot have a body");
        }
        const signal = options.signal;
        if (signal && signal.aborted) {
          reject(signal.reason || g.__chuzzWorkerException("AbortError", "aborted"));
          return;
        }
        if (signal) {
          const abort = () => {
            pending.delete(id);
            removeAbort();
            reject(signal.reason || g.__chuzzWorkerException("AbortError", "aborted"));
          };
          signal.addEventListener("abort", abort, { once: true });
          removeAbort = () => signal.removeEventListener("abort", abort);
        }
        pending.set(id, { resolve, reject, removeAbort });
        try {
          nativeFetch(JSON.stringify({
            id, url: String(address), method, headers: Array.from(headers),
          }), body);
        } catch (error) {
          pending.delete(id);
          removeAbort();
          reject(error);
        }
      } catch (error) {
        removeAbort();
        reject(error);
      }
    });
  };
  publish("__chuzzWorkerFetchResult", function (id, description, body) {
    const entry = pending.get(id);
    if (!entry) return;
    pending.delete(id);
    entry.removeAbort();
    const metadata = JSON.parse(description);
    if (!metadata.ok) {
      entry.reject(new TypeError(metadata.error));
      return;
    }
    // The native response allocation is already owned by this context.
    const response = Object.create(Response.prototype);
    response._body = body;
    response.bodyUsed = false;
    response.status = metadata.status;
    response.statusText = metadata.statusText;
    response.url = metadata.url;
    response.headers = new Headers(metadata.headers);
    response.ok = response.status >= 200 && response.status < 300;
    response.redirected = false;
    response.type = "basic";
    entry.resolve(response);
  });
})(globalThis);

