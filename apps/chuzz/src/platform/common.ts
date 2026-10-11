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
  // The brands agree with the User-Agent and the sec-ch-ua header: a Chrome-family
  // identity reports plain Chromium's list, the honest one reports Chuzz.
  function chromeMajor() {
    const match = /Chrome\/(\d+)\./.exec(String(navigator.userAgent || ""));
    return match ? match[1] : null;
  }
  function brandList(full) {
    const major = chromeMajor();
    if (major === null) {
      return [{ brand: "Chuzz", version: full ? info.version : info.version.split(".")[0] }];
    }
    return [
      { brand: "Chromium", version: full ? `${major}.0.0.0` : major },
      { brand: "Not-A.Brand", version: full ? "99.0.0.0" : "99" }
    ];
  }
  navigator.userAgentData = {
    get brands() { return brandList(false); },
    mobile: false,
    platform: info.uaPlatform,
    toJSON() { return { brands: this.brands, mobile: false, platform: info.uaPlatform }; },
    getHighEntropyValues(hints) {
      return Promise.resolve().then(() => {
        const result = this.toJSON();
        const values = {
          architecture: info.architecture, bitness: info.bitness,
          fullVersionList: brandList(true),
          uaFullVersion: brandList(true)[0].version, wow64: false
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

  // WebAuthn is a Window API. Worker realms still share the other platform APIs.
  if (typeof g.document !== "undefined") {
    const credentialToken = {};
    const responseState = new WeakMap();
    const credentialState = new WeakMap();
    const containerState = new WeakSet();
    const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

    function bufferBytes(value, label) {
      if (value instanceof ArrayBuffer) return Array.from(new Uint8Array(value));
      if (ArrayBuffer.isView(value)) {
        return Array.from(new Uint8Array(value.buffer, value.byteOffset, value.byteLength));
      }
      throw new TypeError(label + " must be a BufferSource");
    }
    function encodeBytes(value) {
      const bytes = bufferBytes(value, "value");
      let result = "";
      for (let offset = 0; offset < bytes.length; offset += 3) {
        const count = Math.min(3, bytes.length - offset);
        const word = (bytes[offset] << 16)
          | ((bytes[offset + 1] || 0) << 8) | (bytes[offset + 2] || 0);
        result += alphabet[(word >>> 18) & 63] + alphabet[(word >>> 12) & 63];
        if (count > 1) result += alphabet[(word >>> 6) & 63];
        if (count > 2) result += alphabet[word & 63];
      }
      return result;
    }
    function decodeBytes(value, label) {
      if (typeof value !== "string" || !/^[A-Za-z0-9_-]*$/.test(value)
          || value.length % 4 === 1) {
        throw exception("EncodingError", label + " is not base64url");
      }
      const bytes = [];
      let word = 0;
      let bits = 0;
      for (const character of value) {
        word = (word << 6) | alphabet.indexOf(character);
        bits += 6;
        if (bits >= 8) {
          bits -= 8;
          bytes.push((word >>> bits) & 255);
        }
      }
      const buffer = new Uint8Array(bytes).buffer;
      if (encodeBytes(buffer) !== value) {
        throw exception("EncodingError", label + " is not canonical base64url");
      }
      return buffer;
    }
    function dictionary(value, label) {
      if (value === null || typeof value !== "object") {
        throw new TypeError(label + " must be a dictionary");
      }
      return value;
    }
    function requiredString(value, label) {
      if (value === undefined) throw new TypeError(label + " is required");
      return String(value);
    }
    function enumeration(value, values, fallback, label) {
      value = value === undefined ? fallback : String(value);
      if (!values.includes(value)) throw new TypeError("Invalid " + label);
      return value;
    }
    function verification(value) {
      return enumeration(value, ["required", "preferred", "discouraged"],
        "preferred", "userVerification");
    }
    function timeout(value) {
      if (value === undefined) return 60000;
      value = Number(value);
      if (!Number.isFinite(value) || value < 0) throw new TypeError("Invalid timeout");
      return Math.max(1, Math.min(120000, Math.trunc(value)));
    }
    function descriptorList(values) {
      return Array.from(values === undefined ? [] : values, descriptor => {
        descriptor = dictionary(descriptor, "credential descriptor");
        return {
          type: enumeration(descriptor.type, ["public-key"], "public-key", "credential type"),
          id: bufferBytes(descriptor.id, "credential id")
        };
      });
    }
    function creationOptions(value) {
      value = dictionary(value, "publicKey");
      const rp = dictionary(value.rp, "rp");
      const user = dictionary(value.user, "user");
      const selection = value.authenticatorSelection === undefined
        ? {} : dictionary(value.authenticatorSelection, "authenticatorSelection");
      const params = Array.from(value.pubKeyCredParams === undefined ? [] : value.pubKeyCredParams,
        parameter => {
          parameter = dictionary(parameter, "pubKeyCredParams entry");
          const alg = Number(parameter.alg);
          if (!Number.isInteger(alg) || alg < -2147483648 || alg > 2147483647) {
            throw new TypeError("Invalid public key algorithm");
          }
          return {
            type: enumeration(parameter.type, ["public-key"], "public-key", "credential type"),
            alg
          };
        });
      return {
        challenge: bufferBytes(value.challenge, "challenge"),
        rp: {
          id: rp.id === undefined ? undefined : String(rp.id),
          name: requiredString(rp.name, "rp.name")
        },
        user: {
          id: bufferBytes(user.id, "user.id"),
          name: requiredString(user.name, "user.name"),
          displayName: requiredString(user.displayName, "user.displayName")
        },
        pubKeyCredParams: params,
        excludeCredentials: descriptorList(value.excludeCredentials),
        authenticatorSelection: {
          authenticatorAttachment: selection.authenticatorAttachment === undefined ? undefined
            : enumeration(selection.authenticatorAttachment, ["platform", "cross-platform"],
              "platform", "authenticatorAttachment"),
          residentKey: selection.residentKey === undefined ? undefined
            : enumeration(selection.residentKey, ["required", "preferred", "discouraged"],
              "discouraged", "residentKey"),
          requireResidentKey: !!selection.requireResidentKey,
          userVerification: verification(selection.userVerification)
        },
        attestation: enumeration(value.attestation, ["none", "indirect", "direct", "enterprise"],
          "none", "attestation"),
        timeout: timeout(value.timeout)
      };
    }
    function requestOptions(value) {
      value = dictionary(value, "publicKey");
      return {
        challenge: bufferBytes(value.challenge, "challenge"),
        rpId: value.rpId === undefined ? undefined : String(value.rpId),
        allowCredentials: descriptorList(value.allowCredentials),
        userVerification: verification(value.userVerification),
        timeout: timeout(value.timeout)
      };
    }
    function attributes(object, values) {
      for (const name of Object.keys(values)) {
        Object.defineProperty(object, name, { enumerable: true, value: values[name] });
      }
    }
    function responseData(response, kind) {
      const state = responseState.get(response);
      if (!state || (kind && state.kind !== kind)) throw new TypeError("Illegal invocation");
      return state;
    }
    class Credential {
      constructor(token, values) {
        if (token !== credentialToken) throw new TypeError("Illegal constructor");
        attributes(this, { id: values.id, type: "public-key" });
      }
    }
    class AuthenticatorResponse {
      constructor(token, values, kind) {
        if (token !== credentialToken) throw new TypeError("Illegal constructor");
        attributes(this, { clientDataJSON: new Uint8Array(values.clientDataJSON).buffer });
        responseState.set(this, { kind, values });
      }
    }
    class AuthenticatorAttestationResponse extends AuthenticatorResponse {
      constructor(token, values) {
        super(token, values, "create");
        attributes(this, { attestationObject: new Uint8Array(values.attestationObject).buffer });
        const state = responseData(this);
        state.authenticatorData = new Uint8Array(values.authenticatorData).buffer;
        state.publicKey = new Uint8Array(values.publicKey).buffer;
      }
      getAuthenticatorData() {
        return responseData(this, "create").authenticatorData.slice(0);
      }
      getPublicKey() { return responseData(this, "create").publicKey.slice(0); }
      getPublicKeyAlgorithm() { return responseData(this, "create").values.publicKeyAlgorithm; }
      getTransports() { return responseData(this, "create").values.transports.slice(); }
    }
    class AuthenticatorAssertionResponse extends AuthenticatorResponse {
      constructor(token, values) {
        super(token, values, "get");
        attributes(this, {
          authenticatorData: new Uint8Array(values.authenticatorData).buffer,
          signature: new Uint8Array(values.signature).buffer,
          userHandle: values.userHandle === null ? null : new Uint8Array(values.userHandle).buffer
        });
      }
    }
    class PublicKeyCredential extends Credential {
      constructor(token, values, create) {
        super(token, values);
        const response = create
          ? new AuthenticatorAttestationResponse(token, values)
          : new AuthenticatorAssertionResponse(token, values);
        attributes(this, {
          rawId: new Uint8Array(values.rawId).buffer,
          response,
          authenticatorAttachment: values.authenticatorAttachment
        });
        credentialState.set(this, { create });
      }
      getClientExtensionResults() {
        if (!credentialState.has(this)) throw new TypeError("Illegal invocation");
        return {};
      }
      toJSON() {
        const state = credentialState.get(this);
        if (!state) throw new TypeError("Illegal invocation");
        const response = this.response;
        const result = {
          id: this.id,
          rawId: encodeBytes(this.rawId),
          type: this.type,
          authenticatorAttachment: this.authenticatorAttachment,
          clientExtensionResults: this.getClientExtensionResults(),
          response: { clientDataJSON: encodeBytes(response.clientDataJSON) }
        };
        if (state.create) {
          Object.assign(result.response, {
            attestationObject: encodeBytes(response.attestationObject),
            authenticatorData: encodeBytes(response.getAuthenticatorData()),
            publicKey: encodeBytes(response.getPublicKey()),
            publicKeyAlgorithm: response.getPublicKeyAlgorithm(),
            transports: response.getTransports()
          });
        } else {
          Object.assign(result.response, {
            authenticatorData: encodeBytes(response.authenticatorData),
            signature: encodeBytes(response.signature),
            userHandle: response.userHandle === null ? null : encodeBytes(response.userHandle)
          });
        }
        return result;
      }
      static isUserVerifyingPlatformAuthenticatorAvailable() {
        return Promise.resolve().then(() => !!call({ op: "webauthn-info" }));
      }
      static isConditionalMediationAvailable() { return Promise.resolve(false); }
      static getClientCapabilities() {
        return this.isUserVerifyingPlatformAuthenticatorAvailable().then(available => ({
          conditionalCreate: false,
          conditionalGet: false,
          hybridTransport: false,
          passkeyPlatformAuthenticator: available,
          userVerifyingPlatformAuthenticator: available,
          relatedOrigins: false,
          signalAllAcceptedCredentials: false,
          signalCurrentUserDetails: false,
          signalUnknownCredential: false
        }));
      }
      static parseCreationOptionsFromJSON(value) {
        value = dictionary(value, "options");
        const user = dictionary(value.user, "user");
        return {
          ...value,
          rp: { ...dictionary(value.rp, "rp") },
          challenge: decodeBytes(value.challenge, "challenge"),
          user: { ...user, id: decodeBytes(user.id, "user.id") },
          pubKeyCredParams: Array.from(value.pubKeyCredParams || [], entry => ({ ...entry })),
          excludeCredentials: jsonDescriptors(value.excludeCredentials)
        };
      }
      static parseRequestOptionsFromJSON(value) {
        value = dictionary(value, "options");
        return {
          ...value,
          challenge: decodeBytes(value.challenge, "challenge"),
          allowCredentials: jsonDescriptors(value.allowCredentials)
        };
      }
    }
    function jsonDescriptors(values) {
      return Array.from(values === undefined ? [] : values, descriptor => {
        descriptor = dictionary(descriptor, "credential descriptor");
        return {
          ...descriptor,
          id: decodeBytes(descriptor.id, "credential id"),
          ...(descriptor.transports === undefined ? {}
            : { transports: Array.from(descriptor.transports, String) })
        };
      });
    }
    function credentialsRequest(create, options) {
      return new Promise((resolve, reject) => {
        options = dictionary(options, "options");
        if (!info.secure) throw exception("SecurityError", "WebAuthn requires a secure context");
        if (options.publicKey === undefined) {
          throw exception("NotSupportedError", "Only public-key credentials are supported");
        }
        const mediation = enumeration(options.mediation,
          ["silent", "optional", "required", "conditional"], "optional", "mediation");
        if (mediation === "conditional" || mediation === "silent") {
          throw exception("NotAllowedError", "This mediation mode is unavailable");
        }
        const signal = options.signal;
        if (signal !== undefined && !(signal instanceof g.AbortSignal)) {
          throw new TypeError("signal must be an AbortSignal");
        }
        if (signal && signal.aborted) throw exception("AbortError", "The request was aborted");
        const prepared = create ? creationOptions(options.publicKey) : requestOptions(options.publicKey);
        const started = call({
          op: create ? "webauthn-create" : "webauthn-get", options: prepared
        });
        if (started && started.error) {
          throw exception(started.error.name, started.error.message);
        }
        const id = started;
        let done = false;
        let timer = null;
        function cleanup() {
          done = true;
          if (timer !== null) g.clearTimeout(timer);
          if (signal) signal.removeEventListener("abort", abort);
        }
        function abort() {
          if (done) return;
          cleanup();
          try { call({ op: "webauthn-abort", id }); } finally {
            reject(exception("AbortError", "The request was aborted"));
          }
        }
        function check() {
          if (done) return;
          if (signal && signal.aborted) { abort(); return; }
          try {
            const state = call({ op: "webauthn-status", id });
            if (state.pending) {
              timer = g.setTimeout(check, 10);
              return;
            }
            cleanup();
            if (state.error) reject(exception(state.error.name, state.error.message));
            else resolve(new PublicKeyCredential(credentialToken, state.result, create));
          } catch (error) {
            cleanup();
            call({ op: "webauthn-abort", id });
            reject(error);
          }
        }
        if (signal) signal.addEventListener("abort", abort, { once: true });
        check();
      });
    }
    class CredentialsContainer {
      constructor(token) {
        if (token !== credentialToken) throw new TypeError("Illegal constructor");
        containerState.add(this);
      }
      create(options = {}) {
        if (!containerState.has(this)) return Promise.reject(new TypeError("Illegal invocation"));
        return credentialsRequest(true, options);
      }
      get(options = {}) {
        if (!containerState.has(this)) return Promise.reject(new TypeError("Illegal invocation"));
        return credentialsRequest(false, options);
      }
      store() {
        if (!containerState.has(this)) return Promise.reject(new TypeError("Illegal invocation"));
        return Promise.reject(exception("NotSupportedError", "Credential storage is unavailable"));
      }
      preventSilentAccess() {
        if (!containerState.has(this)) return Promise.reject(new TypeError("Illegal invocation"));
        return Promise.resolve();
      }
    }
    for (const constructor of [
      Credential, PublicKeyCredential, AuthenticatorResponse,
      AuthenticatorAttestationResponse, AuthenticatorAssertionResponse, CredentialsContainer
    ]) {
      Object.defineProperty(constructor.prototype, Symbol.toStringTag, { value: constructor.name });
      g[constructor.name] = constructor;
    }
    Object.defineProperty(navigator, "credentials", {
      configurable: true, enumerable: true, value: new CredentialsContainer(credentialToken)
    });
  }

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


