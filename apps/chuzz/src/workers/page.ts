(function (g) {
  "use strict";
  const events = g.__chuzzWorkerEvents;
  const create = g.__chuzzWorkerCreate;
  const post = g.__chuzzWorkerPost;
  const terminate = g.__chuzzWorkerTerminate;
  const workers = new Map();
  const ids = new WeakMap();
  class Worker extends events.Target {
    constructor(url, options = {}) {
      super();
      if (options == null) options = {};
      const type = options.type === undefined ? "classic" : String(options.type);
      if (type !== "classic" && type !== "module") throw new TypeError("invalid Worker type");
      const name = options.name === undefined ? "" : String(options.name);
      const id = create(String(url), type, name);
      ids.set(this, id);
      workers.set(id, this);
    }
    postMessage(value, options) {
      const id = ids.get(this);
      if (id === undefined) throw new TypeError("invalid Worker receiver");
      const transfers = Array.isArray(options) ? options : options && options.transfer;
      post(id, value, Array.from(transfers || []));
    }
    terminate() {
      const id = ids.get(this);
      if (id === undefined) throw new TypeError("invalid Worker receiver");
      terminate(id);
      workers.delete(id);
    }
  }
  Object.defineProperty(g, "Worker", { value: Worker, writable: true, configurable: true });
  Object.defineProperty(g, "__chuzzWorkerForget", {
    value(id) { workers.delete(id); },
  });
  Object.defineProperty(g, "__chuzzWorkerDeliver", {
    value(id, type, payload) {
      const worker = workers.get(id);
      if (!worker) return;
      let event;
      if (type === "error") {
        const error = new Error(payload.message);
        error.name = payload.name || "Error";
        error.stack = payload.stack || "";
        event = new events.Error("error", { ...payload, error, cancelable: true });
      } else {
        event = new events.Message(type, { data: payload });
      }
      worker.dispatchEvent(event);
    },
  });
})(globalThis);

