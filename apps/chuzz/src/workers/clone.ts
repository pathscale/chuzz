(function (g) {
  "use strict";
  const kindOf = g.__chuzzCloneKind;
  const nativeClone = g.__chuzzStructuredClone;
  const keys = Object.keys;
  const define = Object.defineProperty;
  const stringify = JSON.stringify;
  const parse = JSON.parse;
  const ArrayCtor = Array;
  const MapCtor = Map;
  const SetCtor = Set;
  const DateCtor = Date;
  const RegExpCtor = RegExp;
  const ObjectCtor = Object;
  const NumberCtor = Number;
  const BigIntCtor = BigInt;
  const StringCtor = String;
  const DataViewCtor = DataView;
  const get = (prototype, name) =>
    Object.getOwnPropertyDescriptor(prototype, name).get;
  const typedPrototype = Object.getPrototypeOf(Uint8Array.prototype);
  const typedName = get(typedPrototype, Symbol.toStringTag);
  const typedBuffer = get(typedPrototype, "buffer");
  const typedOffset = get(typedPrototype, "byteOffset");
  const typedLength = get(typedPrototype, "length");
  const viewBuffer = get(DataView.prototype, "buffer");
  const viewOffset = get(DataView.prototype, "byteOffset");
  const viewLength = get(DataView.prototype, "byteLength");
  const bufferResizable = get(ArrayBuffer.prototype, "resizable");
  const bufferMaximum = get(ArrayBuffer.prototype, "maxByteLength");
  const dateValue = Date.prototype.getTime;
  const regexpSource = get(RegExp.prototype, "source");
  const regexpFlags = get(RegExp.prototype, "flags");
  const mapEach = Map.prototype.forEach;
  const setEach = Set.prototype.forEach;
  const boxed = {
    boolean: Boolean.prototype.valueOf,
    number: Number.prototype.valueOf,
    string: String.prototype.valueOf,
    bigint: BigInt.prototype.valueOf,
  };
  const typed = Object.create(null);
  [
    "Int8Array", "Uint8Array", "Uint8ClampedArray",
    "Int16Array", "Uint16Array", "Int32Array", "Uint32Array",
    "Float16Array", "Float32Array", "Float64Array",
    "BigInt64Array", "BigUint64Array",
  ].forEach(name => {
    if (typeof g[name] === "function") typed[name] = g[name];
  });
  const errors = {
    Error, EvalError, RangeError, ReferenceError, SyntaxError, TypeError, URIError,
  };
  function failure(message) {
    return g.__chuzzWorkerException("DataCloneError", message);
  }
  function publish(name, value) {
    define(g, name, { value });
  }
  publish("__chuzzBufferMaximum", function (buffer) {
    return bufferResizable.call(buffer) ? bufferMaximum.call(buffer) : -1;
  });
  publish("__chuzzCloneEncode", function (root, transfers) {
    const records = [];
    const seen = new MapCtor();
    const buffers = [];
    const bufferIds = new MapCtor();
    function bufferId(buffer) {
      if (!bufferIds.has(buffer)) {
        bufferIds.set(buffer, buffers.length);
        buffers.push(buffer);
      }
      return bufferIds.get(buffer);
    }
    function visit(value) {
      if (value === null) return ["p", "null"];
      const type = typeof value;
      if (type === "undefined") return ["p", "undefined"];
      // String(-0) is "0"; the decoder reads "-0" back as negative zero.
      if (type === "number") return ["p", "number", Object.is(value, -0) ? "-0" : StringCtor(value)];
      if (type === "bigint") return ["p", "bigint", StringCtor(value)];
      if (type === "string" || type === "boolean") return ["p", type, value];
      if (type !== "object") throw failure("value is not structured-cloneable");
      if (seen.has(value)) return ["r", seen.get(value)];
      const id = records.length;
      seen.set(value, id);
      records.push(null);
      const kind = kindOf(value);
      let record;
      if (kind === "buffer") {
        record = ["buffer", bufferId(value)];
      } else if (kind === "typed") {
        record = [
          "typed", typedName.call(value), visit(typedBuffer.call(value)),
          typedOffset.call(value), typedLength.call(value),
        ];
      } else if (kind === "view") {
        record = [
          "view", visit(viewBuffer.call(value)),
          viewOffset.call(value), viewLength.call(value),
        ];
      } else if (kind === "date") {
        record = ["date", StringCtor(dateValue.call(value))];
      } else if (kind === "regexp") {
        record = ["regexp", regexpSource.call(value), regexpFlags.call(value)];
      } else if (kind === "map") {
        const entries = [];
        mapEach.call(value, (entry, key) => entries.push([visit(key), visit(entry)]));
        record = ["map", entries];
      } else if (kind === "set") {
        const entries = [];
        setEach.call(value, entry => entries.push(visit(entry)));
        record = ["set", entries];
      } else if (kind === "error") {
        const name = StringCtor(value.name);
        record = [
          "error", errors[name] ? name : "Error",
          StringCtor(value.message), StringCtor(value.stack || ""),
          "cause" in value ? visit(value.cause) : null,
        ];
      } else if (boxed[kind]) {
        record = ["boxed", visit(boxed[kind].call(value))];
      } else if (kind === "object" || kind === "array") {
        const properties = [];
        for (const key of keys(value)) properties.push([key, visit(value[key])]);
        record = [kind, properties, kind === "array" ? value.length : 0];
      } else {
        throw failure("object is not structured-cloneable");
      }
      records[id] = record;
      return ["r", id];
    }
    const value = visit(root);
    // The transfer list may contain buffers unreachable from the graph.
    for (const buffer of transfers) bufferId(buffer);
    return [stringify([value, records]), buffers];
  });
  publish("__chuzzCloneDecode", function (description, buffers) {
    const [root, records] = parse(description);
    const values = new ArrayCtor(records.length);
    const constructed = new SetCtor();
    function read(token) {
      if (token[0] === "p") {
        switch (token[1]) {
          case "null": return null;
          case "undefined": return undefined;
          case "number": return token[2] === "-0" ? -0 : NumberCtor(token[2]);
          case "bigint": return BigIntCtor(token[2]);
          default: return token[2];
        }
      }
      const id = token[1];
      if (constructed.has(id)) return values[id];
      const record = records[id];
      const kind = record[0];
      let value;
      switch (kind) {
        case "buffer": value = buffers[record[1]]; break;
        case "typed":
          value = new typed[record[1]](read(record[2]), record[3], record[4]);
          break;
        case "view":
          value = new DataViewCtor(read(record[1]), record[2], record[3]);
          break;
        case "date": value = new DateCtor(NumberCtor(record[1])); break;
        case "regexp": value = new RegExpCtor(record[1], record[2]); break;
        case "map": value = new MapCtor(); break;
        case "set": value = new SetCtor(); break;
        case "array": value = new ArrayCtor(record[2]); break;
        case "object": value = {}; break;
        case "boxed": value = ObjectCtor(read(record[1])); break;
        case "error":
          value = new errors[record[1]](record[2]);
          define(value, "stack", { value: record[3], writable: true, configurable: true });
          break;
        default: throw failure("invalid clone record");
      }
      values[id] = value;
      constructed.add(id);
      if (kind === "array" || kind === "object") {
        for (const [key, token] of record[1]) {
          define(value, key, {
            value: read(token), writable: true, enumerable: true, configurable: true,
          });
        }
      } else if (kind === "map") {
        for (const [key, entry] of record[1]) value.set(read(key), read(entry));
      } else if (kind === "set") {
        for (const entry of record[1]) value.add(read(entry));
      } else if (kind === "error" && record[4] !== null) {
        define(value, "cause", {
          value: read(record[4]), writable: true, configurable: true,
        });
      }
      return value;
    }
    return read(root);
  });
  define(g, "structuredClone", {
    configurable: true,
    writable: true,
    value(value, options) {
      return nativeClone(value, Array.from(options && options.transfer || []));
    },
  });
})(globalThis);

