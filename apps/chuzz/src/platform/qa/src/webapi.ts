(function () {
  "use strict";
  const run = document.getElementById("run");
  const result = document.getElementById("result");
  function assert(value, name) { if (!value) throw new Error(name); }
  async function rejects(operation, name) {
    let rejected = false;
    try { await operation(); } catch (error) { rejected = true; }
    assert(rejected, name);
  }
  function throws(operation, name) {
    let thrown = false;
    try { operation(); } catch (error) { thrown = true; }
    assert(thrown, name);
  }
  async function collect(stream) {
    const values = [];
    for await (const value of stream) values.push(value);
    return values;
  }
  function readFile(blob, method) {
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      const events = [];
      for (const name of ["loadstart", "progress", "load", "loadend"]) {
        reader.addEventListener(name, () => events.push(name));
      }
      reader.onerror = () => reject(reader.error);
      reader.onloadend = () => resolve({ value: reader.result, events, state: reader.readyState });
      reader[method](blob);
    });
  }
  run.addEventListener("click", async () => {
    run.disabled = true;
    let worker, channel, peer;
    try {
      const headers = new Headers([["X-Test", "one"], ["x-test", "two"]]);
      assert(headers.get("X-TEST") === "one, two", "Headers combine and ignore case");
      headers.set("X-Test", "three");
      headers.append("a", "first");
      assert(Array.from(headers)[0][0] === "a" && headers.get("x-test") === "three",
        "Headers sorted iteration and set");
      headers.delete("x-test");
      assert(!headers.has("X-Test"), "Headers delete");
      throws(() => headers.get("bad name"), "Headers validate read names");
      throws(() => headers.set("x", "one\r\ntwo"), "Headers reject header injection");
      const guarded = new Request(location.href, { headers: { cookie: "forbidden", "x-test": "allowed" } });
      assert(!guarded.headers.has("cookie") && guarded.headers.get("x-test") === "allowed",
        "Request header guard");
      throws(() => new Request(location.href, { method: "GET", body: "bad" }),
        "GET rejects a body");

      const response = Response.json({ answer: 42 }, { status: 201 });
      const clone = response.clone();
      assert(response instanceof Response && response.ok && response.status === 201,
        "Response status");
      assert((await response.json()).answer === 42 && response.bodyUsed, "Response JSON consumption");
      assert((await clone.json()).answer === 42, "Response clone has independent consumption");
      await rejects(() => response.text(), "Response cannot be consumed twice");
      const errorResponse = Response.error();
      assert(errorResponse.status === 0 && !errorResponse.ok && errorResponse.type === "error"
        && errorResponse.body === null, "Response.error");
      const redirect = Response.redirect(location.href, 307);
      assert(redirect.status === 307 && redirect.headers.get("location") === location.href,
        "Response.redirect");
      throws(() => redirect.headers.set("x", "bad"), "Immutable redirect headers");
      throws(() => new Response("bad", { status: 204 }), "Null-body status validation");
      const binary = new Response(new Uint8Array([0, 255, 128]));
      const binaryReader = binary.body.getReader();
      assert((await binaryReader.read()).value[1] === 255 && binary.bodyUsed,
        "Response reader preserves bytes and disturbs body");
      assert((await binaryReader.read()).done, "Response stream reaches EOF");
      binaryReader.releaseLock();

      const file = new File([new Uint8Array([0, 255]), "hello"], "sample.bin", {
        type: "APPLICATION/OCTET-STREAM", lastModified: 123
      });
      assert(file instanceof Blob && file.name === "sample.bin" && file.lastModified === 123,
        "File metadata");
      assert(new Uint8Array(await file.slice(0, 2).arrayBuffer())[1] === 255,
        "Blob slice preserves binary");
      assert(await file.slice(2).text() === "hello", "Blob text");
      const blobChunks = await collect(file.stream());
      assert(blobChunks[0][1] === 255, "Blob stream");
      const textRead = await readFile(new Blob(["hello"]), "readAsText");
      assert(textRead.value === "hello" && textRead.state === FileReader.DONE
        && textRead.events.join(",") === "loadstart,progress,load,loadend", "FileReader text and events");
      const bufferRead = await readFile(file, "readAsArrayBuffer");
      assert(new Uint8Array(bufferRead.value)[1] === 255, "FileReader ArrayBuffer");
      const urlRead = await readFile(new Blob(["hi"], { type: "text/plain" }), "readAsDataURL");
      assert(urlRead.value === "data:text/plain;base64,aGk=", "FileReader DataURL");
      const abortedReader = new FileReader();
      const abortEvents = [];
      abortedReader.onabort = () => abortEvents.push("abort");
      abortedReader.onloadend = () => abortEvents.push("loadend");
      abortedReader.readAsText(file);
      abortedReader.abort();
      assert(abortedReader.result === null && abortEvents.join(",") === "abort,loadend",
        "FileReader abort");

      const form = new FormData(document.getElementById("sample"));
      assert(form.get("title") === "hello" && form.get("checked") === "on"
        && !form.has("excluded") && !form.has("unchecked"), "FormData successful controls");
      form.append("repeated", "one");
      form.append("repeated", "two");
      assert(form.getAll("repeated").join(",") === "one,two", "FormData getAll");
      form.set("repeated", "replacement");
      assert(form.getAll("repeated").length === 1, "FormData set replaces duplicates");
      form.delete("repeated");
      assert(!form.has("repeated"), "FormData delete");
      form.append("upload", file, "renamed.bin");
      const formRequest = new Request(location.href, { method: "POST", body: form });
      assert(formRequest.headers.get("content-type").startsWith("multipart/form-data; boundary="),
        "Multipart content type");
      const parsed = await new Response(await formRequest.arrayBuffer(), {
        headers: { "content-type": formRequest.headers.get("content-type") }
      }).formData();
      assert(parsed.get("title") === "hello" && parsed.get("upload").name === "renamed.bin"
        && new Uint8Array(await parsed.get("upload").arrayBuffer())[1] === 255,
        "Multipart names, filenames and binary round trip");
      const urlForm = await new Response("a=one&a=two&space=hello+world", {
        headers: { "content-type": "application/x-www-form-urlencoded" }
      }).formData();
      assert(urlForm.getAll("a").join(",") === "one,two" && urlForm.get("space") === "hello world",
        "URL encoded formData");

      let canceled;
      const cancelStream = new ReadableStream({ cancel(reason) { canceled = reason; } });
      const cancelReader = cancelStream.getReader();
      throws(() => cancelStream.getReader(), "Reader lock");
      await cancelReader.cancel("stop");
      assert(canceled === "stop" && (await cancelReader.read()).done, "Readable cancellation");
      cancelReader.releaseLock();
      let count = 0;
      const source = new ReadableStream({
        pull(controller) {
          if (count < 3) controller.enqueue(++count);
          else controller.close();
        }
      });
      const branches = source.tee();
      const branchValues = await Promise.all(branches.map(collect));
      assert(branchValues[0].join(",") === "1,2,3" && branchValues[1].join(",") === "1,2,3",
        "Readable tee");
      const transformed = new ReadableStream({
        start(controller) { controller.enqueue("hello"); controller.close(); }
      }).pipeThrough(new TransformStream({
        transform(value, controller) { controller.enqueue(value.toUpperCase()); }
      }));
      const output = [];
      await transformed.pipeTo(new WritableStream({
        write(value) { output.push(value); }, close() { output.push("closed"); }
      }));
      assert(output.join(",") === "HELLO,closed", "Transform and pipeTo close");
      const failed = new ReadableStream({ start(controller) { controller.error(new Error("source failed")); } });
      await rejects(() => collect(failed), "Readable errors reject reads");
      const sink = new WritableStream({ write() { throw new Error("sink failed"); } });
      const writer = sink.getWriter();
      await rejects(() => writer.write("x"), "Writable error rejection");
      await rejects(() => writer.closed, "Writable closed rejects");
      writer.releaseLock();

      const unicode = new ReadableStream({
        start(controller) { controller.enqueue("\uD83D"); controller.enqueue("\uDE00"); controller.close(); }
      }).pipeThrough(new TextEncoderStream()).pipeThrough(new TextDecoderStream());
      assert((await collect(unicode)).join("") === "\uD83D\uDE00", "Encoding streams preserve split surrogates");
      for (const format of ["gzip", "deflate", "deflate-raw"]) {
        const restored = new Blob(["compressed hello"]).stream()
          .pipeThrough(new CompressionStream(format))
          .pipeThrough(new DecompressionStream(format));
        assert(await new Response(restored).text() === "compressed hello", format + " compression round trip");
      }
      await rejects(() => new Response(new Blob([new Uint8Array([1, 2, 3])]).stream()
        .pipeThrough(new DecompressionStream("gzip"))).text(), "Invalid compressed input rejects");

      const fetched = await fetch(new Request("./payload.json"));
      assert(fetched instanceof Response && fetched.body instanceof ReadableStream
        && fetched.status === 200 && fetched.type === "basic", "fetch returns a real Response");
      throws(() => fetched.headers.set("x", "bad"), "Fetched headers immutable");
      assert((await fetched.json()).webapi === true, "fetch Request and JSON bytes");
      const abortController = new AbortController();
      abortController.abort("fixture abort");
      await rejects(() => fetch("./payload.json", { signal: abortController.signal }), "fetch abort");

      const channelName = "webapi-" + crypto.randomUUID();
      channel = new BroadcastChannel(channelName);
      peer = new BroadcastChannel(channelName);
      const peerMessage = new Promise(resolve => { peer.onmessage = resolve; });
      const map = new Map([["bytes", new Uint8Array([1, 255])]]);
      channel.postMessage(map);
      map.get("bytes")[1] = 0;
      const received = await peerMessage;
      assert(received.data instanceof Map && received.data.get("bytes")[1] === 255
        && received.origin === origin, "Broadcast clone and origin");
      worker = new Worker("./src/webapi-worker.ts");
      const workerReady = new Promise((resolve, reject) => {
        worker.onmessage = event => {
          if (event.data === "ready") resolve();
          else if (event.data && event.data.error) reject(new Error(event.data.error));
        };
        worker.onerror = event => reject(new Error(event.message));
      });
      worker.postMessage({ name: channelName });
      await workerReady;
      const workerEcho = new Promise(resolve => {
        channel.onmessage = event => { if (event.data.worker) resolve(event.data); };
      });
      peer.postMessage({ ping: "worker" });
      assert((await workerEcho).worker === "worker", "Broadcast reaches worker realm");

      const order = [];
      let releaseFirst;
      const held = new Promise(resolve => { releaseFirst = resolve; });
      const first = navigator.locks.request(channelName, async lock => {
        assert(lock.name === channelName && lock.mode === "exclusive", "Lock metadata");
        order.push("first");
        await held;
      });
      await Promise.resolve();
      const unavailable = await navigator.locks.request(channelName, { ifAvailable: true }, lock => lock);
      assert(unavailable === null, "Lock ifAvailable");
      const second = navigator.locks.request(channelName, () => { order.push("second"); return 42; });
      const snapshot = await navigator.locks.query();
      assert(snapshot.held.some(lock => lock.name === channelName)
        && snapshot.pending.some(lock => lock.name === channelName), "Lock query");
      releaseFirst();
      await first;
      assert(await second === 42 && order.join(",") === "first,second", "Lock ordering and callback value");
      const shared = await Promise.all([1, 2].map(value =>
        navigator.locks.request(channelName, { mode: "shared" }, lock => {
          assert(lock.mode === "shared", "Shared lock");
          return value;
        })));
      assert(shared.join(",") === "1,2", "Shared locks");

      assert(navigator.hardwareConcurrency >= 1 && navigator.maxTouchPoints === 0
        && navigator.webdriver === false && navigator.javaEnabled() === false,
        "Navigator values");
      assert(navigator.plugins.length === 0 && navigator.plugins.item(0) === null
        && navigator.mimeTypes.namedItem("application/pdf") === null, "Empty plugin lists");
      const hints = await navigator.userAgentData.getHighEntropyValues(["architecture", "bitness"]);
      assert(hints.platform === navigator.userAgentData.platform
        && ["32", "64"].includes(hints.bitness), "User agent platform hints");
      assert(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(crypto.randomUUID()),
        "Secure UUID version and variant");
      window.name = 123;
      assert(window.name === "123" && !closed && opener === null && frameElement === null
        && origin === new URL(location.href).origin, "Window identity");
      let synchronous = true;
      const selfMessage = new Promise(resolve => {
        function listener(event) {
          if (event.data && event.data.webapiSelf) {
            removeEventListener("message", listener);
            resolve(event);
          }
        }
        addEventListener("message", listener);
      });
      postMessage({ webapiSelf: true }, origin);
      synchronous = false;
      const selfEvent = await selfMessage;
      assert(!synchronous && selfEvent.source === window && selfEvent.origin === origin,
        "Window postMessage source and origin");
      const marker = new Error("WEBAPI reported error");
      let reported = null;
      function errorListener(event) {
        if (event.error === marker) { reported = event.error; event.preventDefault(); }
      }
      addEventListener("error", errorListener);
      reportError(marker);
      removeEventListener("error", errorListener);
      assert(reported === marker, "reportError dispatch");
      assert(confirm("WEBAPI confirm") === false && prompt("WEBAPI prompt") === null,
        "Headless dialogs");
      assert(new Audio("sample.wav").getAttribute("src") === "sample.wav", "Audio constructor");
      const previousState = history.state;
      history.replaceState({ webapi: 42 }, "");
      assert(history.state.webapi === 42, "history.state");
      history.replaceState(previousState, "");
      assert(Number.isFinite(performance.timeOrigin) && performance.now() >= 0,
        "Performance origin");
      scrollTo(0, 100);
      assert(scrollY > 0 && visualViewport.pageTop === scrollY
        && visualViewport.width === innerWidth, "Viewport scroll and visual viewport");
      scroll(0, 0);
      assert(scrollY === 0, "scroll alias");
      result.textContent = "READY: webapi PASS";
    } catch (error) {
      result.textContent = "FAIL: " + String(error && error.message || error);
    } finally {
      if (worker) worker.terminate();
      if (channel) channel.close();
      if (peer) peer.close();
      run.disabled = false;
    }
  });
})();

