(function (g) {
  "use strict";
  const state = new WeakMap();
  class WorkerEvent {
    constructor(type, options = {}) {
      if (!arguments.length) throw new TypeError("Event requires a type");
      options = options || {};
      this.type = String(type);
      this.bubbles = !!options.bubbles;
      this.cancelable = !!options.cancelable;
      this.composed = !!options.composed;
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
  const EventBase = typeof g.Event === "function" ? g.Event : WorkerEvent;
  const messageState = new WeakMap();
  class WorkerMessageEvent extends EventBase {
    constructor(type, options = {}) {
      if (!arguments.length) throw new TypeError("MessageEvent requires a type");
      options = options || {};
      super(type, options);
      const source = options.source === undefined ? null : options.source;
      const windowSource = typeof g.Window === "function" && source instanceof g.Window;
      if (source !== null && !windowSource &&
          !(typeof g.MessagePort === "function" && source instanceof g.MessagePort)) {
        throw new TypeError("Invalid MessageEvent source");
      }
      const ports = options.ports === undefined ? [] : Array.from(options.ports);
      if (ports.some(port => !(typeof g.MessagePort === "function" && port instanceof g.MessagePort))) {
        throw new TypeError("MessageEvent ports require MessagePort entries");
      }
      messageState.set(this, {
        data: options.data === undefined ? null : options.data,
        origin: options.origin === undefined ? "" : String(options.origin),
        lastEventId: options.lastEventId === undefined ? "" : String(options.lastEventId),
        source,
        ports: Object.freeze(ports),
      });
    }
    get data() { return message(this).data; }
    get origin() { return message(this).origin; }
    get lastEventId() { return message(this).lastEventId; }
    get source() { return message(this).source; }
    get ports() { return message(this).ports; }
  }
  function message(event) {
    const value = messageState.get(event);
    if (!value) throw new TypeError("Illegal invocation");
    return value;
  }
  class WorkerErrorEvent extends EventBase {
    constructor(type, options = {}) {
      options = options || {};
      super(type, options);
      this.message = String(options.message || "");
      this.filename = String(options.filename || "");
      this.lineno = Number(options.lineno || 0);
      this.colno = Number(options.colno || 0);
      this.error = options.error === undefined ? null : options.error;
    }
  }
  const nativeTarget = typeof g.EventTarget === "function" ? g.EventTarget : null;
  function attach(target) {
    state.set(target, new Map());
    for (const type of ["message", "messageerror", "error"]) {
      if (!nativeTarget) {
        target["on" + type] = null;
        continue;
      }
      let handler = null;
      let registered = false;
      Object.defineProperty(target, "on" + type, {
        configurable: true,
        enumerable: true,
        get() { return handler; },
        set(value) {
          handler = typeof value === "function" ? value : null;
          if (handler && !registered) {
            registered = true;
            nativeTarget.prototype.addEventListener.call(target, type, event => {
              if (handler && handler.call(target, event) === true && type === "error") {
                event.preventDefault();
              }
            });
          }
        },
      });
    }
  }
  class WorkerTarget {
    constructor() { attach(this); }
    addEventListener(type, callback, options = {}) {
      if (callback == null) return;
      type = String(type);
      const capture = typeof options === "boolean" ? options : !!(options && options.capture);
      const listeners = state.get(this);
      if (!listeners) throw new TypeError("Illegal invocation");
      const entries = listeners.get(type) || [];
      if (entries.some(entry => entry.callback === callback && entry.capture === capture)) return;
      if (options && options.signal && options.signal.aborted) return;
      const entry = { callback, capture, once: !!(options && options.once) };
      entries.push(entry);
      listeners.set(type, entries);
      if (options && options.signal) {
        options.signal.addEventListener("abort", () => {
          this.removeEventListener(type, callback, capture);
        }, { once: true });
      }
    }
    removeEventListener(type, callback, options = {}) {
      const capture = typeof options === "boolean" ? options : !!(options && options.capture);
      const listeners = state.get(this);
      if (!listeners) throw new TypeError("Illegal invocation");
      const entries = listeners.get(String(type)) || [];
      listeners.set(String(type), entries.filter(entry =>
        entry.callback !== callback || entry.capture !== capture));
    }
    dispatchEvent(event) {
      if (!(event instanceof EventBase)) throw new TypeError("dispatchEvent requires an Event");
      if (!event.type || event.eventPhase !== 0) {
        throw new g.DOMException("Event is uninitialized or already being dispatched", "InvalidStateError");
      }
      const listeners = state.get(this);
      if (!listeners) throw new TypeError("Illegal invocation");
      event._stopped = false;
      event.target = this;
      event.currentTarget = this;
      event.eventPhase = 2;
      event.isTrusted = false;
      const report = error => {
        if (typeof g.__chuzzWorkerReport === "function") g.__chuzzWorkerReport(error);
        else setTimeout(() => { throw error; }, 0);
      };
      try {
        const handler = this["on" + event.type];
        if (typeof handler === "function") {
          try {
            if (handler.call(this, event) === true && event.type === "error") event.preventDefault();
          } catch (error) { report(error); }
        }
        for (const entry of (listeners.get(event.type) || []).slice()) {
          if (event._stopped) break;
          if (!(listeners.get(event.type) || []).includes(entry)) continue;
          if (entry.once) this.removeEventListener(event.type, entry.callback, entry.capture);
          try {
            if (typeof entry.callback === "function") entry.callback.call(this, event);
            else if (typeof entry.callback.handleEvent === "function") entry.callback.handleEvent(event);
          } catch (error) { report(error); }
        }
      } finally {
        event.currentTarget = null;
        event.eventPhase = 0;
        event._stopped = false;
      }
      return !event.defaultPrevented;
    }
  }
  class NativeWorkerTarget extends (nativeTarget || WorkerTarget) {
    constructor() {
      super();
      if (nativeTarget) attach(this);
    }
  }
  if (typeof g.Event !== "function") g.Event = WorkerEvent;
  if (typeof g.EventTarget !== "function") g.EventTarget = WorkerTarget;
  if (typeof g.MessageEvent !== "function") {
    Object.defineProperty(WorkerMessageEvent.prototype, Symbol.toStringTag, {
      value: "MessageEvent", configurable: true,
    });
    g.MessageEvent = WorkerMessageEvent;
  }
  if (typeof g.ErrorEvent !== "function") g.ErrorEvent = WorkerErrorEvent;
  Object.defineProperty(g, "__chuzzWorkerEvents", {
    value: Object.freeze({
      Target: NativeWorkerTarget, Message: g.MessageEvent, Error: g.ErrorEvent, attach,
    }),
  });
  Object.defineProperty(g, "__chuzzWorkerException", {
    value(name, message) { return new g.DOMException(message, name); },
  });
})(globalThis);

