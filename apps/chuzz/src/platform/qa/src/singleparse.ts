// JavaScript syntax only. Boa evaluates this source directly.
(function () {
    "use strict";

    const proof = globalThis.__singleparse;
    proof.order.push("defer");
    proof.deferredState = document.readyState;
    proof.deferredCurrent = document.currentScript === document.getElementById("deferred");

    function assert(condition, message) {
        if (!condition) throw new Error(message);
    }

    function task() {
        return new Promise(resolve => setTimeout(resolve, 0));
    }

    function deadline(promise, label) {
        let timer;
        return Promise.race([
            promise,
            new Promise((resolve, reject) => {
                timer = setTimeout(() => reject(new Error(label + " timed out")), 8000);
            })
        ]).finally(() => clearTimeout(timer));
    }

    async function waitForLoad() {
        while (proof.windowLoads === 0) {
            await new Promise(resolve => setTimeout(resolve, 10));
        }
    }

    document.getElementById("run").addEventListener("click", async function () {
        this.disabled = true;
        const result = document.getElementById("result");
        try {
            result.textContent = "RUNNING: SINGLEPARSE";
            await deadline(document.fonts.ready, "bundled font");
            await deadline(proof.markup.decode(), "markup image");
            await deadline(waitForLoad(), "window load");
            await task();
            await task();
            proof.record(proof.observer.takeRecords());

            assert(proof.bodyHidden, "early head script cannot see body markup");
            assert(proof.sameHead && proof.root.isConnected && proof.head.isConnected &&
                proof.anchor.isConnected, "head script retains its original DOM objects");
            assert(proof.sameChildren && document.body === proof.body,
                "body children retain identity across DOMContentLoaded");
            assert(proof.beforeChildren.length === document.body.children.length &&
                proof.beforeChildren.every((node, index) =>
                    node === document.body.children[index]),
                "body children retain identity after load");
            assert(proof.removals.length === 0, "startup removed source elements");

            const elements = [document.body].concat(
                Array.from(document.body.querySelectorAll("*")));
            for (const node of elements) {
                assert(proof.insertions.get(node) === 1,
                    "element inserted exactly once: " + (node.id || node.tagName));
                if (node.id) {
                    assert(proof.ids.get(node.id) === 1,
                        "source ID inserted exactly once: " + node.id);
                }
            }

            assert(proof.imageLoads === 1 && proof.imageErrors === 0 &&
                proof.imageTarget === proof.markup &&
                proof.markup === document.getElementById("markup"),
                "markup image onload fires once on the retained object");
            assert(proof.markup.naturalWidth === 2 && proof.markup.naturalHeight === 2,
                "markup image decoded");
            assert(proof.blockingCurrent && proof.futureHidden &&
                proof.blockingState === "loading" && proof.tailState === "loading",
                "blocking external script runs at its parser boundary");
            assert(proof.deferredCurrent && proof.deferredState === "interactive" &&
                proof.moduleCurrent && proof.moduleState === "interactive",
                "defer and module scripts run before DOMContentLoaded");
            assert(proof.dcl === 1 && proof.windowLoads === 1 &&
                proof.dclState === "interactive" && proof.loadState === "complete",
                "startup events fire once at the correct readyState");
            assert(proof.states.join("|") === "loading|interactive|complete",
                "readyState transitions");
            assert(proof.order.join("|") ===
                "blocking|defer|module|DOMContentLoaded|load", "startup ordering");

            if (proof.host) {
                assert(proof.host.body === null &&
                    proof.host.root === proof.root &&
                    proof.host.head === proof.head &&
                    proof.host.anchor === proof.anchor,
                    "embedder prelude sees the retained first-boundary DOM");
            }

            proof.observer.disconnect();
            result.textContent = "READY: SINGLEPARSE PASS";
        } catch (error) {
            result.textContent = "FAIL: SINGLEPARSE " + error.message;
            console.error("SINGLEPARSE failure: " + (error && error.stack));
        }
    });

    document.getElementById("large").addEventListener("click", function () {
        this.disabled = true;
        const font = new URL("./fonts/FiraSans-Regular.woff2", location.href).href;
        const rows = [];
        for (let index = 0; index < 20000; index++) {
            rows.push('<span data-index="' + index + '">Source element</span>');
        }

        // This function is serialized into the generated navigation. It uses
        // only that document's globals, without retaining this page's closure.
        function finishLargePage() {
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
        }

        const source = '<!doctype html><html lang="en"><head><meta charset="UTF-8">' +
            '<title>SINGLEPARSE large source page</title>' +
            '<style>@font-face{font-family:SingleParse;src:url(' +
            JSON.stringify(font) + ') format("woff2");}' +
            'body{font-family:SingleParse;margin:20px;color:#102030;background:#fff;}' +
            '#result{font-size:20px;}#large-rows{display:none;}</style>' +
            '<script>globalThis.singleparseLarge={headStart:performance.now(),' +
            'navigationStart:__SINGLEPARSE_START__};</script></head><body>' +
            '<h1>SINGLEPARSE large source page</h1>' +
            '<p id="result" role="status">RUNNING: SINGLEPARSE LARGE</p>' +
            '<p id="timing" role="status">Parsing 20,000 source elements</p>' +
            '<div id="large-rows">' + rows.join("") + '</div>' +
            '<script>(' + finishLargePage.toString() + ')();</script></body></html>';

        // The complete generated markup goes through the navigation loader.
        // document.write or innerHTML would miss the discarded initial parse.
        const stamped = source.replace("__SINGLEPARSE_START__", String(Date.now()));
        location.href = "data:text/html;charset=utf-8," + encodeURIComponent(stamped);
    });
})();
