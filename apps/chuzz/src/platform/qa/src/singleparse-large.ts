// Runs at the tail of singleparse-large.html, after the parser has built every source element.
(function () {
    const timing = globalThis.singleparseLarge;
    timing.parseMs = performance.now() - timing.headStart;
    timing.startupMs = Date.now() - timing.navigationStart;
    timing.count = document.getElementById("large-rows").children.length;
    console.log("SINGLEPARSE 20000 source elements: head-to-tail parse " +
        timing.parseMs.toFixed(1) + " ms; navigation/startup " +
        timing.startupMs + " ms");
    document.getElementById("timing").textContent =
        "20,000 source elements: head-to-tail parse " +
        timing.parseMs.toFixed(1) + " ms; navigation/startup " +
        timing.startupMs + " ms";
    document.addEventListener("DOMContentLoaded", async function () {
        const result = document.getElementById("result");
        try {
            await document.fonts.ready;
            if (timing.count !== 20000) {
                throw new Error("Incorrect source element count: " + timing.count);
            }
            result.textContent = "READY: SINGLEPARSE LARGE PASS";
        } catch (error) {
            result.textContent = "FAIL: SINGLEPARSE LARGE " + error.message;
            console.error(error);
        }
    });
})();
