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
        // A real navigation through the loader to a static 20,000-element page; the start time rides in the query.
        location.href = "./singleparse-large.html?t=" + Date.now();
    });
})();
