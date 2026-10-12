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
  function installPermissions() {
    if (!g.navigator || "permissions" in g.navigator) return;
    const stateByStatus = new WeakMap();
    const names = new Set([
      "camera", "clipboard-read", "clipboard-write", "geolocation", "local-fonts",
      "microphone", "midi", "notifications", "persistent-storage", "push",
      "screen-wake-lock", "storage-access", "window-management"
    ]);
    class PermissionStatus extends Target {
      constructor(token, name, state) {
        super();
        if (token !== stateByStatus) throw new TypeError("Illegal constructor");
        stateByStatus.set(this, { name, state });
        this.onchange = null;
      }
      get name() {
        const state = stateByStatus.get(this);
        if (!state) throw new TypeError("Illegal invocation");
        return state.name;
      }
      get state() {
        const state = stateByStatus.get(this);
        if (!state) throw new TypeError("Illegal invocation");
        return state.state;
      }
    }
    Object.defineProperty(PermissionStatus.prototype, Symbol.toStringTag, {
      value: "PermissionStatus"
    });
    const permissions = {
      query(descriptor) {
        if (this !== permissions) throw new TypeError("Illegal invocation");
        if (descriptor === null || typeof descriptor !== "object") {
          return Promise.reject(new TypeError("A permission descriptor is required"));
        }
        const name = String(descriptor.name);
        if (!names.has(name)) return Promise.reject(new TypeError("Unknown permission name"));
        return Promise.resolve(new PermissionStatus(
          stateByStatus, name, "denied"
        ));
      }
    };
    Object.defineProperty(permissions, Symbol.toStringTag, { value: "Permissions" });
    g.PermissionStatus = PermissionStatus;
    Object.defineProperty(g.navigator, "permissions", {
      configurable: true, enumerable: true, value: permissions
    });
  }

  function installOptionReflection() {
    const prototype = g.HTMLOptionElement && g.HTMLOptionElement.prototype;
    if (!prototype) return;
    if (!("text" in prototype)) {
      Object.defineProperty(prototype, "text", {
        configurable: true, enumerable: true,
        get: function () {
          return String(this.textContent || "").replace(/[\t\n\f\r ]+/g, " ").trim();
        },
        set: function (value) { this.textContent = String(value); }
      });
    }
    if (!("defaultSelected" in prototype)) {
      Object.defineProperty(prototype, "defaultSelected", {
        configurable: true, enumerable: true,
        get: function () { return this.hasAttribute("selected"); },
        set: function (value) {
          if (value) this.setAttribute("selected", "");
          else this.removeAttribute("selected");
        }
      });
    }
  }

  function installOptionConstructor() {
    installOptionReflection();
    if ("Option" in g || !g.document) return;
    function Option(text, value, defaultSelected, selected) {
      const option = g.document.createElement("option");
      option.text = text === undefined ? "" : String(text);
      if (arguments.length > 1 && value !== undefined) option.value = String(value);
      option.defaultSelected = arguments.length > 2 ? !!defaultSelected : false;
      option.selected = arguments.length > 3 ? !!selected : option.defaultSelected;
      return option;
    }
    if (g.HTMLOptionElement && g.HTMLOptionElement.prototype) {
      Option.prototype = g.HTMLOptionElement.prototype;
    }
    Object.defineProperty(Option, Symbol.toStringTag, { value: "Option" });
    g.Option = Option;
  }

  function installQueryCommandSupported() {
    if (!g.document) return;
    const prototype = g.Document && g.Document.prototype
      ? g.Document.prototype : g.document;
    if ("queryCommandSupported" in prototype) return;
    Object.defineProperty(prototype, "queryCommandSupported", {
      configurable: true, writable: true,
      value: function queryCommandSupported(command) {
        if (!(this && this.nodeType === 9)) throw new TypeError("Illegal invocation");
        String(command);
        return false;
      }
    });
  }

  function installReportingObserver() {
    if ("ReportingObserver" in g) return;
    const states = new WeakMap();
    class ReportingObserver extends Target {
      constructor(callback, options = {}) {
        super();
        if (typeof callback !== "function") throw new TypeError("A callback is required");
        states.set(this, { callback, options, observed: false });
        this.onreport = null;
      }
      observe(options = {}) {
        const state = states.get(this);
        if (!state) throw new TypeError("Illegal invocation");
        if (options === null || typeof options !== "object") {
          throw new TypeError("Observer options must be a dictionary");
        }
        const types = options.types === undefined
          ? options.type === undefined ? [] : [String(options.type)]
          : Array.from(options.types, String);
        if (!types.length || types.some(type =>
          !ReportingObserver.supportedEntryTypes.includes(type))) {
          throw new TypeError("Unsupported report type");
        }
        state.observed = true;
      }
      disconnect() {
        const state = states.get(this);
        if (!state) throw new TypeError("Illegal invocation");
        state.observed = false;
      }
      takeRecords() {
        if (!states.has(this)) throw new TypeError("Illegal invocation");
        return [];
      }
    }
    Object.defineProperty(ReportingObserver, "supportedEntryTypes", {
      value: Object.freeze(["deprecation", "intervention", "crash"])
    });
    Object.defineProperty(ReportingObserver.prototype, Symbol.toStringTag, {
      value: "ReportingObserver"
    });
    g.ReportingObserver = ReportingObserver;
  }

  function installTrustedTypes() {
    if ("trustedTypes" in g) return;
    const brands = new WeakMap();
    const policies = new Map();
    const names = [];
    function makeTrusted(kind, value) {
      const trusted = Object.create(null);
      brands.set(trusted, { kind, value: String(value) });
      Object.defineProperties(trusted, {
        toString: { value: function () { return brands.get(this).value; } },
        valueOf: { value: function () { return brands.get(this).value; } },
        [Symbol.toStringTag]: { value: "Trusted" + kind }
      });
      return Object.freeze(trusted);
    }
    function TrustedTypePolicy(name, rules) {
      Object.defineProperties(this, {
        name: { enumerable: true, value: name },
        createHTML: { value: value => makeTrusted("HTML",
          rules.createHTML ? rules.createHTML(String(value)) : String(value)) },
        createScript: { value: value => makeTrusted("Script",
          rules.createScript ? rules.createScript(String(value)) : String(value)) },
        createScriptURL: { value: value => makeTrusted("ScriptURL",
          rules.createScriptURL ? rules.createScriptURL(String(value)) : String(value)) }
      });
      Object.freeze(this);
    }
    // The token lists of every `trusted-types` directive in the document's
    // meta CSPs. Each is enforced on its own, as separate policies are.
    function trustedTypesDirectives() {
      const lists = [];
      const doc = g.document;
      if (!doc || typeof doc.querySelectorAll !== "function") return lists;
      for (const meta of doc.querySelectorAll("meta[http-equiv]")) {
        if (String(meta.getAttribute("http-equiv")).toLowerCase() !== "content-security-policy") {
          continue;
        }
        for (const directive of String(meta.getAttribute("content") || "").split(";")) {
          const tokens = directive.trim().split(/\s+/);
          if (tokens[0].toLowerCase() === "trusted-types") lists.push(tokens.slice(1));
        }
      }
      return lists;
    }
    const trustedTypes = {
      createPolicy(name, rules = {}, options = {}) {
        name = String(name);
        if (rules === null || typeof rules !== "object") {
          throw new TypeError("Policy rules must be a dictionary");
        }
        // Only an enforced CSP `trusted-types` directive refuses a policy.
        // Without one, any name may be created any number of times.
        const csp = trustedTypesDirectives();
        for (const allowed of csp) {
          const permitted = allowed.includes("*") || allowed.includes(name);
          const duplicate = policies.has(name) && !allowed.includes("'allow-duplicates'");
          if (allowed.includes("'none'") || !permitted || duplicate) {
            throw new TypeError(
              "Failed to create policy '" + name + "': refused by the trusted-types directive"
            );
          }
        }
        for (const key of ["createHTML", "createScript", "createScriptURL"]) {
          if (rules[key] !== undefined && typeof rules[key] !== "function") {
            throw new TypeError(key + " must be callable");
          }
        }
        const policy = new TrustedTypePolicy(name, rules);
        if (!policies.has(name)) names.push(name);
        policies.set(name, policy);
        return policy;
      },
      getPolicy(name) { return policies.get(String(name)) || null; },
      getPolicyNames() { return names.slice(); },
      isHTML(value) { return brands.has(value) && brands.get(value).kind === "HTML"; },
      isScript(value) { return brands.has(value) && brands.get(value).kind === "Script"; },
      isScriptURL(value) {
        return brands.has(value) && brands.get(value).kind === "ScriptURL";
      },
      emptyHTML: makeTrusted("HTML", ""),
      emptyScript: makeTrusted("Script", "")
    };
    Object.defineProperty(trustedTypes, Symbol.toStringTag, { value: "TrustedTypePolicyFactory" });
    g.trustedTypes = Object.freeze(trustedTypes);
    g.TrustedTypePolicy = TrustedTypePolicy;
  }

  function installSpeechSynthesis() {
    if ("speechSynthesis" in g) return;
    const utteranceStates = new WeakMap();
    const pending = new Set();
    class SpeechSynthesisUtterance extends Target {
      constructor(text = "") {
        super();
        this.text = String(text);
        this.lang = "";
        this.voice = null;
        this.volume = 1;
        this.rate = 1;
        this.pitch = 1;
        this.onstart = this.onend = this.onerror = null;
        utteranceStates.set(this, { timer: null });
      }
    }
    Object.defineProperty(SpeechSynthesisUtterance.prototype, Symbol.toStringTag, {
      value: "SpeechSynthesisUtterance"
    });
    class SpeechSynthesis extends Target {
      constructor(token) {
        super();
        if (token !== utteranceStates) throw new TypeError("Illegal constructor");
        this.onvoiceschanged = null;
      }
      get pending() { return false; }
      get speaking() { return false; }
      get paused() { return false; }
      get onvoiceschanged() { return this._onvoiceschanged || null; }
      set onvoiceschanged(value) { this._onvoiceschanged = value; }
      getVoices() { return []; }
      speak(utterance) {
        if (!(utterance instanceof SpeechSynthesisUtterance)) {
          throw new TypeError("A SpeechSynthesisUtterance is required");
        }
        const state = utteranceStates.get(utterance);
        if (!state) throw new TypeError("Illegal invocation");
        if (state.timer !== null) g.clearTimeout(state.timer);
        pending.add(utterance);
        state.timer = g.setTimeout(() => {
          state.timer = null;
          pending.delete(utterance);
          utterance.dispatchEvent(event("error", { error: "synthesis-unavailable" }));
        }, 0);
      }
      cancel() {
        // There is no audio queue. Pending unavailable errors are canceled.
        for (const utterance of pending) {
          const state = utteranceStates.get(utterance);
          if (state && state.timer !== null) g.clearTimeout(state.timer);
          if (state) state.timer = null;
        }
        pending.clear();
      }
      pause() {}
      resume() {}
    }
    Object.defineProperty(SpeechSynthesis.prototype, Symbol.toStringTag, {
      value: "SpeechSynthesis"
    });
    g.SpeechSynthesisUtterance = SpeechSynthesisUtterance;
    g.SpeechSynthesis = SpeechSynthesis;
    Object.defineProperty(g, "speechSynthesis", {
      configurable: true, enumerable: true,
      value: new SpeechSynthesis(utteranceStates)
    });
  }

  function installElementScrolling(elementPrototype) {
    function scrollElement(element, x, y, relative) {
      if (!element || element.nodeType !== 1) throw new TypeError("Illegal invocation");
      let left;
      let top;
      let behavior = "auto";
      if (x !== null && typeof x === "object") {
        const options = x;
        behavior = options.behavior === undefined ? "auto" : String(options.behavior);
        left = options.left === undefined ? element.scrollLeft
          : Number(options.left) + (relative ? element.scrollLeft : 0);
        top = options.top === undefined ? element.scrollTop
          : Number(options.top) + (relative ? element.scrollTop : 0);
      } else {
        left = x === undefined ? 0 : Number(x);
        top = y === undefined ? 0 : Number(y);
        if (relative) {
          left += element.scrollLeft;
          top += element.scrollTop;
        }
      }
      if (!["auto", "instant", "smooth"].includes(behavior)) {
        throw new TypeError("Invalid scroll behavior");
      }
      if (!Number.isFinite(left)) left = 0;
      if (!Number.isFinite(top)) top = 0;
      // Smooth scrolling is unavailable; valid requests move immediately.
      element.scrollLeft = left;
      element.scrollTop = top;
    }
    function scrollElementTo(x, y) { return scrollElement(this, x, y, false); }
    function scrollElementBy(x, y) { return scrollElement(this, x, y, true); }
    function scrollElementAlias(x, y) { return scrollElementTo.call(this, x, y); }
    if (!("scrollTo" in elementPrototype)) {
      Object.defineProperty(elementPrototype, "scrollTo", {
        configurable: true, writable: true, value: scrollElementTo
      });
    }
    if (!("scrollBy" in elementPrototype)) {
      Object.defineProperty(elementPrototype, "scrollBy", {
        configurable: true, writable: true, value: scrollElementBy
      });
    }
    if (!("scroll" in elementPrototype)) {
      Object.defineProperty(elementPrototype, "scroll", {
        configurable: true, writable: true, value: scrollElementAlias
      });
    }
  }

  function installRequestFullscreen(elementPrototype) {
    if ("requestFullscreen" in elementPrototype) return;
    Object.defineProperty(elementPrototype, "requestFullscreen", {
      configurable: true, writable: true,
      value: function requestElementFullscreen() {
        if (!(this && this.nodeType === 1)) {
          return Promise.reject(new TypeError("Illegal invocation"));
        }
        return Promise.reject(exception(
          "NotSupportedError", "Fullscreen integration is unavailable"
        ));
      }
    });
  }

  function checkElementVisibility(element, options = {}) {
    if (!(element && element.nodeType === 1)) throw new TypeError("Illegal invocation");
    if (!g.document.documentElement.contains(element)) return false;
    let node = element;
    while (node && node.nodeType === 1) {
      const style = g.getComputedStyle(node);
      if (style.display === "none") return false;
      if (node === element && style.visibility !== "visible") return false;
      if (options.checkOpacity && Number(style.opacity) === 0) return false;
      node = node.parentElement;
    }
    const ownStyle = g.getComputedStyle(element);
    if (ownStyle.display === "contents") {
      return Array.from(element.querySelectorAll("*")).some(child =>
        child.getClientRects().length > 0
        && g.getComputedStyle(child).display !== "none"
        && g.getComputedStyle(child).visibility === "visible");
    }
    return element.getClientRects().length > 0;
  }

  function installCheckVisibility(elementPrototype) {
    if ("checkVisibility" in elementPrototype) return;
    Object.defineProperty(elementPrototype, "checkVisibility", {
      configurable: true, writable: true,
      value: function checkVisibility(options = {}) {
        if (options === null || typeof options !== "object") {
          throw new TypeError("Visibility options must be a dictionary");
        }
        return checkElementVisibility(this, options);
      }
    });
  }

  function installElementAnimations(elementPrototype) {
    if (typeof g.requestAnimationFrame !== "function"
        || typeof g.cancelAnimationFrame !== "function") return;
    const active = new WeakMap();
    const animationState = new WeakMap();
    function cssProperty(name) {
      return String(name).replace(/[A-Z]/g, character => "-" + character.toLowerCase());
    }
    // `{opacity: ["0", "1"]}` is the same effect as two keyframe objects.
    function propertyIndexedFrames(indexed) {
      const frames = [];
      for (const name of Object.keys(indexed)) {
        if (name === "offset" || name === "easing" || name === "composite") continue;
        const values = Array.isArray(indexed[name]) ? indexed[name] : [indexed[name]];
        values.forEach((value, index) => {
          frames[index] = frames[index] || {};
          frames[index][name] = value;
        });
      }
      return frames;
    }
    function framePairs(keyframes) {
      // No keyframes is a valid effect that changes nothing; feature tests
      // such as `element.animate([], 0)` rely on it.
      if (keyframes === null || keyframes === undefined) return [];
      if (!Array.isArray(keyframes) && typeof keyframes === "object") {
        keyframes = propertyIndexedFrames(keyframes);
      }
      if (Array.isArray(keyframes) && keyframes.length === 0) return [];
      if (!Array.isArray(keyframes) || keyframes.length !== 2
          || !keyframes[0] || !keyframes[1]) {
        throw exception("NotSupportedError", "Only two-frame keyframes are supported");
      }
      const first = keyframes[0];
      const last = keyframes[1];
      const properties = Object.keys(first).filter(key =>
        Object.prototype.hasOwnProperty.call(last, key));
      if (!properties.length) throw exception("NotSupportedError", "No animatable properties");
      return properties.map(name => {
        const start = String(first[name]).trim();
        const end = String(last[name]).trim();
        const a = /^(-?(?:[0-9]+\.?[0-9]*|\.[0-9]+))([a-z%]*)$/i.exec(start);
        const b = /^(-?(?:[0-9]+\.?[0-9]*|\.[0-9]+))([a-z%]*)$/i.exec(end);
        if (!a || !b || a[2] !== b[2]) {
          throw exception("NotSupportedError", "Only matching numeric CSS values are supported");
        }
        return { name: cssProperty(name), from: Number(a[1]), to: Number(b[1]), unit: a[2] };
      });
    }
    function deferred() {
      let resolve;
      let reject;
      const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
      promise.catch(() => {});
      return { promise, resolve, reject };
    }
    function removeActive(state) {
      const set = active.get(state.element);
      if (set) set.delete(state.animation);
    }
    function restore(state) {
      for (const item of state.properties) {
        if (item.value) state.element.style.setProperty(item.name, item.value, item.priority);
        else state.element.style.removeProperty(item.name);
      }
    }
    function apply(state, progress) {
      for (const item of state.properties) {
        const value = item.from + (item.to - item.from) * progress;
        state.element.style.setProperty(item.name, String(value) + item.unit);
      }
    }
    class Animation extends Target {
      constructor(element, keyframes, options) {
        super();
        if (!element || element.nodeType !== 1) throw new TypeError("Illegal invocation");
        options = typeof options === "number" ? { duration: options } : options || {};
        const duration = options.duration === undefined ? 0 : Number(options.duration);
        if (options.delay !== undefined && Number(options.delay) !== 0) {
          throw exception("NotSupportedError", "Animation delay is unsupported");
        }
        if (options.direction !== undefined && String(options.direction) !== "normal") {
          throw exception("NotSupportedError", "Animation direction is unsupported");
        }
        if (!Number.isFinite(duration) || duration < 0) throw new TypeError("Invalid duration");
        if (options.iterations !== undefined && Number(options.iterations) !== 1) {
          throw exception("NotSupportedError", "Only one animation iteration is supported");
        }
        if (options.easing !== undefined && String(options.easing) !== "linear") {
          throw exception("NotSupportedError", "Only linear easing is supported");
        }
        const fill = options.fill === undefined ? "none" : String(options.fill);
        if (fill !== "none" && fill !== "forwards") {
          throw exception("NotSupportedError", "Only none and forwards fill are supported");
        }
        const pairs = framePairs(keyframes);
        const properties = pairs.map(pair => ({
          name: pair.name,
          from: pair.from,
          to: pair.to,
          unit: pair.unit,
          value: element.style.getPropertyValue(pair.name),
          priority: element.style.getPropertyPriority(pair.name)
        }));
        const state = {
          animation: this, element, properties, duration, fill,
          currentTime: 0, startTime: null, frame: null, playState: "idle",
          finished: deferred()
        };
        animationState.set(this, state);
        this.onfinish = this.oncancel = null;
        this.ready = Promise.resolve(this);
      }
      get currentTime() {
        const state = animationState.get(this);
        if (!state) throw new TypeError("Illegal invocation");
        return state.currentTime;
      }
      set currentTime(value) {
        const state = animationState.get(this);
        if (!state) throw new TypeError("Illegal invocation");
        state.currentTime = Number(value);
        if (state.playState === "running") apply(state, Math.max(0, Math.min(1,
          state.currentTime / Math.max(1, state.duration))));
      }
      get playState() {
        const state = animationState.get(this);
        if (!state) throw new TypeError("Illegal invocation");
        return state.playState;
      }
      get finished() {
        const state = animationState.get(this);
        if (!state) throw new TypeError("Illegal invocation");
        return state.finished.promise;
      }
      play() {
        const state = animationState.get(this);
        if (!state) throw new TypeError("Illegal invocation");
        if (state.playState === "finished" || state.playState === "idle") {
          state.finished = deferred();
          state.currentTime = 0;
          state.startTime = null;
        }
        state.playState = "running";
        let set = active.get(state.element);
        if (!set) { set = new Set(); active.set(state.element, set); }
        set.add(this);
        const tick = timestamp => {
          if (state.playState !== "running") return;
          if (state.startTime === null) state.startTime = timestamp - state.currentTime;
          state.currentTime = Math.max(0, timestamp - state.startTime);
          const progress = state.duration === 0 ? 1
            : Math.min(1, state.currentTime / state.duration);
          apply(state, progress);
          if (progress >= 1) {
            state.playState = "finished";
            state.frame = null;
            removeActive(state);
            if (state.fill !== "forwards") restore(state);
            state.finished.resolve(this);
            this.dispatchEvent(event("finish"));
          } else {
            state.frame = g.requestAnimationFrame(tick);
          }
        };
        state.frame = g.requestAnimationFrame(tick);
        return this;
      }
      pause() {
        const state = animationState.get(this);
        if (!state) throw new TypeError("Illegal invocation");
        if (state.playState === "running") {
          if (state.frame !== null) g.cancelAnimationFrame(state.frame);
          state.frame = null;
          state.playState = "paused";
        }
      }
      cancel() {
        const state = animationState.get(this);
        if (!state) throw new TypeError("Illegal invocation");
        if (state.frame !== null) g.cancelAnimationFrame(state.frame);
        state.frame = null;
        state.currentTime = 0;
        state.startTime = null;
        const wasActive = state.playState === "running" || state.playState === "paused";
        state.playState = "idle";
        removeActive(state);
        restore(state);
        if (wasActive) {
          state.finished.reject(exception("AbortError", "The animation was canceled"));
          this.dispatchEvent(event("cancel"));
        }
      }
      finish() {
        const state = animationState.get(this);
        if (!state) throw new TypeError("Illegal invocation");
        if (state.frame !== null) g.cancelAnimationFrame(state.frame);
        state.frame = null;
        state.currentTime = state.duration;
        apply(state, 1);
        state.playState = "finished";
        removeActive(state);
        if (state.fill !== "forwards") restore(state);
        state.finished.resolve(this);
        this.dispatchEvent(event("finish"));
      }
    }
    Object.defineProperty(Animation.prototype, Symbol.toStringTag, { value: "Animation" });
    function animateElement(element, keyframes, options) {
      const animation = new Animation(element, keyframes, options);
      animation.play();
      return animation;
    }
    function getElementAnimations(element, options = {}) {
      if (!(element && element.nodeType === 1)) throw new TypeError("Illegal invocation");
      const elements = [element];
      if (options && options.subtree) elements.push(...element.querySelectorAll("*"));
      const result = [];
      for (const node of elements) {
        const set = active.get(node);
        if (set) for (const animation of set) {
          if (animation.playState === "running" || animation.playState === "paused") {
            result.push(animation);
          }
        }
      }
      return result;
    }
    if (!("animate" in elementPrototype)) {
      Object.defineProperty(elementPrototype, "animate", {
        configurable: true, writable: true,
        value: function (keyframes, options) { return animateElement(this, keyframes, options); }
      });
    }
    if (!("getAnimations" in elementPrototype)) {
      Object.defineProperty(elementPrototype, "getAnimations", {
        configurable: true, writable: true,
        value: function (options) { return getElementAnimations(this, options); }
      });
    }
    if (!("Animation" in g)) g.Animation = Animation;
  }

  function installPartTokens(elementPrototype) {
    if ("part" in elementPrototype || typeof g.Proxy !== "function") return;
    const lists = new WeakMap();
    function read(element) {
      const value = element.getAttribute("part") || "";
      return value.trim() ? Array.from(new Set(value.trim().split(/[\t\n\f\r ]+/))) : [];
    }
    function write(element, values) {
      if (values.length) element.setAttribute("part", values.join(" "));
      else element.removeAttribute("part");
    }
    function validate(value) {
      value = String(value);
      if (!value) throw exception("SyntaxError", "A token cannot be empty");
      if (/[\t\n\f\r ]/.test(value)) {
        throw exception("InvalidCharacterError", "A token cannot contain whitespace");
      }
      return value;
    }
    function partTokens(element) {
      if (!(element && element.nodeType === 1)) throw new TypeError("Illegal invocation");
      if (lists.has(element)) return lists.get(element);
      const methods = {
        item(index) { return read(element)[Number(index)] || null; },
        contains(token) { return read(element).includes(validate(token)); },
        add(...tokens) {
          const current = read(element);
          for (const token of tokens.map(validate)) if (!current.includes(token)) current.push(token);
          write(element, current);
        },
        remove(...tokens) {
          const remove = new Set(tokens.map(validate));
          write(element, read(element).filter(token => !remove.has(token)));
        },
        toggle(token, force) {
          token = validate(token);
          const current = read(element);
          const exists = current.includes(token);
          const add = force === undefined ? !exists : !!force;
          if (add && !exists) current.push(token);
          if (!add && exists) current.splice(current.indexOf(token), 1);
          write(element, current);
          return add;
        },
        replace(oldToken, newToken) {
          oldToken = validate(oldToken);
          newToken = validate(newToken);
          const current = read(element);
          const index = current.indexOf(oldToken);
          if (index < 0) return false;
          if (!current.includes(newToken)) current[index] = newToken;
          else current.splice(index, 1);
          write(element, current);
          return true;
        },
        supports(token) { validate(token); return true; },
        forEach(callback, thisArg) {
          if (typeof callback !== "function") throw new TypeError("Callback must be callable");
          read(element).forEach((token, index) => callback.call(thisArg, token, index, proxy));
        },
        toString() { return element.getAttribute("part") || ""; }
      };
      const proxy = new g.Proxy({}, {
        get(target, key) {
          const current = read(element);
          if (key === "length") return current.length;
          if (key === "value") return element.getAttribute("part") || "";
          if (key === Symbol.iterator) return function () { return current[Symbol.iterator](); };
          if (key === "keys") return function () { return current.keys(); };
          if (key === "values") return function () { return current.values(); };
          if (key === "entries") return function () { return current.entries(); };
          if (typeof key === "string" && /^(0|[1-9][0-9]*)$/.test(key)) {
            return current[Number(key)];
          }
          return methods[key];
        },
        set(target, key, value) {
          if (key !== "value") return false;
          const tokens = String(value).trim().split(/[\t\n\f\r ]+/).filter(Boolean);
          write(element, Array.from(new Set(tokens)));
          return true;
        },
        ownKeys() { return read(element).map((_, index) => String(index)); },
        getOwnPropertyDescriptor(target, key) {
          if (typeof key === "string" && /^(0|[1-9][0-9]*)$/.test(key)) {
            const token = read(element)[Number(key)];
            if (token !== undefined) return { configurable: true, enumerable: true, value: token };
          }
          return undefined;
        }
      });
      lists.set(element, proxy);
      return proxy;
    }
    Object.defineProperty(elementPrototype, "part", {
      configurable: true, enumerable: true,
      get: function () { return partTokens(this); }
    });
  }

  if (typeof g.document !== "undefined") {
    installPermissions();
    installOptionConstructor();
    installQueryCommandSupported();
    installReportingObserver();
    installTrustedTypes();
    installSpeechSynthesis();
    const elementPrototype = g.Element && g.Element.prototype;
    if (elementPrototype) {
      installElementScrolling(elementPrototype);
      installRequestFullscreen(elementPrototype);
      installCheckVisibility(elementPrototype);
      installElementAnimations(elementPrototype);
      installPartTokens(elementPrototype);
    }
  }

  Object.defineProperty(g, "__chuzzPlatform", {
    value: Object.freeze({ Target, exception, event, message, info })
  });
})(globalThis);


