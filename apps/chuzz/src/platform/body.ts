// Fetch bodies and file objects share the page's byte-preserving Blob storage.
(function (g) {
  "use strict";
  const streams = g.__chuzzStreams;
  const platform = g.__chuzzPlatform;
  const encoder = new TextEncoder();
  const bytesOfBlob = g.__chuzzBlobBytes;
  const usv = value => new TextDecoder().decode(encoder.encode(String(value)));
  const headersState = new WeakMap();
  const forbidden = new Set([
    "accept-charset", "accept-encoding", "access-control-request-headers",
    "access-control-request-method", "connection", "content-length", "cookie", "cookie2",
    "date", "dnt", "expect", "host", "keep-alive", "origin", "referer", "te", "trailer",
    "transfer-encoding", "upgrade", "via"
  ]);
  function headerName(value) {
    value = String(value).toLowerCase();
    if (!/^[!#$%&'*+.^_`|~0-9a-z-]+$/.test(value)) throw new TypeError("Invalid header name");
    return value;
  }
  function headerValue(value) {
    value = String(value).replace(/^[\t ]+|[\t ]+$/g, "");
    if (/[\r\n\0]/.test(value) || /[^\u0000-\u00ff]/.test(value)) {
      throw new TypeError("Invalid header value");
    }
    return value;
  }
  function allowed(state, name, value) {
    if (state.guard === "immutable") throw new TypeError("Headers are immutable");
    if (state.guard === "response" && (name === "set-cookie" || name === "set-cookie2")) return false;
    if (state.guard.startsWith("request")) {
      if (forbidden.has(name) || name.startsWith("proxy-") || name.startsWith("sec-")) return false;
      if (/^x-http-method(?:-override)?$|^x-method-override$/.test(name)
          && value.split(",").some(method => /^(CONNECT|TRACE|TRACK)$/i.test(method.trim()))) return false;
    }
    if (state.guard === "request-no-cors") {
      if (!["accept", "accept-language", "content-language", "content-type"].includes(name)) return false;
      if (value.length > 128) return false;
      if (name === "accept" || name === "content-type") {
        if (/[\u0000-\u0008\u000a-\u001f\u007f"():<>?@[\]\\{}]/.test(value)) return false;
      } else if (!/^[0-9A-Za-z *,.\-;=]*$/.test(value)) return false;
      if (name === "content-type" && ![
        "application/x-www-form-urlencoded", "multipart/form-data", "text/plain"
      ].includes(value.split(";")[0].trim().toLowerCase())) return false;
    }
    return true;
  }
  function initializeHeaders(target, input, guard) {
    const state = { values: new Map(), guard };
    headersState.set(target, state);
    if (input == null) return;
    if (typeof input[Symbol.iterator] === "function") {
      for (const pair of input) {
        const values = Array.from(pair);
        if (values.length !== 2) throw new TypeError("A header entry must contain two values");
        target.append(values[0], values[1]);
      }
    } else {
      for (const name of Object.keys(input)) target.append(name, input[name]);
    }
  }
  function makeHeaders(input, guard) {
    const result = Object.create(Headers.prototype);
    initializeHeaders(result, input, guard);
    return result;
  }
  class Headers {
    constructor(input) { initializeHeaders(this, input, "none"); }
    append(name, value) {
      name = headerName(name);
      value = headerValue(value);
      const state = headersState.get(this);
      const previous = state.values.get(name);
      if (!allowed(state, name, previous ? previous.join(", ") + ", " + value : value)) return;
      state.values.set(name, previous ? previous.concat(value) : [value]);
    }
    set(name, value) {
      name = headerName(name);
      value = headerValue(value);
      const state = headersState.get(this);
      if (allowed(state, name, value)) state.values.set(name, [value]);
    }
    delete(name) {
      name = headerName(name);
      const state = headersState.get(this);
      if (allowed(state, name, state.values.get(name)?.join(", ") || "")) state.values.delete(name);
    }
    get(name) { return headersState.get(this).values.get(headerName(name))?.join(", ") ?? null; }
    has(name) { return headersState.get(this).values.has(headerName(name)); }
    getSetCookie() { return (headersState.get(this).values.get("set-cookie") || []).slice(); }
    *entries() {
      const list = () => Array.from(headersState.get(this).values.keys()).sort().flatMap(name =>
        name === "set-cookie" ? headersState.get(this).values.get(name).map(value => [name, value])
          : [[name, headersState.get(this).values.get(name).join(", ")]]);
      let index = 0;
      while (index < list().length) yield list()[index++];
    }
    *keys() { for (const [name] of this) yield name; }
    *values() { for (const [, value] of this) yield value; }
    [Symbol.iterator]() { return this.entries(); }
    forEach(callback, receiver) {
      for (const [name, value] of this) callback.call(receiver, value, name, this);
    }
  }

  class File extends Blob {
    constructor(parts, name, options = {}) {
      if (arguments.length < 2) throw new TypeError("File requires parts and a name");
      if (typeof name === "symbol") throw new TypeError("File name must be a string");
      options = options || {};
      name = usv(name).replace(/\//g, ":");
      super(parts, options);
      const modified = options.lastModified === undefined ? Date.now() : Number(options.lastModified);
      Object.defineProperties(this, {
        name: { enumerable: true, value: usv(name) },
        lastModified: { enumerable: true, value: Number.isFinite(modified) ? Math.trunc(modified) : 0 },
        webkitRelativePath: { enumerable: true, value: "" }
      });
    }
  }
  const fileLists = new WeakMap();
  class FileList {
    constructor() { throw new TypeError("Illegal constructor"); }
    get length() { return fileLists.get(this).length; }
    item(index) { return fileLists.get(this)[Number(index) >>> 0] || null; }
    [Symbol.iterator]() { return fileLists.get(this)[Symbol.iterator](); }
  }
  function fileList(files) {
    const result = Object.create(FileList.prototype);
    files = Array.from(files);
    fileLists.set(result, files);
    files.forEach((file, index) => Object.defineProperty(result, index, {
      enumerable: true, value: file
    }));
    return result;
  }
  const forms = new WeakMap();
  function formValue(value, filename) {
    if (value instanceof Blob) {
      return new File([value], filename === undefined
        ? value instanceof File ? value.name : "blob" : filename, {
        type: value.type, lastModified: value instanceof File ? value.lastModified : Date.now()
      });
    }
    if (filename !== undefined) throw new TypeError("A filename requires a Blob");
    return usv(value);
  }
  function disabled(control) {
    if (control.hasAttribute("disabled")) return true;
    let parent = control.parentElement;
    while (parent) {
      if (parent.tagName === "FIELDSET" && parent.hasAttribute("disabled")) {
        const legend = Array.from(parent.children).find(child => child.tagName === "LEGEND");
        if (!legend || !legend.contains(control)) return true;
      }
      parent = parent.parentElement;
    }
    return false;
  }
  class FormData {
    constructor(form, submitter) {
      forms.set(this, []);
      if (form === undefined) return;
      if (!form || form.tagName !== "FORM") throw new TypeError("A form element is required");
      if (submitter != null && !["submit", "image"].includes(
        String(submitter.getAttribute("type") || (submitter.tagName === "BUTTON" ? "submit" : "text"))
          .toLowerCase())) throw new TypeError("Invalid submitter");
      const controls = form.isConnected
        ? form.ownerDocument.querySelectorAll("input,select,textarea,button")
        : form.querySelectorAll("input,select,textarea,button");
      let ownsSubmitter = submitter == null;
      for (const control of controls) {
        const owner = control.getAttribute("form");
        if (owner !== null ? !form.id || owner !== form.id : control.closest("form") !== form) continue;
        if (control === submitter) ownsSubmitter = true;
        if (disabled(control) || control.closest("datalist")) continue;
        const name = control.getAttribute("name") || "";
        const type = String(control.getAttribute("type")
          || (control.tagName === "BUTTON" ? "submit" : "text")).toLowerCase();
        if (["submit", "reset", "button", "image"].includes(type) && control !== submitter) continue;
        if (type === "image") {
          this.append(name ? name + ".x" : "x", "0");
          this.append(name ? name + ".y" : "y", "0");
          continue;
        }
        if (!name) continue;
        if (["checkbox", "radio"].includes(type) && !control.checked) continue;
        if (control.tagName === "SELECT") {
          for (const option of control.querySelectorAll("option")) {
            if (option.selected && !option.hasAttribute("disabled")
                && !(option.parentElement?.tagName === "OPTGROUP"
                  && option.parentElement.hasAttribute("disabled"))) {
              this.append(name, option.getAttribute("value") ?? option.textContent.trim());
            }
          }
        } else if (type === "file") {
          const files = control.files || fileList([]);
          if (!files.length) this.append(name, new File([], "", { type: "application/octet-stream" }));
          else for (const file of files) this.append(name, file);
        } else {
          const value = name === "_charset_" && type === "hidden" ? "UTF-8"
            : ["checkbox", "radio"].includes(type)
              ? control.getAttribute("value") ?? "on"
              : control.value ?? control.getAttribute("value") ?? control.textContent ?? "";
          this.append(name, value);
          if (control.hasAttribute("dirname")) {
            this.append(control.getAttribute("dirname"), control.dir === "rtl" ? "rtl" : "ltr");
          }
        }
      }
      if (!ownsSubmitter) throw platform.exception("NotFoundError", "Submitter belongs to another form");
    }
    append(name, value, filename) { forms.get(this).push([usv(name), formValue(value, filename)]); }
    get(name) { return forms.get(this).find(entry => entry[0] === usv(name))?.[1] ?? null; }
    getAll(name) { name = usv(name); return forms.get(this).filter(entry => entry[0] === name).map(entry => entry[1]); }
    has(name) { name = usv(name); return forms.get(this).some(entry => entry[0] === name); }
    delete(name) {
      name = usv(name);
      const entries = forms.get(this);
      for (let index = entries.length - 1; index >= 0; index--) {
        if (entries[index][0] === name) entries.splice(index, 1);
      }
    }
    set(name, value, filename) {
      name = usv(name);
      value = formValue(value, filename);
      const entries = forms.get(this);
      const first = entries.findIndex(entry => entry[0] === name);
      if (first < 0) { entries.push([name, value]); return; }
      entries[first] = [name, value];
      for (let index = entries.length - 1; index > first; index--) {
        if (entries[index][0] === name) entries.splice(index, 1);
      }
    }
    *entries() {
      let index = 0;
      while (index < forms.get(this).length) yield forms.get(this)[index++].slice();
    }
    *keys() { for (const [name] of this) yield name; }
    *values() { for (const [, value] of this) yield value; }
    [Symbol.iterator]() { return this.entries(); }
    forEach(callback, receiver) {
      for (const [name, value] of this) callback.call(receiver, value, name, this);
    }
  }
  function join(chunks) {
    const length = chunks.reduce((total, chunk) => total + chunk.length, 0);
    if (length > 64 * 1024 * 1024) throw new RangeError("Body allocation limit exceeded");
    const result = new Uint8Array(length);
    let offset = 0;
    for (const chunk of chunks) { result.set(chunk, offset); offset += chunk.length; }
    return result;
  }
  const newline = text => text.replace(/\r\n|\r|\n/g, "\r\n");
  const quoted = text => newline(text).replace(/\r/g, "%0D").replace(/\n/g, "%0A").replace(/"/g, "%22");
  function multipart(form) {
    const random = g.crypto.getRandomValues(new Uint8Array(16));
    const boundary = "----chuzz" + Array.from(random, value => value.toString(16).padStart(2, "0")).join("");
    const chunks = [];
    for (const [name, value] of form) {
      let header = "--" + boundary + "\r\nContent-Disposition: form-data; name=\"" + quoted(name) + "\"";
      if (value instanceof Blob) {
        header += "; filename=\"" + quoted(value.name) + "\"\r\nContent-Type: "
          + (value.type || "application/octet-stream");
      }
      chunks.push(encoder.encode(header + "\r\n\r\n"));
      chunks.push(value instanceof Blob ? bytesOfBlob(value) : encoder.encode(newline(value)));
      chunks.push(encoder.encode("\r\n"));
    }
    chunks.push(encoder.encode("--" + boundary + "--\r\n"));
    return { bytes: join(chunks), type: "multipart/form-data; boundary=" + boundary };
  }
  function extract(body) {
    if (body == null) return { bytes: null, stream: null, type: null };
    if (body instanceof ReadableStream) {
      if (body.locked || streams.disturbed(body)) throw new TypeError("Body stream is unavailable");
      return { bytes: null, stream: body, type: null };
    }
    let bytes, type = null;
    if (body instanceof Blob) { bytes = bytesOfBlob(body).slice(); type = body.type || null; }
    else if (body instanceof FormData) {
      const encoded = multipart(body);
      bytes = encoded.bytes;
      type = encoded.type;
    } else if (body instanceof ArrayBuffer) bytes = new Uint8Array(body).slice();
    else if (ArrayBuffer.isView(body)) bytes = new Uint8Array(body.buffer, body.byteOffset, body.byteLength).slice();
    else if (body instanceof URLSearchParams) {
      bytes = encoder.encode(String(body));
      type = "application/x-www-form-urlencoded;charset=UTF-8";
    } else { bytes = encoder.encode(usv(body)); type = "text/plain;charset=UTF-8"; }
    return { bytes, stream: streams.fromBytes(bytes), type };
  }
  const bodies = new WeakMap();
  function installBody(target, body, headers) {
    const state = extract(body);
    if (state.type && !headers.has("content-type")) headers.append("content-type", state.type);
    bodies.set(target, state);
  }
  function consumed(target) {
    const stream = bodies.get(target).stream;
    return !!stream && streams.disturbed(stream);
  }
  async function consume(target, signal) {
    const stream = bodies.get(target).stream;
    if (!stream) return new Uint8Array(0);
    if (stream.locked || streams.disturbed(stream)) throw new TypeError("Body has already been consumed");
    const reader = stream.getReader();
    let abort;
    const aborted = new Promise((resolve, reject) => {
      if (!signal) return;
      abort = () => { reader.cancel(signal.reason).catch(() => {}); reject(signal.reason); };
      if (signal.aborted) abort();
      else signal.addEventListener("abort", abort, { once: true });
    });
    const chunks = [];
    let length = 0;
    try {
      while (true) {
        const result = await Promise.race([reader.read(), aborted]);
        if (result.done) return join(chunks);
        if (!(result.value instanceof Uint8Array)) throw new TypeError("A body chunk must be Uint8Array");
        length += result.value.length;
        if (length > 64 * 1024 * 1024) throw new RangeError("Body allocation limit exceeded");
        chunks.push(result.value.slice());
      }
    } catch (error) {
      await reader.cancel(error).catch(() => {});
      throw error;
    } finally {
      if (signal && abort) signal.removeEventListener("abort", abort);
      reader.releaseLock();
    }
  }
  function parseForm(bytes, type) {
    const form = new FormData();
    if (/^application\/x-www-form-urlencoded(?:;|$)/i.test(type)) {
      for (const [name, value] of new URLSearchParams(new TextDecoder().decode(bytes))) form.append(name, value);
      return form;
    }
    const match = /^multipart\/form-data\s*;[\s\S]*?\bboundary=(?:"([^"]+)"|([^;\s]+))/i.exec(type);
    if (!match) throw new TypeError("Body is not encoded form data");
    const boundary = match[1] || match[2];
    let binary = "";
    for (let offset = 0; offset < bytes.length; offset += 8192) {
      binary += String.fromCharCode(...bytes.subarray(offset, offset + 8192));
    }
    const marker = "--" + boundary;
    let position = binary.indexOf(marker);
    if (position < 0) throw new TypeError("Missing multipart boundary");
    const decode = text => new TextDecoder().decode(Uint8Array.from(text, value => value.charCodeAt(0)));
    while (true) {
      position += marker.length;
      if (binary.slice(position, position + 2) === "--") return form;
      if (binary.slice(position, position + 2) !== "\r\n") throw new TypeError("Invalid multipart delimiter");
      position += 2;
      const endHeaders = binary.indexOf("\r\n\r\n", position);
      if (endHeaders < 0) throw new TypeError("Invalid multipart headers");
      const headers = new Headers();
      for (const line of binary.slice(position, endHeaders).split("\r\n")) {
        const colon = line.indexOf(":");
        if (colon <= 0) throw new TypeError("Invalid multipart header");
        headers.append(line.slice(0, colon), line.slice(colon + 1));
      }
      const disposition = headers.get("content-disposition") || "";
      const name = /(?:^|;)\s*name="((?:\\.|[^"])*)"/i.exec(disposition);
      if (!/^form-data(?:;|$)/i.test(disposition) || !name) throw new TypeError("Invalid form disposition");
      const filename = /(?:^|;)\s*filename="((?:\\.|[^"])*)"/i.exec(disposition);
      const start = endHeaders + 4;
      let next = binary.indexOf("\r\n" + marker, start);
      while (next >= 0 && !["--", "\r\n"].includes(
        binary.slice(next + 2 + marker.length, next + 4 + marker.length))) {
        next = binary.indexOf("\r\n" + marker, next + 2);
      }
      if (next < 0) throw new TypeError("Unterminated multipart body");
      let data = Uint8Array.from(binary.slice(start, next), value => value.charCodeAt(0));
      if ((headers.get("content-transfer-encoding") || "").toLowerCase() === "base64") {
        data = Uint8Array.from(g.atob(binary.slice(start, next)), value => value.charCodeAt(0));
      }
      const key = decode(name[1].replace(/\\(.)/g, "$1"));
      if (filename) {
        form.append(key, new File([data], decode(filename[1].replace(/\\(.)/g, "$1")), {
          type: headers.get("content-type") || "text/plain"
        }));
      } else form.append(key, new TextDecoder().decode(data));
      position = next + 2;
    }
  }
  function bodyMethods(prototype) {
    Object.defineProperties(prototype, {
      body: { configurable: true, enumerable: true, get() { return bodies.get(this).stream; } },
      bodyUsed: { configurable: true, enumerable: true, get() { return consumed(this); } }
    });
    prototype.arrayBuffer = function () { return consume(this).then(bytes => bytes.buffer); };
    prototype.text = function () { return consume(this).then(bytes => new TextDecoder().decode(bytes)); };
    prototype.json = function () { return this.text().then(JSON.parse); };
    prototype.blob = function () {
      const type = this.headers.get("content-type") || "";
      return consume(this).then(bytes => new Blob([bytes], { type }));
    };
    prototype.formData = function () {
      const type = this.headers.get("content-type") || "";
      return consume(this).then(bytes => parseForm(bytes, type));
    };
  }
  const responseState = new WeakMap();
  function response(body, options, metadata) {
    const result = Object.create(Response.prototype);
    const status = options.status === undefined ? 200 : Number(options.status);
    if (!metadata && (!Number.isInteger(status) || status < 200 || status > 599)) {
      throw new RangeError("Invalid response status");
    }
    const statusText = String(options.statusText || "");
    if (/[\r\n]/.test(statusText) || /[^\u0000-\u00ff]/.test(statusText)) throw new TypeError("Invalid statusText");
    if ([101, 204, 205, 304].includes(status) && body != null) throw new TypeError("Status cannot have a body");
    const headers = makeHeaders(options.headers, "response");
    installBody(result, body, headers);
    if (metadata?.immutable) headersState.get(headers).guard = "immutable";
    responseState.set(result, {
      status, statusText, headers, url: metadata?.url || "", type: metadata?.type || "default",
      redirected: !!metadata?.redirected
    });
    return result;
  }
  class Response {
    constructor(body = null, options = {}) { return response(body, options || {}, null); }
    get status() { return responseState.get(this).status; }
    get statusText() { return responseState.get(this).statusText; }
    get headers() { return responseState.get(this).headers; }
    get url() { return responseState.get(this).url; }
    get type() { return responseState.get(this).type; }
    get redirected() { return responseState.get(this).redirected; }
    get ok() { return this.status >= 200 && this.status <= 299; }
    clone() {
      if (this.bodyUsed || this.body?.locked) throw new TypeError("Body has already been consumed");
      const result = Object.create(Response.prototype);
      responseState.set(result, { ...responseState.get(this), headers: makeHeaders(this.headers, "none") });
      headersState.get(result.headers).guard = headersState.get(this.headers).guard;
      cloneBody(this, result);
      return result;
    }
    static json(value, options = {}) {
      const text = JSON.stringify(value);
      if (text === undefined) throw new TypeError("Value cannot be serialized as JSON");
      const headers = new Headers(options.headers);
      if (!headers.has("content-type")) headers.set("content-type", "application/json");
      return new Response(text, { ...options, headers });
    }
    static error() {
      return response(null, { status: 0 }, { type: "error", immutable: true });
    }
    static redirect(url, status = 302) {
      status = Number(status);
      if (![301, 302, 303, 307, 308].includes(status)) throw new RangeError("Invalid redirect status");
      const address = new URL(String(url), g.location.href).href;
      return response(null, { status, headers: { location: address } }, { immutable: true });
    }
  }
  function cloneBody(source, destination) {
    if (!source.body) { bodies.set(destination, { stream: null, bytes: null }); return; }
    const branches = streams.tee(source.body, true);
    bodies.set(source, { stream: branches[0], bytes: null });
    bodies.set(destination, { stream: branches[1], bytes: null });
  }
  const requestState = new WeakMap();
  function method(value) {
    value = String(value);
    if (!/^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/.test(value)) throw new TypeError("Invalid HTTP method");
    if (/^(CONNECT|TRACE|TRACK)$/i.test(value)) throw new TypeError("Forbidden HTTP method");
    return /^(DELETE|GET|HEAD|OPTIONS|POST|PUT)$/i.test(value) ? value.toUpperCase() : value;
  }
  class Request {
    constructor(input, options = {}) {
      options = options || {};
      const source = input instanceof Request ? input : null;
      const url = new URL(source ? source.url : String(input), g.location.href);
      if (url.username || url.password) throw new TypeError("URL cannot contain credentials");
      url.hash = "";
      const verb = method(options.method ?? source?.method ?? "GET");
      const mode = String(options.mode ?? source?.mode ?? "cors");
      if (!["cors", "same-origin", "no-cors"].includes(mode)) throw new TypeError("Invalid request mode");
      if (mode === "no-cors" && !["GET", "HEAD", "POST"].includes(verb)) throw new TypeError("Invalid no-cors method");
      const credentials = String(options.credentials ?? source?.credentials ?? "same-origin");
      if (!["omit", "same-origin", "include"].includes(credentials)) throw new TypeError("Invalid credentials");
      const redirect = String(options.redirect ?? source?.redirect ?? "follow");
      if (!["follow", "error", "manual"].includes(redirect)) throw new TypeError("Invalid redirect mode");
      const headers = makeHeaders(options.headers === undefined ? source?.headers : options.headers,
        mode === "no-cors" ? "request-no-cors" : "request");
      let body = options.body;
      const inherited = body === undefined && source && source.body;
      if ((verb === "GET" || verb === "HEAD") && (body != null || inherited)) {
        throw new TypeError("GET and HEAD requests cannot have a body");
      }
      if (body instanceof ReadableStream && options.duplex !== "half") throw new TypeError("Streaming body requires duplex: half");
      installBody(this, body === undefined ? null : body, headers);
      if (inherited) {
        if (source.bodyUsed || source.body.locked) throw new TypeError("Body has already been consumed");
        const reader = source.body.getReader();
        streams.disturb(source.body);
        const stream = new ReadableStream({
          async pull(controller) {
            try {
              const result = await reader.read();
              if (result.done) { controller.close(); reader.releaseLock(); }
              else controller.enqueue(result.value);
            } catch (error) { controller.error(error); reader.releaseLock(); }
          },
          cancel(reason) { return reader.cancel(reason).finally(() => reader.releaseLock()); }
        });
        bodies.set(this, { stream, bytes: null });
      }
      const controller = typeof AbortController === "function" ? new AbortController() : null;
      const originalSignal = options.signal === undefined ? source?.signal : options.signal;
      if (originalSignal && controller) {
        if (originalSignal.aborted) controller.abort(originalSignal.reason);
        else originalSignal.addEventListener("abort", () => controller.abort(originalSignal.reason), { once: true });
      }
      requestState.set(this, {
        url: url.href, method: verb, headers, mode, credentials, redirect,
        signal: controller ? controller.signal : originalSignal || null
      });
    }
    get url() { return requestState.get(this).url; }
    get method() { return requestState.get(this).method; }
    get headers() { return requestState.get(this).headers; }
    get signal() { return requestState.get(this).signal; }
    get credentials() { return requestState.get(this).credentials; }
    get mode() { return requestState.get(this).mode; }
    get redirect() { return requestState.get(this).redirect; }
    get duplex() { return "half"; }
    clone() {
      if (this.bodyUsed || this.body?.locked) throw new TypeError("Body has already been consumed");
      const result = new Request(this.url, {
        method: this.method, headers: this.headers, signal: this.signal,
        mode: this.mode, credentials: this.credentials, redirect: this.redirect
      });
      cloneBody(this, result);
      return result;
    }
  }
  bodyMethods(Request.prototype);
  bodyMethods(Response.prototype);
  Blob.prototype.stream = function () {
    const bytes = bytesOfBlob(this);
    if (!bytes) throw new TypeError("Invalid Blob");
    let offset = 0;
    return new ReadableStream({
      pull(controller) {
        if (offset >= bytes.length) { controller.close(); return; }
        const next = Math.min(bytes.length, offset + 65536);
        controller.enqueue(bytes.slice(offset, next));
        offset = next;
      }
    });
  };
  class FileReader extends platform.Target {
    constructor() {
      super();
      this._state = 0;
      this._result = null;
      this._error = null;
      this._generation = 0;
      for (const type of ["loadstart", "progress", "load", "error", "abort", "loadend"]) this["on" + type] = null;
    }
    get readyState() { return this._state; }
    get result() { return this._result; }
    get error() { return this._error; }
    _fire(type, loaded, total) {
      const options = { loaded, total, lengthComputable: total !== undefined };
      const event = typeof g.ProgressEvent === "function"
        ? new g.ProgressEvent(type, options) : platform.event(type, options);
      this.dispatchEvent(event);
    }
    _read(blob, kind, label) {
      if (!(blob instanceof Blob)) throw new TypeError("A Blob is required");
      if (this._state === 1) throw platform.exception("InvalidStateError", "Read already in progress");
      this._state = 1;
      this._result = this._error = null;
      const generation = ++this._generation;
      g.setTimeout(() => {
        if (generation !== this._generation) return;
        this._fire("loadstart", 0, blob.size);
        if (generation !== this._generation) return;
        blob.arrayBuffer().then(buffer => {
          if (generation !== this._generation) return;
          if (kind === "array") this._result = buffer;
          else if (kind === "text") this._result = new TextDecoder(label || "utf-8").decode(buffer);
          else {
            const bytes = new Uint8Array(buffer);
            let binary = "";
            for (let offset = 0; offset < bytes.length; offset += 8192) {
              binary += String.fromCharCode(...bytes.subarray(offset, offset + 8192));
            }
            this._result = "data:" + (blob.type || "application/octet-stream") + ";base64," + g.btoa(binary);
          }
          this._fire("progress", blob.size, blob.size);
          if (generation !== this._generation) return;
          this._state = 2;
          this._fire("load", blob.size, blob.size);
          if (generation === this._generation) this._fire("loadend", blob.size, blob.size);
        }).catch(error => {
          if (generation !== this._generation) return;
          this._state = 2;
          this._result = null;
          this._error = error;
          this._fire("error", 0, blob.size);
          if (generation === this._generation) this._fire("loadend", 0, blob.size);
        });
      }, 0);
    }
    readAsText(blob, label) { this._read(blob, "text", label); }
    readAsArrayBuffer(blob) { this._read(blob, "array"); }
    readAsDataURL(blob) { this._read(blob, "url"); }
    abort() {
      const loading = this._state === 1;
      this._generation++;
      this._result = null;
      if (!loading) return;
      this._state = 2;
      this._error = null;
      this._fire("abort", 0, undefined);
      this._fire("loadend", 0, undefined);
    }
  }
  for (const [name, value] of [["EMPTY", 0], ["LOADING", 1], ["DONE", 2]]) {
    Object.defineProperty(FileReader, name, { value });
    Object.defineProperty(FileReader.prototype, name, { value });
  }
  for (const Constructor of [Headers, Request, Response, FormData, File, FileList, FileReader]) {
    g[Constructor.name] = Constructor;
    Object.defineProperty(Constructor.prototype, Symbol.toStringTag, { value: Constructor.name });
  }
  Object.defineProperty(g, "__chuzzBodyApi", {
    value: Object.freeze({
      fileList,
      payloadSync(url, options = {}) {
        const verb = method(options.method || "GET");
        const request = new Request(url, {
          ...options, method: verb, body: verb === "GET" || verb === "HEAD" ? null : options.body
        });
        const state = bodies.get(request);
        if (state.stream && !state.bytes) throw new TypeError("XHR does not accept a stream body");
        return {
          url: request.url, method: request.method, headers: Array.from(request.headers),
          body: state.bytes ? Array.from(state.bytes) : null, credentials: request.credentials
        };
      },
      installFetch(send, forget) {
        g.fetch = function (input, init) {
          return new Promise((resolve, reject) => {
            let request;
            try { request = new Request(input, init); }
            catch (error) { reject(error); return; }
            if (request.mode === "no-cors" || request.redirect !== "follow") {
              reject(new TypeError("The network bridge requires cors/same-origin and redirect: follow"));
              return;
            }
            if (request.mode === "same-origin"
                && new URL(request.url).origin !== new URL(g.location.href).origin) {
              reject(new TypeError("Cross-origin request in same-origin mode"));
              return;
            }
            const signal = request.signal;
            let id = 0, resultResponse = null, aborted = false;
            function reason() {
              return signal.reason === undefined
                ? platform.exception("AbortError", "The operation was aborted") : signal.reason;
            }
            function cleanup() { if (signal) signal.removeEventListener("abort", abort); }
            function abort() {
              aborted = true;
              if (id) forget(id);
              cleanup();
              if (resultResponse?.body) streams.error(resultResponse.body, reason());
              reject(reason());
            }
            if (signal?.aborted) { abort(); return; }
            if (signal) signal.addEventListener("abort", abort, { once: true });
            const hasBody = request.body !== null;
            consume(request, signal).then(bytes => {
              if (aborted) return;
              id = send({
                url: request.url, method: request.method, headers: Array.from(request.headers),
                body: hasBody ? Array.from(bytes) : null, credentials: request.credentials
              }, result => {
                if (aborted) return;
                if (!result.ok) { cleanup(); reject(new TypeError("fetch failed: " + result.error)); return; }
                try {
                  const finalURL = new URL(result.url || request.url, request.url);
                  if (request.mode === "same-origin" && finalURL.origin !== new URL(request.url).origin) {
                    throw new TypeError("Cross-origin redirect in same-origin mode");
                  }
                  const body = request.method === "HEAD" || [204, 205, 304].includes(result.status)
                    ? null : new Uint8Array(result.bytes);
                  resultResponse = response(body, {
                    status: result.status, statusText: result.statusText, headers: result.headers
                  }, {
                    immutable: true, url: finalURL.href, redirected: finalURL.href !== request.url,
                    type: finalURL.origin === new URL(g.location.href).origin ? "basic" : "cors"
                  });
                  if (resultResponse.body) streams.closed(resultResponse.body).then(cleanup, cleanup);
                  else cleanup();
                  resolve(resultResponse);
                } catch (error) { cleanup(); reject(error); }
              });
            }, error => { cleanup(); reject(error); });
          });
        };
      }
    })
  });
})(globalThis);

