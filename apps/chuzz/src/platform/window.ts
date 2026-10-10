(function (g) {
  "use strict";
  const platform = g.__chuzzPlatform;
  const info = platform.info;
  const define = (name, get) => Object.defineProperty(g, name, {
    configurable: true, enumerable: true, get
  });
  let name = "";
  Object.defineProperty(g, "name", {
    configurable: true, enumerable: true, get: () => name, set: value => { name = String(value); }
  });
  define("closed", () => false);
  define("opener", () => null);
  define("frameElement", () => null);
  define("length", () => g.document.querySelectorAll("iframe,frame").length);
  define("origin", () => new URL(g.location.href).origin);
  define("isSecureContext", () => info.secure);
  define("crossOriginIsolated", () => false);
  g.close = function () {};
  g.alert = value => { if (g.console) g.console.log(value === undefined ? "" : String(value)); };
  g.confirm = value => { g.alert(value); return false; };
  g.prompt = (value, initial) => { g.alert(value); if (initial !== undefined) g.alert(initial); return null; };
  g.reportError = function (error) {
    const event = new g.ErrorEvent("error", {
      message: String(error && error.message !== undefined ? error.message : error),
      error, cancelable: true
    });
    if (g.dispatchEvent(event) && g.console) g.console.error(error);
  };
  g.postMessage = function (value, targetOrigin, transfer) {
    if (targetOrigin && typeof targetOrigin === "object") {
      transfer = targetOrigin.transfer;
      targetOrigin = targetOrigin.targetOrigin;
    }
    if (targetOrigin === undefined) targetOrigin = "/";
    targetOrigin = String(targetOrigin);
    let expected;
    if (targetOrigin === "*") expected = "*";
    else if (targetOrigin === "/") expected = g.origin;
    else {
      try { expected = new URL(targetOrigin).origin; }
      catch (error) { throw platform.exception("SyntaxError", "Invalid target origin"); }
    }
    const data = g.structuredClone(value, { transfer: Array.from(transfer || []) });
    const sourceOrigin = g.origin;
    g.setTimeout(() => {
      if (expected === "*" || expected === g.origin) {
        g.dispatchEvent(platform.message(data, sourceOrigin, g));
      }
    }, 0);
  };
  if (typeof g.__blitzViewportScroll === "function"
      && typeof g.__blitzScrollViewport === "function") {
    define("scrollX", () => g.__blitzViewportScroll()[0]);
    define("scrollY", () => g.__blitzViewportScroll()[1]);
    define("pageXOffset", () => g.scrollX);
    define("pageYOffset", () => g.scrollY);
    function scroll(x, y, relative) {
      if (x && typeof x === "object") {
        y = x.top === undefined ? relative ? 0 : g.scrollY : x.top;
        x = x.left === undefined ? relative ? 0 : g.scrollX : x.left;
      }
      x = Number(x);
      y = Number(y);
      x = Number.isFinite(x) ? x : 0;
      y = Number.isFinite(y) ? y : 0;
      if (relative) { x += g.scrollX; y += g.scrollY; }
      // The page poll notices the moved viewport and fires the scroll event.
      g.__blitzScrollViewport(x, y);
    }
    g.scrollTo = (x, y) => scroll(x, y, false);
    g.scrollBy = (x, y) => scroll(x, y, true);
    g.scroll = g.scrollTo;
  }
  const viewport = new platform.Target();
  const dimensions = () => [g.innerWidth, g.innerHeight, g.scrollX || 0, g.scrollY || 0];
  Object.defineProperties(viewport, {
    width: { enumerable: true, get: () => g.innerWidth },
    height: { enumerable: true, get: () => g.innerHeight },
    scale: { enumerable: true, get: () => 1 },
    offsetLeft: { enumerable: true, get: () => 0 },
    offsetTop: { enumerable: true, get: () => 0 },
    pageLeft: { enumerable: true, get: () => g.scrollX || 0 },
    pageTop: { enumerable: true, get: () => g.scrollY || 0 }
  });
  viewport.onresize = viewport.onscroll = null;
  let previous = dimensions();
  g.setInterval(() => {
    const next = dimensions();
    if (next[0] !== previous[0] || next[1] !== previous[1]) viewport.dispatchEvent(new Event("resize"));
    if (next[2] !== previous[2] || next[3] !== previous[3]) viewport.dispatchEvent(new Event("scroll"));
    previous = next;
  }, 50);
  g.visualViewport = viewport;
  g.Audio = function Audio(source) {
    const element = g.document.createElement("audio");
    element.setAttribute("preload", "auto");
    if (source !== undefined) element.setAttribute("src", String(source));
    return element;
  };
  if (g.HTMLAudioElement) g.Audio.prototype = g.HTMLAudioElement.prototype;
  if (g.performance && typeof g.performance.now === "function"
      && g.performance.timeOrigin === undefined) {
    const now = g.performance.now.bind(g.performance);
    const start = now();
    const timeOrigin = Date.now();
    g.performance.now = () => now() - start;
    Object.defineProperty(g.performance, "timeOrigin", { enumerable: true, value: timeOrigin });
  }
})(globalThis);

