"use strict";

(function () {
  const run = document.getElementById("run");
  const result = document.getElementById("result");
  const report = (message) => { result.textContent = message; };
  const requireApi = (condition, name) => {
    if (!condition) throw new Error("missing capability: " + name);
  };
  const assert = (condition, name) => {
    if (!condition) throw new Error("failed: " + name);
  };

  run.addEventListener("click", async () => {
    run.disabled = true;
    let phase = "fetch and response bodies";
    try {
      requireApi(typeof fetch === "function", "fetch");
      requireApi(typeof Headers === "function", "Headers");
      requireApi(typeof Request === "function", "Request");
      requireApi(typeof Response === "function", "Response");
      const headers = new Headers([["x-w38", "one"]]);
      headers.append("x-w38", "two");
      assert(headers.get("X-W38") === "one, two", "Headers append and case folding");
      const req = new Request(location.href, { headers });
      assert(req.headers.get("x-w38") === "one, two", "Request headers");
      const response = new Response(new Uint8Array([0, 255, 128]), { status: 201, headers });
      assert(response.status === 201 && new Uint8Array(await response.arrayBuffer())[1] === 255,
        "Response status and raw bytes");
      const fetched = await fetch("./payload.json", { cache: "no-store" });
      assert(fetched.ok && (await fetched.json()).webapi === true, "real same-origin fetch body");

      phase = "events";
      report("Checking: " + phase);
      requireApi(typeof Event === "function" && typeof CustomEvent === "function", "Event/CustomEvent");
      const root = document.getElementById("fixture-root");
      const child = document.getElementById("fixture-child");
      const path = [];
      root.addEventListener("w38", (event) => path.push(...event.composedPath()));
      child.dispatchEvent(new CustomEvent("w38", { bubbles: true, composed: true, detail: 7 }));
      assert(path[0] === child && path.includes(root), "event composed path and bubbling");
      phase = "observers";
      report("Checking: " + phase);
      requireApi(typeof MutationObserver === "function", "MutationObserver");
      const observed = new Promise((resolve, reject) => {
        const observer = new MutationObserver((records) => {
          observer.disconnect();
          records.length ? resolve() : reject(new Error("failed: MutationObserver records"));
        });
        observer.observe(root, { childList: true });
        root.appendChild(document.createElement("i"));
        setTimeout(() => { observer.disconnect(); reject(new Error("failed: MutationObserver delivery")); }, 2000);
      });
      await observed;
      requireApi(typeof ResizeObserver === "function", "ResizeObserver");
      await new Promise((resolve, reject) => {
        const observer = new ResizeObserver((entries) => {
          observer.disconnect();
          entries.length ? resolve() : reject(new Error("failed: ResizeObserver entries"));
        });
        observer.observe(root);
        requestAnimationFrame(() => requestAnimationFrame(() => {
          observer.disconnect();
          reject(new Error("failed: ResizeObserver delivery"));
        }));
      });

      phase = "storage and history";
      report("Checking: " + phase);
      requireApi(typeof localStorage === "object", "localStorage");
      localStorage.setItem("w38", "stored");
      assert(localStorage.getItem("w38") === "stored", "localStorage round trip");
      localStorage.removeItem("w38");
      requireApi(typeof history.pushState === "function", "history.pushState");
      const state = { nested: { value: 3 } };
      history.pushState(state, "", "#w38-readiness");
      state.nested.value = 9;
      assert(history.state.nested.value === 3 && location.hash === "#w38-readiness", "history state clone and URL");
      phase = "crypto";
      report("Checking: " + phase);
      requireApi(!!globalThis.crypto && typeof crypto.getRandomValues === "function", "crypto.getRandomValues");
      const random = crypto.getRandomValues(new Uint8Array(16));
      requireApi(typeof random.some === "function", "TypedArray.prototype.some");
      assert(random.some((value) => value !== 0), "crypto random bytes");
      phase = "clock and animation frames";
      report("Checking: " + phase);
      requireApi(typeof performance !== "undefined" && typeof performance.now === "function", "performance.now");
      const before = performance.now();
      assert(performance.now() >= before, "monotonic performance clock");
      requireApi(typeof requestAnimationFrame === "function", "requestAnimationFrame");
      const frame = await new Promise((resolve) => requestAnimationFrame(resolve));
      assert(Number.isFinite(frame), "animation frame timestamp");

      phase = "style";
      report("Checking: " + phase);
      requireApi(typeof getComputedStyle === "function", "getComputedStyle");
      const styled = document.getElementById("fixture-root");
      styled.style.color = "rgb(1, 2, 3)";
      assert(getComputedStyle(styled).color === "rgb(1, 2, 3)", "live style declaration and computed style");
      phase = "template";
      report("Checking: " + phase);
      const template = document.getElementById("fixture-template");
      requireApi(!!template.content, "HTMLTemplateElement.content");
      assert(template.content.firstElementChild?.textContent === "inert", "inert template content");
      phase = "parser";
      report("Checking: " + phase);
      requireApi(typeof DOMParser === "function", "DOMParser");
      const parsed = new DOMParser().parseFromString("<main><b>parsed</b></main>", "text/html");
      assert(parsed.querySelector("main b")?.textContent === "parsed", "DOMParser document");
      phase = "form data";
      report("Checking: " + phase);
      requireApi(typeof FormData === "function", "FormData");
      const form = new FormData(document.getElementById("fixture-form"));
      assert(form.get("fixture") === "w38", "FormData successful controls");
      report("READY: W38 GUEST PASS");
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      report("READY: W38 GUEST FAIL\n" + phase + ": " + message);
    } finally {
      run.disabled = false;
    }
  });
})();
