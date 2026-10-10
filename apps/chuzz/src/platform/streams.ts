// Default streams. Byte/BYOB sources are explicitly rejected.
(function (g) {
  "use strict";
  function deferred() {
    let resolve, reject;
    const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
    promise.catch(() => {});
    return { promise, resolve, reject };
  }
  function highWaterMark(strategy, fallback) {
    const value = strategy.highWaterMark === undefined ? fallback : Number(strategy.highWaterMark);
    if (Number.isNaN(value) || value < 0) throw new RangeError("Invalid highWaterMark");
    return value;
  }
  const readable = new WeakMap();
  const readerState = new WeakMap();
  function finish(state) {
    if (state.status !== "readable") return;
    state.status = "closed";
    state.closed.resolve();
    for (const entry of state.reads.splice(0)) entry.resolve({ value: undefined, done: true });
  }
  function fail(state, error) {
    if (state.status !== "readable") return;
    state.status = "errored";
    state.error = error;
    state.queue = [];
    state.total = 0;
    state.closed.reject(error);
    for (const entry of state.reads.splice(0)) entry.reject(error);
  }
  function pull(state) {
    if (!state.started || state.status !== "readable" || state.closing
        || (!state.reads.length && state.hwm - state.total <= 0)) return;
    if (state.pulling) { state.again = true; return; }
    state.pulling = true;
    Promise.resolve().then(() => {
      if (state.source.pull) return state.source.pull(state.controller);
    }).then(() => {
      state.pulling = false;
      if (state.again) { state.again = false; pull(state); }
    }, error => { state.pulling = false; fail(state, error); });
  }
  class ReadableStreamDefaultController {
    constructor() { throw new TypeError("Illegal constructor"); }
    get desiredSize() {
      const state = this._state;
      return state.status === "errored" ? null
        : state.status === "closed" ? 0 : state.hwm - state.total;
    }
    enqueue(value) {
      const state = this._state;
      if (state.status !== "readable" || state.closing) throw new TypeError("Stream is not writable");
      if (state.reads.length) {
        state.reads.shift().resolve({ value, done: false });
      } else {
        try {
          const size = Number(state.size(value));
          if (!Number.isFinite(size) || size < 0) throw new RangeError("Invalid chunk size");
          state.queue.push({ value, size });
          state.total += size;
        } catch (error) { fail(state, error); throw error; }
      }
      pull(state);
    }
    close() {
      const state = this._state;
      if (state.status !== "readable" || state.closing) throw new TypeError("Stream is not readable");
      state.closing = true;
      if (!state.queue.length) finish(state);
    }
    error(error) { fail(this._state, error); }
  }
  function cancel(state, reason) {
    state.disturbed = true;
    if (state.status === "closed") return Promise.resolve();
    if (state.status === "errored") return Promise.reject(state.error);
    state.queue = [];
    state.total = 0;
    finish(state);
    return Promise.resolve().then(() => state.source.cancel && state.source.cancel(reason));
  }
  class ReadableStreamDefaultReader {
    constructor(stream) {
      const state = readable.get(stream);
      if (!state || state.reader) throw new TypeError("Stream is locked or invalid");
      state.reader = this;
      const closed = deferred();
      state.closed.promise.then(closed.resolve, closed.reject);
      readerState.set(this, { state, closed });
    }
    get closed() { return readerState.get(this).closed.promise; }
    read() {
      const entry = readerState.get(this);
      if (!entry.state) return Promise.reject(new TypeError("Reader has been released"));
      const state = entry.state;
      state.disturbed = true;
      if (state.queue.length) {
        const chunk = state.queue.shift();
        state.total -= chunk.size;
        if (state.closing && !state.queue.length) finish(state);
        else pull(state);
        return Promise.resolve({ value: chunk.value, done: false });
      }
      if (state.status === "closed") return Promise.resolve({ value: undefined, done: true });
      if (state.status === "errored") return Promise.reject(state.error);
      const request = deferred();
      state.reads.push(request);
      pull(state);
      return request.promise;
    }
    cancel(reason) {
      const state = readerState.get(this).state;
      return state ? cancel(state, reason) : Promise.reject(new TypeError("Reader has been released"));
    }
    releaseLock() {
      const entry = readerState.get(this);
      if (!entry.state) return;
      const error = new TypeError("Reader has been released");
      for (const request of entry.state.reads.splice(0)) request.reject(error);
      entry.state.reader = null;
      entry.state = null;
      entry.closed.reject(error);
      entry.closed = deferred();
      entry.closed.reject(error);
    }
  }
  function tee(stream, cloneBytes) {
    const reader = stream.getReader();
    const canceled = [false, false];
    const reasons = [];
    const controllers = [];
    const canceledPromise = deferred();
    let reading = false;
    let again = false;
    let finished = false;
    function next() {
      if (finished) return;
      if (reading) { again = true; return; }
      reading = true;
      reader.read().then(result => {
        reading = false;
        if (result.done) {
          controllers.forEach((controller, index) => {
            if (!canceled[index]) controller.close();
          });
          canceledPromise.resolve();
          finished = true;
          reader.releaseLock();
          return;
        }
        controllers.forEach((controller, index) => {
          if (!canceled[index]) {
            const value = cloneBytes && index === 1 && ArrayBuffer.isView(result.value)
              ? new Uint8Array(result.value.buffer,
                result.value.byteOffset, result.value.byteLength).slice() : result.value;
            controller.enqueue(value);
          }
        });
        if (again) { again = false; next(); }
      }, error => {
        controllers.forEach((controller, index) => {
          if (!canceled[index]) controller.error(error);
        });
        canceledPromise.resolve();
        finished = true;
        reader.releaseLock();
      });
    }
    return [0, 1].map(index => new ReadableStream({
      start(controller) { controllers[index] = controller; },
      pull: next,
      cancel(reason) {
        canceled[index] = true;
        reasons[index] = reason;
        if (canceled[0] && canceled[1]) {
          reader.cancel(reasons).then(canceledPromise.resolve, canceledPromise.reject);
        }
        return canceledPromise.promise;
      }
    }));
  }
  class ReadableStream {
    constructor(source = {}, strategy = {}) {
      if (source.type !== undefined) throw new TypeError("Byte/BYOB streams are unavailable");
      const state = {
        source, size: strategy.size || (() => 1), hwm: highWaterMark(strategy, 1),
        queue: [], total: 0, reads: [], reader: null, status: "readable",
        closing: false, started: false, pulling: false, again: false,
        disturbed: false, closed: deferred()
      };
      readable.set(this, state);
      const controller = Object.create(ReadableStreamDefaultController.prototype);
      controller._state = state;
      state.controller = controller;
      let start;
      try { start = source.start && source.start(controller); }
      catch (error) { fail(state, error); start = undefined; }
      Promise.resolve(start).then(() => { state.started = true; pull(state); },
        error => fail(state, error));
    }
    get locked() { return !!readable.get(this).reader; }
    getReader(options = {}) {
      if (options.mode !== undefined) throw new TypeError("BYOB readers are unavailable");
      return new ReadableStreamDefaultReader(this);
    }
    cancel(reason) {
      return this.locked ? Promise.reject(new TypeError("Stream is locked"))
        : cancel(readable.get(this), reason);
    }
    tee() { return tee(this, false); }
    async pipeTo(destination, options = {}) {
      if (!(destination instanceof WritableStream)) throw new TypeError("Invalid destination");
      const reader = this.getReader();
      let writer;
      try { writer = destination.getWriter(); }
      catch (error) { reader.releaseLock(); throw error; }
      const interruption = deferred();
      const signal = options.signal;
      const abort = () => interruption.reject(signal.reason);
      writer.closed.then(
        () => interruption.reject(new TypeError("Destination closed before piping completed")),
        interruption.reject
      );
      if (signal) {
        if (signal.aborted) abort();
        else signal.addEventListener("abort", abort, { once: true });
      }
      let sourceDone = false;
      try {
        while (true) {
          await Promise.race([writer.ready, interruption.promise]);
          const result = await Promise.race([reader.read(), interruption.promise]);
          if (result.done) { sourceDone = true; break; }
          await Promise.race([writer.write(result.value), interruption.promise]);
        }
        if (!options.preventClose) await writer.close();
      } catch (error) {
        const cleanup = [];
        if (!options.preventAbort) cleanup.push(writer.abort(error));
        if (!options.preventCancel && !sourceDone) cleanup.push(reader.cancel(error));
        await Promise.allSettled(cleanup);
        throw error;
      } finally {
        if (signal) signal.removeEventListener("abort", abort);
        reader.releaseLock();
        writer.releaseLock();
      }
    }
    pipeThrough(pair, options = {}) {
      if (!pair || !(pair.readable instanceof ReadableStream)
          || !(pair.writable instanceof WritableStream)) throw new TypeError("Invalid transform");
      if (this.locked || pair.writable.locked) throw new TypeError("Stream is locked");
      this.pipeTo(pair.writable, options).catch(() => {});
      return pair.readable;
    }
    values(options = {}) {
      const reader = this.getReader();
      let finished = false;
      let chain = Promise.resolve();
      const iterator = {
        next() {
          chain = chain.then(async () => {
            if (finished) return { done: true, value: undefined };
            try {
              const result = await reader.read();
              if (result.done) { finished = true; reader.releaseLock(); }
              return result;
            } catch (error) { finished = true; reader.releaseLock(); throw error; }
          });
          return chain;
        },
        return(value) {
          chain = chain.catch(() => {}).then(async () => {
            if (!finished) {
              finished = true;
              try { if (!options.preventCancel) await reader.cancel(value); }
              finally { reader.releaseLock(); }
            }
            return { value, done: true };
          });
          return chain;
        },
        [Symbol.asyncIterator]() { return this; }
      };
      return iterator;
    }
    [Symbol.asyncIterator]() { return this.values(); }
  }

  const writable = new WeakMap();
  const writerState = new WeakMap();
  function pressure(state) {
    const blocked = state.status === "writable" && state.total >= state.hwm;
    if (blocked && !state.blocked) state.ready = deferred();
    if (!blocked) state.ready.resolve();
    state.blocked = blocked;
  }
  function writeError(state, error) {
    if (state.status === "closed" || state.status === "errored") return;
    state.status = "errored";
    state.error = error;
    state.ready.reject(error);
    state.closed.reject(error);
    if (state.abortController) state.abortController.abort(error);
  }
  class WritableStreamDefaultController {
    constructor() { throw new TypeError("Illegal constructor"); }
    error(error) { writeError(this._state, error); }
    get signal() { return this._state.abortController && this._state.abortController.signal; }
  }
  function abortWrite(state, reason) {
    if (state.status === "closed") return Promise.resolve();
    if (state.status === "errored") return Promise.resolve();
    writeError(state, reason);
    return state.tail.catch(() => {}).then(() => state.sink.abort && state.sink.abort(reason));
  }
  function write(state, value) {
    if (state.status !== "writable" || state.closing) {
      return Promise.reject(state.error || new TypeError("Stream is not writable"));
    }
    let size;
    try {
      size = Number(state.size(value));
      if (!Number.isFinite(size) || size < 0) throw new RangeError("Invalid chunk size");
    } catch (error) { writeError(state, error); return Promise.reject(error); }
    state.total += size;
    pressure(state);
    const operation = state.tail.then(() => {
      if (state.status === "errored") throw state.error;
      return state.sink.write && state.sink.write(value, state.controller);
    }).then(() => {
      if (state.status === "errored") throw state.error;
    });
    state.tail = operation.then(() => {
      state.total -= size;
      pressure(state);
    }, error => {
      state.total -= size;
      writeError(state, error);
      throw error;
    });
    state.tail.catch(() => {});
    return operation;
  }
  function closeWrite(state) {
    if (state.status !== "writable" || state.closing) {
      return Promise.reject(state.error || new TypeError("Stream cannot be closed"));
    }
    state.closing = true;
    state.ready.resolve();
    const operation = state.tail.then(() => {
      if (state.status === "errored") throw state.error;
      return state.sink.close && state.sink.close();
    }).then(() => {
      if (state.status === "errored") throw state.error;
      state.status = "closed";
      state.closed.resolve();
    }, error => { writeError(state, error); throw error; });
    state.tail = operation;
    state.tail.catch(() => {});
    return operation;
  }
  class WritableStreamDefaultWriter {
    constructor(stream) {
      const state = writable.get(stream);
      if (!state || state.writer) throw new TypeError("Stream is locked or invalid");
      state.writer = this;
      writerState.set(this, state);
    }
    get closed() {
      const state = writerState.get(this);
      return state ? state.closed.promise : Promise.reject(new TypeError("Writer has been released"));
    }
    get ready() {
      const state = writerState.get(this);
      return state ? state.ready.promise : Promise.reject(new TypeError("Writer has been released"));
    }
    get desiredSize() {
      const state = writerState.get(this);
      if (!state) throw new TypeError("Writer has been released");
      return state.status === "errored" ? null : state.status === "closed" ? 0
        : state.hwm - state.total;
    }
    write(value) {
      const state = writerState.get(this);
      return state ? write(state, value) : Promise.reject(new TypeError("Writer has been released"));
    }
    close() {
      const state = writerState.get(this);
      return state ? closeWrite(state) : Promise.reject(new TypeError("Writer has been released"));
    }
    abort(reason) {
      const state = writerState.get(this);
      return state ? abortWrite(state, reason) : Promise.reject(new TypeError("Writer has been released"));
    }
    releaseLock() {
      const state = writerState.get(this);
      if (!state) return;
      state.writer = null;
      writerState.set(this, null);
    }
  }
  class WritableStream {
    constructor(sink = {}, strategy = {}) {
      if (sink.type !== undefined) throw new RangeError("Invalid sink type");
      const state = {
        sink, size: strategy.size || (() => 1), hwm: highWaterMark(strategy, 1),
        total: 0, writer: null, status: "writable", closing: false, blocked: false,
        ready: deferred(), closed: deferred(),
        abortController: typeof g.AbortController === "function" ? new g.AbortController() : null
      };
      writable.set(this, state);
      const controller = Object.create(WritableStreamDefaultController.prototype);
      controller._state = state;
      state.controller = controller;
      let start;
      try { start = sink.start && sink.start(controller); }
      catch (error) { start = Promise.reject(error); }
      state.tail = Promise.resolve(start).catch(error => {
        writeError(state, error);
        throw error;
      });
      state.tail.catch(() => {});
      pressure(state);
    }
    get locked() { return !!writable.get(this).writer; }
    getWriter() { return new WritableStreamDefaultWriter(this); }
    abort(reason) {
      return this.locked ? Promise.reject(new TypeError("Stream is locked"))
        : abortWrite(writable.get(this), reason);
    }
    close() {
      return this.locked ? Promise.reject(new TypeError("Stream is locked"))
        : closeWrite(writable.get(this));
    }
  }
  class TransformStream {
    constructor(transformer = {}, writableStrategy = {}, readableStrategy = {}) {
      if (transformer.readableType !== undefined || transformer.writableType !== undefined) {
        throw new RangeError("Typed transforms are unavailable");
      }
      let inputController, outputController;
      let demand = deferred();
      let started;
      const controller = {
        get desiredSize() { return outputController.desiredSize; },
        enqueue(value) {
          outputController.enqueue(value);
          const state = readable.get(output);
          if (!state.reads.length && outputController.desiredSize <= 0) demand = deferred();
        },
        error(error) {
          outputController.error(error);
          if (inputController) inputController.error(error);
          demand.reject(error);
        },
        terminate() {
          outputController.close();
          const error = new TypeError("Transform terminated");
          if (inputController) inputController.error(error);
          demand.reject(error);
        }
      };
      const output = new ReadableStream({
        start(value) { outputController = value; },
        pull() { demand.resolve(); },
        cancel(reason) {
          if (inputController) inputController.error(reason);
          demand.reject(reason);
          return transformer.cancel && transformer.cancel(reason);
        }
      }, { ...readableStrategy, highWaterMark: highWaterMark(readableStrategy, 0) });
      started = Promise.resolve().then(() => transformer.start && transformer.start(controller));
      const input = new WritableStream({
        start(value) { inputController = value; return started; },
        async write(value) {
          await demand.promise;
          try {
            if (transformer.transform) await transformer.transform(value, controller);
            else controller.enqueue(value);
          } catch (error) { controller.error(error); throw error; }
        },
        async close() {
          try {
            if (transformer.flush) await transformer.flush(controller);
            if (readable.get(output).status === "readable" && !readable.get(output).closing) {
              outputController.close();
            }
          } catch (error) { controller.error(error); throw error; }
        },
        abort(reason) { controller.error(reason); }
      }, writableStrategy);
      Object.defineProperties(this, {
        readable: { enumerable: true, value: output },
        writable: { enumerable: true, value: input }
      });
      started.catch(error => controller.error(error));
    }
  }
  class TextEncoderStream {
    constructor() {
      let pending = "";
      const encoder = new TextEncoder();
      const transform = new TransformStream({
        transform(value, controller) {
          let text = pending + String(value);
          pending = "";
          if (text.length && /[\uD800-\uDBFF]/.test(text[text.length - 1])) {
            pending = text[text.length - 1];
            text = text.slice(0, -1);
          }
          if (text) controller.enqueue(encoder.encode(text));
        },
        flush(controller) { if (pending) controller.enqueue(encoder.encode(pending)); }
      });
      Object.defineProperties(this, {
        readable: { enumerable: true, value: transform.readable },
        writable: { enumerable: true, value: transform.writable },
        encoding: { enumerable: true, value: "utf-8" }
      });
    }
  }
  class TextDecoderStream {
    constructor(label = "utf-8", options = {}) {
      const decoder = new TextDecoder(label, options);
      const transform = new TransformStream({
        transform(value, controller) {
          const text = decoder.decode(value, { stream: true });
          if (text) controller.enqueue(text);
        },
        flush(controller) {
          const text = decoder.decode();
          if (text) controller.enqueue(text);
        }
      });
      Object.defineProperties(this, {
        readable: { enumerable: true, value: transform.readable },
        writable: { enumerable: true, value: transform.writable },
        encoding: { enumerable: true, value: decoder.encoding },
        fatal: { enumerable: true, value: decoder.fatal },
        ignoreBOM: { enumerable: true, value: decoder.ignoreBOM }
      });
    }
  }
  function compression(target, format, decode) {
    format = String(format);
    if (!["gzip", "deflate", "deflate-raw"].includes(format)) throw new TypeError("Invalid format");
    const chunks = [];
    let length = 0;
    const transform = new TransformStream({
      transform(value) {
        if (!(value instanceof ArrayBuffer) && !ArrayBuffer.isView(value)) {
          throw new TypeError("Compression requires BufferSource chunks");
        }
        const bytes = value instanceof ArrayBuffer ? new Uint8Array(value)
          : new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
        length += bytes.length;
        if (length > 64 * 1024 * 1024) throw new RangeError("Compression allocation limit exceeded");
        chunks.push(bytes.slice());
      },
      flush(controller) {
        const input = new Uint8Array(length);
        let offset = 0;
        for (const chunk of chunks) { input.set(chunk, offset); offset += chunk.length; }
        controller.enqueue(new Uint8Array(g.__chuzzCodec(format, input.buffer, decode)));
      }
    });
    Object.defineProperties(target, {
      readable: { enumerable: true, value: transform.readable },
      writable: { enumerable: true, value: transform.writable }
    });
  }
  class CompressionStream {
    constructor(format) { compression(this, format, false); }
  }
  class DecompressionStream {
    constructor(format) { compression(this, format, true); }
  }
  for (const Constructor of [
    ReadableStream, ReadableStreamDefaultReader, ReadableStreamDefaultController,
    WritableStream, WritableStreamDefaultWriter, WritableStreamDefaultController,
    TransformStream, TextEncoderStream, TextDecoderStream, CompressionStream, DecompressionStream
  ]) {
    g[Constructor.name] = Constructor;
    Object.defineProperty(Constructor.prototype, Symbol.toStringTag, { value: Constructor.name });
  }
  Object.defineProperty(g, "__chuzzStreams", {
    value: Object.freeze({
      tee,
      disturbed: stream => readable.get(stream).disturbed,
      disturb: stream => { readable.get(stream).disturbed = true; },
      closed: stream => readable.get(stream).closed.promise,
      error: (stream, error) => fail(readable.get(stream), error),
      fromBytes(bytes) {
        return new ReadableStream({
          start(controller) {
            if (bytes.length) controller.enqueue(bytes);
            controller.close();
          }
        });
      }
    })
  });
})(globalThis);

