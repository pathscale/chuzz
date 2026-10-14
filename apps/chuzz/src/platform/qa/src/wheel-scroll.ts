const EXPECTED_WHEEL_TICKS = 10;
const result = document.getElementById("result");
const outer = document.getElementById("outer");
const inner = document.getElementById("inner");
const sequence = [];
const syntheticModesAndDeltas = [];
let syntheticInput = true;
let actualWheelCount = 0;
let outcomeScheduled = false;
let failed = false;

function fail(reason) {
  if (failed) return;
  failed = true;
  result.textContent = "FAIL: " + reason;
}

inner.addEventListener("wheel", (event) => {
  sequence.push("passive:" + event.deltaMode);
  if (syntheticInput && !event.cancelable) fail("line-page-wheel-not-cancelable");
  event.preventDefault();
  if (event.defaultPrevented) {
    fail(syntheticInput ? "passive-line-page-wheel-cancelled" : "passive-pixel-wheel-cancelled");
  }
}, { passive: true });

outer.addEventListener("wheel", (event) => {
  sequence.push("active:" + event.deltaMode);
  if (syntheticInput) {
    syntheticModesAndDeltas.push(event.deltaMode + ":" + event.deltaY);
    event.preventDefault();
    if (!event.defaultPrevented) fail("non-passive-line-page-wheel-not-cancelled");
    return;
  }

  if (event.deltaMode !== 0) {
    fail("real-wheel-input-was-not-pixel-mode");
    return;
  }
  if (event.defaultPrevented) fail("pixel-wheel-cancelled-before-active-listener");
  actualWheelCount += 1;
  if (actualWheelCount > EXPECTED_WHEEL_TICKS) {
    fail("unexpected-real-wheel-count");
    return;
  }
  if (actualWheelCount === EXPECTED_WHEEL_TICKS) scheduleOutcome();
}, { passive: false });

function scheduleOutcome() {
  if (outcomeScheduled || failed) return;
  outcomeScheduled = true;
  setTimeout(() => {
    outcomeScheduled = false;
    if (failed || actualWheelCount !== EXPECTED_WHEEL_TICKS) return;

    const expectedSequence = ["passive:1", "active:1", "passive:2", "active:2"];
    for (let tick = 0; tick < EXPECTED_WHEEL_TICKS; tick++) {
      expectedSequence.push("passive:0", "active:0");
    }
    if (sequence.join("|") !== expectedSequence.join("|")) {
      fail("wheel-listener-event-sequence");
      return;
    }
    if (syntheticModesAndDeltas.join("|") !== "1:-3|2:-1") {
      fail("line-page-delta-mode-sequence");
      return;
    }
    const innerMaximum = inner.scrollHeight - inner.clientHeight;
    if (Math.abs(inner.scrollTop - innerMaximum) > 1) {
      fail("inner-scroll-offset-" + Math.round(inner.scrollTop));
      return;
    }
    if (Math.abs(outer.scrollTop - 120) > 1) {
      fail("outer-scroll-offset-" + Math.round(outer.scrollTop));
      return;
    }
    result.textContent = "READY: WHEEL-SCROLL PASS";
  }, 0);
}

try {
  const expectedSyntheticSequence = [];
  for (const [deltaMode, deltaY] of [[1, -3], [2, -1]]) {
    const event = new WheelEvent("wheel", {
      bubbles: true,
      cancelable: true,
      deltaY,
      deltaMode
    });
    const wasNotCanceled = inner.dispatchEvent(event);
    expectedSyntheticSequence.push("passive:" + deltaMode, "active:" + deltaMode);
    if (wasNotCanceled || !event.defaultPrevented) {
      fail("non-passive-line-page-wheel-did-not-cancel");
    }
  }
  syntheticInput = false;
  if (!failed && sequence.join("|") !== expectedSyntheticSequence.join("|")) {
    fail("line-page-listener-event-sequence");
  }
  if (!failed && syntheticModesAndDeltas.join("|") !== "1:-3|2:-1") {
    fail("line-page-delta-mode-sequence");
  }
  if (!failed && (inner.scrollTop !== 0 || outer.scrollTop !== 0)) {
    fail("synthetic-wheel-ran-scroll-default");
  }
  if (!failed) result.textContent = "READY: WHEEL-SCROLL WAITING";
} catch (error) {
  syntheticInput = false;
  fail("synthetic-wheel-setup-" + String(error));
}

setTimeout(() => {
  if (!failed && actualWheelCount < EXPECTED_WHEEL_TICKS) {
    fail("real-pixel-wheel-input-missing");
  }
}, 8000);
