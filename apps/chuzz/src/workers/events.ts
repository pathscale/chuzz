(function (g) {
  "use strict";
  const state = new WeakMap();
  class WorkerEvent {
    constructor(type, options = {}) {
      this.type = String(type);
      this.bubbles = false;
      this.cancelable = !!options.cancelable;
      this.defaultPrevented = false;
      this.target = null;
      this.currentTarget = null;
      this.eventPhase = 0;
      this.isTrusted = false;
      this.timeStamp = 0;
      this._stopped = false;
    }
    preventDefault() {
      if (this.cancelable) this.defaultPrevented = true;
    }
    stopPropagation() {}
    stopImmediatePropagation() { this._stopped = true; }
  }
  class WorkerMessageEvent extends WorkerEvent {
    constructor(type, options = {}) {
      super(type, options);
      this.data = options.data;
      this.origin = "";
      this.lastEventId = "";
      this.source = null;
      this.ports = [];
    }
  }
  class WorkerErrorEvent extends WorkerEvent {
    constructor(type, options = {}) {
      super(type, options);
      this.message = String(options.message || "");
      this.filename = String(options.filename || "");
      this.lineno = Number(options.lineno || 0);
      this.colno = Number(options.colno || 0);
      this.error = options.error === undefined ? null : options.error;
    }
  }
  function attach(target) {
    state.set(target, new Map());
    target.onmessage = null;
    target.onmessageerror = null;
    target.onerror = null;
  }
  class WorkerTarget {
    constructor() { attach(this); }
    addEventListener(type, callback, options = {}) {
      if (callback == null) return;
      type = String(type);
      const capture = typeof options === "boolean" ? options : !!options.capture;
      const listeners = state.get(this);
      const entries = listeners.get(type) || [];
      if (entries.some(entry => entry.callback === callback && entry.capture === capture)) return;
      if (options && options.signal && options.signal.aborted) return;
      const entry = { callback, capture, once: !!options.once };
      entries.push(entry);
      listeners.set(type, entries);
      if (options && options.signal) {
        options.signal.addEventListener("abort", () => {
          this.removeEventListener(type, callback, capture);
        }, { once: true });
      }
    }
    removeEventListener(type, callback, options = {}) {
      const capture = typeof options === "boolean" ? options : !!options.capture;
      const listeners = state.get(this);
      const entries = listeners.get(String(type)) || [];
      listeners.set(String(type), entries.filter(entry =>
        entry.callback !== callback || entry.capture !== capture));
    }
    dispatchEvent(event) {
      event.target = this;
      event.currentTarget = this;
      event.eventPhase = 2;
      const report = error => {
        if (typeof g.__chuzzWorkerReport === "function") {
          g.__chuzzWorkerReport(error);
        } else {
          setTimeout(() => { throw error; }, 0);
        }
      };
      const handler = this["on" + event.type];
      if (typeof handler === "function") {
        try {
          if (handler.call(this, event) === true && event.type === "error") event.preventDefault();
        } catch (error) { report(error); }
      }
      const listeners = state.get(this);
      for (const entry of (listeners.get(event.type) || []).slice()) {
        if (event._stopped) break;
        if (!(listeners.get(event.type) || []).includes(entry)) continue;
        if (entry.once) this.removeEventListener(event.type, entry.callback, entry.capture);
        try {
          if (typeof entry.callback === "function") entry.callback.call(this, event);
          else if (typeof entry.callback.handleEvent === "function") entry.callback.handleEvent(event);
        } catch (error) { report(error); }
      }
      event.currentTarget = null;
      event.eventPhase = 0;
      return !event.defaultPrevented;
    }
  }
  Object.defineProperty(g, "__chuzzWorkerEvents", {
    value: Object.freeze({
      Target: WorkerTarget, Message: WorkerMessageEvent, Error: WorkerErrorEvent, attach,
    }),
  });
  Object.defineProperty(g, "__chuzzWorkerException", {
    value(name, message) {
      if (typeof g.DOMException === "function") return new g.DOMException(message, name);
      const error = new Error(message);
      error.name = name;
      return error;
    },
  });
  // A document has the engine's native Event, whose dispatch accepts only real
  // events: MessageEvent there is a subclass of it. A worker realm keeps the
  // script event class above.
  if (typeof g.MessageEvent !== "function" && typeof g.Event === "function" && typeof g.document === "object") {
    g.MessageEvent = class MessageEvent extends g.Event {
      constructor(type, init) {
        super(type, init);
        const options = init || {};
        Object.defineProperties(this, {
          data: { configurable: true, enumerable: true, value: options.data === undefined ? null : options.data },
          origin: { configurable: true, enumerable: true, value: options.origin === undefined ? "" : String(options.origin) },
          lastEventId: { configurable: true, enumerable: true, value: options.lastEventId === undefined ? "" : String(options.lastEventId) },
          source: { configurable: true, enumerable: true, value: options.source === undefined ? null : options.source },
          ports: { configurable: true, enumerable: true, value: Object.freeze(Array.from(options.ports || [])) },
        });
      }
    };
  }
  if (typeof g.ErrorEvent !== "function" && typeof g.Event === "function" && typeof g.document === "object") {
    g.ErrorEvent = class ErrorEvent extends g.Event {
      constructor(type, init) {
        super(type, init);
        const options = init || {};
        const field = value => ({ configurable: true, enumerable: true, value });
        Object.defineProperties(this, {
          message: field(String(options.message || "")),
          filename: field(String(options.filename || "")),
          lineno: field(Number(options.lineno || 0)),
          colno: field(Number(options.colno || 0)),
          error: field(options.error === undefined ? null : options.error),
        });
      }
    };
  }
  if (typeof g.MessageEvent !== "function") g.MessageEvent = WorkerMessageEvent;
  if (typeof g.ErrorEvent !== "function") g.ErrorEvent = WorkerErrorEvent;
})(globalThis);

