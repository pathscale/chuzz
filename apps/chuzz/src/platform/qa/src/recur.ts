"use strict";

(function () {
  var result = document.getElementById("result");
  var run = document.getElementById("run");

  function fail(reason) {
    result.textContent = "FAIL: RECUR " + reason;
  }

  function plain(limit) {
    function down(n) {
      if (n >= limit) return n;
      return down(n + 1);
    }
    return down(0);
  }

  function getters(limit) {
    var seen = 0;
    var obj = {};
    Object.defineProperty(obj, "v", {
      configurable: true,
      get: function () {
        seen += 1;
        if (seen >= limit) return seen;
        return obj.v;
      }
    });
    return obj.v;
  }

  function dispatches(limit) {
    var seen = 0;
    var el = document.createElement("div");
    el.addEventListener("recur", function () {
      seen += 1;
      if (seen < limit) el.dispatchEvent(new Event("recur"));
    });
    el.dispatchEvent(new Event("recur"));
    return seen;
  }

  function overflow() {
    try {
      function down() { return down(); }
      down();
      return "no throw";
    } catch (error) {
      if (error instanceof RangeError && error.message === "Maximum call stack size exceeded") {
        return "caught";
      }
      var name = error && error.name ? String(error.name) : "unknown";
      var message = error && error.message ? String(error.message) : String(error);
      return name + ": " + message;
    }
  }

  function promiseChain(limit) {
    var chain = Promise.resolve(0);
    var i;
    for (i = 0; i < limit; i += 1) {
      chain = chain.then(function (value) { return value + 1; });
    }
    return chain;
  }

  run.addEventListener("click", function () {
    try {
      var depth = plain(9000);
      if (depth !== 9000) return fail("plain " + depth);
      var got = getters(1024);
      if (got !== 1024) return fail("getter " + got);
      var sent = dispatches(500);
      if (sent !== 500) return fail("dispatch " + sent);
      var caught = overflow();
      if (caught !== "caught") return fail("infinite " + caught);
      promiseChain(10000).then(function (value) {
        if (value === 10000) result.textContent = "READY: RECUR PASS";
        else fail("promise " + value);
      }, function (error) {
        fail("promise " + (error && error.message ? error.message : error));
      });
    } catch (error) {
      fail((error && error.name ? error.name : "throw") + " " + (error && error.message ? error.message : error));
    }
  });
})();
