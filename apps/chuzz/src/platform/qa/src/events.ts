"use strict";

(function () {
  const result = document.getElementById("result");
  const run = document.getElementById("run");
  const sandbox = document.getElementById("sandbox");
  const submitter = document.getElementById("submitter");

  function assert(condition, name) {
    if (!condition) throw new Error(name);
  }

  function throwsTypeError(callback, name) {
    let caught = false;
    try {
      callback();
    } catch (error) {
      caught = error instanceof TypeError;
    }
    assert(caught, name);
  }

  function check(Constructor, init, expected, Parent) {
    const event = new Constructor("probe", Object.assign({
      bubbles: true,
      cancelable: true,
      composed: true
    }, init));
    assert(event instanceof Constructor && event instanceof Parent && event instanceof Event,
      Constructor.name + " inheritance");
    assert(event.constructor === Constructor, Constructor.name + " constructor");
    assert(Object.prototype.toString.call(event) === "[object " + Constructor.name + "]",
      Constructor.name + " tag");
    assert(event.type === "probe" && event.bubbles && event.cancelable && event.composed,
      Constructor.name + " base dictionary");
    assert(event.isTrusted === false && event.target === null && event.eventPhase === 0,
      Constructor.name + " initial state");
    for (const name of Object.keys(expected)) {
      assert(event[name] === expected[name], Constructor.name + "." + name);
      assert(Reflect.set(event, name, "changed") === false, Constructor.name + "." + name + " readonly");
    }
    let seen = null;
    const listener = function (received) {
      seen = received;
      assert(received.target === sandbox && received.currentTarget === sandbox,
        Constructor.name + " dispatch targets");
      assert(received.composedPath()[0] === sandbox, Constructor.name + " dispatch path");
      received.preventDefault();
    };
    sandbox.addEventListener("probe", listener);
    try {
      assert(sandbox.dispatchEvent(event) === false, Constructor.name + " cancellation result");
    } finally {
      sandbox.removeEventListener("probe", listener);
    }
    assert(seen === event && event.defaultPrevented, Constructor.name + " delivered identity");
    assert(event.currentTarget === null && event.eventPhase === 0 && event.composedPath().length === 0,
      Constructor.name + " dispatch cleanup");
    return event;
  }

  async function historyProof() {
    const originalURL = location.href;
    const originalState = history.state;
    const stem = new URL(originalURL);
    stem.hash = "";
    const firstURL = stem.href + "#events-one";
    const secondURL = stem.href + "#events-two";
    const original = { page: 1, nested: { value: 7 } };
    let notifications = 0;
    const count = function () { notifications += 1; };
    addEventListener("popstate", count);
    addEventListener("hashchange", count);
    try {
      history.pushState(original, "", firstURL);
      original.nested.value = 99;
      history.pushState({ page: 2 }, "", secondURL);
      assert(notifications === 0, "pushState emits no navigation events");
      await new Promise(function (resolve, reject) {
        const order = [];
        let timer;
        function clean() {
          clearTimeout(timer);
          removeEventListener("popstate", onPop);
          removeEventListener("hashchange", onHash);
        }
        function onPop(event) {
          try {
            assert(event instanceof PopStateEvent && event.isTrusted, "native popstate interface");
            assert(event.state.page === 1 && event.state.nested.value === 7,
              "popstate cloned entry state");
            assert(history.state === event.state && location.href === firstURL,
              "popstate current history state and URL");
            order.push("popstate");
          } catch (error) {
            clean();
            reject(error);
          }
        }
        function onHash(event) {
          try {
            assert(event instanceof HashChangeEvent && event.isTrusted, "native hashchange interface");
            assert(event.oldURL === secondURL && event.newURL === firstURL,
              "hashchange old and new URL");
            order.push("hashchange");
            assert(order.join(",") === "popstate,hashchange", "history event ordering");
            clean();
            resolve();
          } catch (error) {
            clean();
            reject(error);
          }
        }
        addEventListener("popstate", onPop);
        addEventListener("hashchange", onHash);
        timer = setTimeout(function () {
          clean();
          reject(new Error("history traversal notifications"));
        }, 3000);
        history.back();
        assert(location.href === secondURL && order.length === 0, "history traversal is asynchronous");
      });
      assert(notifications === 2, "one popstate and one hashchange");
      history.go(1000);
      await new Promise(function (resolve) { setTimeout(resolve, 20); });
      assert(location.href === firstURL && notifications === 2, "out of range traversal is ignored");
    } finally {
      removeEventListener("popstate", count);
      removeEventListener("hashchange", count);
      history.replaceState(originalState, "", originalURL);
    }
  }

  run.addEventListener("click", async function (click) {
    run.disabled = true;
    result.textContent = "Running events";
    try {
      assert(click instanceof PointerEvent && click instanceof MouseEvent &&
        click instanceof UIEvent && click instanceof Event, "native click prototype chain");
      assert(click.isTrusted && click.target === run && click.view === window,
        "native click trust target and view");
      assert(click.pointerType === "mouse" && click.pointerId === 1, "native mouse pointer identity");
      assert(!(new Event("plain") instanceof CustomEvent), "CustomEvent has a distinct prototype");
      throwsTypeError(function () { MouseEvent("probe"); }, "constructor requires new");
      throwsTypeError(function () { new WheelEvent("probe", { deltaX: Infinity }); }, "finite wheel delta");

      check(UIEvent, { view: window, detail: 4 }, { view: window, detail: 4 }, Event);
      const mouse = check(MouseEvent, {
        screenX: 11, screenY: 12, clientX: 21, clientY: 22, button: 2, buttons: 2,
        ctrlKey: true, shiftKey: true, altKey: true, metaKey: true,
        relatedTarget: submitter, movementX: 3, movementY: -4, modifierCapsLock: true
      }, {
        screenX: 11, screenY: 12, clientX: 21, clientY: 22, x: 21, y: 22,
        button: 2, buttons: 2, relatedTarget: submitter, movementX: 3, movementY: -4
      }, UIEvent);
      assert(mouse.getModifierState("Control") && mouse.getModifierState("Shift") &&
        mouse.getModifierState("Alt") && mouse.getModifierState("Meta") &&
        mouse.getModifierState("CapsLock") && !mouse.getModifierState("unknown"),
        "mouse modifier states");
      assert(mouse.pageX === mouse.clientX + window.scrollX ||
        mouse.pageX === mouse.clientX, "mouse page coordinate");
      throwsTypeError(function () {
        Object.getOwnPropertyDescriptor(MouseEvent.prototype, "clientX").get.call(new Event("plain"));
      }, "MouseEvent accessor checks its brand");

      check(PointerEvent, {
        pointerId: 8, width: 9, height: 10, pressure: 0.5, pointerType: "pen", isPrimary: true
      }, {
        pointerId: 8, width: 9, height: 10, pressure: 0.5, pointerType: "pen", isPrimary: true
      }, MouseEvent);
      const keyboard = check(KeyboardEvent, {
        key: "ArrowRight", code: "ArrowRight", location: 2, repeat: true,
        isComposing: true, keyCode: 39, charCode: 0, ctrlKey: true
      }, {
        key: "ArrowRight", code: "ArrowRight", location: 2, repeat: true,
        isComposing: true, keyCode: 39, charCode: 0, which: 39
      }, UIEvent);
      assert(keyboard.getModifierState("Control"), "keyboard modifier state");
      check(FocusEvent, { relatedTarget: submitter }, { relatedTarget: submitter }, UIEvent);
      const input = check(InputEvent, {
        data: "a", inputType: "insertText", isComposing: true
      }, { data: "a", inputType: "insertText", isComposing: true }, UIEvent);
      assert(input.getTargetRanges().length === 0, "input empty target ranges");
      check(WheelEvent, { deltaX: 1, deltaY: -2, deltaZ: 3, deltaMode: 1 },
        { deltaX: 1, deltaY: -2, deltaZ: 3, deltaMode: 1 }, MouseEvent);
      assert(WheelEvent.DOM_DELTA_LINE === 1 && KeyboardEvent.DOM_KEY_LOCATION_RIGHT === 2,
        "interface constants");

      const touch = new Touch({
        identifier: 5, target: sandbox, clientX: 10, clientY: 20,
        pageX: 10, pageY: 20, screenX: 30, screenY: 40, force: 0.5,
        radiusX: 2, radiusY: 3, rotationAngle: 45
      });
      assert(touch.identifier === 5 && touch.target === sandbox && touch.force === 0.5 &&
        touch.radiusX === 2 && touch.rotationAngle === 45, "Touch dictionary");
      throwsTypeError(function () { new Touch({ identifier: 1 }); }, "Touch target is required");
      const touches = [touch];
      const touchEvent = check(TouchEvent, {
        touches: touches, targetTouches: touches, changedTouches: touches
      }, {}, UIEvent);
      touches.length = 0;
      assert(touchEvent.touches instanceof TouchList && touchEvent.touches.length === 1 &&
        touchEvent.touches[0] === touch && touchEvent.touches.item(0) === touch &&
        touchEvent.touches.item(1) === null && Array.from(touchEvent.touches)[0] === touch &&
        touchEvent.targetTouches[0] === touch && touchEvent.changedTouches[0] === touch,
        "immutable TouchList snapshots and iteration");

      check(ProgressEvent, { lengthComputable: true, loaded: 5, total: 10 },
        { lengthComputable: true, loaded: 5, total: 10 }, Event);
      const state = { value: 3 };
      check(PopStateEvent, { state: state }, { state: state }, Event);
      check(HashChangeEvent, { oldURL: "https://example.test/#a", newURL: "https://example.test/#b" },
        { oldURL: "https://example.test/#a", newURL: "https://example.test/#b" }, Event);
      check(StorageEvent, { key: "k", oldValue: null, newValue: "v", url: "https://example.test/" },
        { key: "k", oldValue: null, newValue: "v", url: "https://example.test/", storageArea: null }, Event);
      check(SubmitEvent, { submitter: submitter }, { submitter: submitter }, Event);
      check(AnimationEvent, { animationName: "grow", elapsedTime: 0.5, pseudoElement: "::before" },
        { animationName: "grow", elapsedTime: 0.5, pseudoElement: "::before" }, Event);
      check(TransitionEvent, { propertyName: "opacity", elapsedTime: 0.5, pseudoElement: "::after" },
        { propertyName: "opacity", elapsedTime: 0.5, pseudoElement: "::after" }, Event);
      check(PageTransitionEvent, { persisted: true }, { persisted: true }, Event);
      check(CompositionEvent, { data: "text" }, { data: "text" }, UIEvent);
      check(DragEvent, {}, { dataTransfer: null }, MouseEvent);
      check(ClipboardEvent, {}, { clipboardData: null }, Event);
      check(CustomEvent, { detail: state }, { detail: state }, Event);

      const uncancelable = new Event("uncancelable");
      uncancelable.preventDefault();
      assert(!uncancelable.defaultPrevented, "uncancelable preventDefault");
      const legacy = document.createEvent("mOuSeEvEnTs");
      let uninitialized = false;
      try { sandbox.dispatchEvent(legacy); } catch (error) {
        uninitialized = error.name === "InvalidStateError";
      }
      assert(uninitialized, "legacy event requires initialization");
      legacy.initMouseEvent("legacy", true, true, window, 7, 1, 2, 3, 4,
        true, false, true, false, 2, submitter);
      assert(legacy instanceof MouseEvent && legacy.view === window && legacy.detail === 7 &&
        legacy.clientX === 3 && legacy.clientY === 4 && legacy.button === 2 &&
        legacy.relatedTarget === submitter && legacy.getModifierState("Control"),
        "legacy initMouseEvent");
      const custom = document.createEvent("CustomEvent");
      custom.initCustomEvent("legacy-custom", false, true, state);
      assert(custom.detail === state && custom.cancelable, "legacy initCustomEvent");
      const ui = document.createEvent("UIEvents");
      ui.initUIEvent("legacy-ui", true, false, window, 8);
      assert(ui.view === window && ui.detail === 8, "legacy initUIEvent");
      const key = document.createEvent("KeyboardEvent");
      key.initKeyboardEvent("keydown", true, true, window, "k", 1, true, false, false, true);
      assert(key.key === "k" && key.location === 1 && key.ctrlKey && key.metaKey,
        "legacy initKeyboardEvent");
      const composition = document.createEvent("CompositionEvent");
      composition.initCompositionEvent("compositionend", true, false, window, "done");
      assert(composition.data === "done", "legacy initCompositionEvent");
      const storage = document.createEvent("StorageEvent");
      storage.initStorageEvent("storage", false, false, "key", "old", "new", location.href, null);
      assert(storage.key === "key" && storage.oldValue === "old" && storage.newValue === "new",
        "legacy initStorageEvent");

      let recursiveRejected = false;
      const nested = new Event("nested");
      const nestedListener = function (event) {
        try { sandbox.dispatchEvent(event); } catch (error) {
          recursiveRejected = error.name === "InvalidStateError";
        }
      };
      sandbox.addEventListener("nested", nestedListener);
      sandbox.dispatchEvent(nested);
      sandbox.removeEventListener("nested", nestedListener);
      assert(recursiveRejected && nested.eventPhase === 0, "recursive dispatch rejection and cleanup");

      await historyProof();
      result.textContent = "READY: events PASS";
    } catch (error) {
      result.textContent = "FAIL: " + error.message;
    } finally {
      run.disabled = false;
    }
  });
})();
