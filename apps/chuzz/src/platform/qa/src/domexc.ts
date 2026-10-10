(function () {
    "use strict";

    function assert(value, message) {
        if (!value) throw new Error(message);
    }

    function exception(action, name, code) {
        try { action(); }
        catch (error) {
            assert(error instanceof DOMException, name + " is a DOMException");
            assert(error instanceof Error, name + " inherits Error");
            assert(error.name === name && error.code === code, name + " fields");
            return error;
        }
        throw new Error("Missing " + name);
    }

    function delay(milliseconds) {
        return new Promise(resolve => setTimeout(resolve, milliseconds));
    }

    function deadline(promise, label) {
        let timer;
        return Promise.race([
            promise,
            new Promise((resolve, reject) => {
                timer = setTimeout(() => reject(new Error(label + " timed out")), 5000);
            })
        ]).finally(() => clearTimeout(timer));
    }

    async function run() {
        const defaults = new DOMException();
        assert(defaults.name === "Error" && defaults.message === "" && defaults.code === 0,
            "DOMException defaults");
        const aborted = new DOMException("cancelled", "AbortError");
        assert(aborted instanceof DOMException && aborted instanceof Error, "exception prototype chain");
        assert(Object.getPrototypeOf(DOMException.prototype) === Error.prototype,
            "DOMException prototype inherits Error.prototype");
        assert(Object.prototype.toString.call(aborted) === "[object DOMException]", "exception tag");
        assert(aborted.toString() === "AbortError: cancelled", "Error toString");
        assert(aborted.code === 20 && DOMException.ABORT_ERR === 20 &&
            aborted.ABORT_ERR === 20, "legacy constants");
        assert(DOMException.DOMSTRING_SIZE_ERR === 2 && DOMException.VALIDATION_ERR === 16 &&
            DOMException.DATA_CLONE_ERR === 25, "obsolete and current constants");
        assert(new DOMException("x", "UnknownError").code === 0, "unknown name code");
        class DerivedException extends DOMException {}
        assert(new DerivedException("x", "SyntaxError") instanceof DerivedException,
            "DOMException subclass");

        exception(() => document.querySelector("["), "SyntaxError", 12);
        exception(() => document.createDocumentFragment().querySelector("["), "SyntaxError", 12);
        const parent = document.createElement("div");
        const child = document.createElement("span");
        exception(() => parent.removeChild(child), "NotFoundError", 8);
        parent.appendChild(child);
        exception(() => child.appendChild(parent), "HierarchyRequestError", 3);
        exception(() => parent.classList.add(""), "SyntaxError", 12);
        exception(() => parent.classList.add("two words"), "InvalidCharacterError", 5);
        exception(() => parent.attributes.removeNamedItem("absent"), "NotFoundError", 8);
        exception(() => document.createEvent("unsupported"), "NotSupportedError", 9);
        exception(() => structuredClone(function () {}), "DataCloneError", 25);

        const controller = new AbortController();
        controller.abort();
        assert(controller.signal.reason instanceof DOMException &&
            controller.signal.reason.name === "AbortError", "abort reason branding");
        const xhr = new XMLHttpRequest();
        exception(() => xhr.send(), "InvalidStateError", 11);

        const channel = new MessageChannel();
        try {
            assert(channel.port1 instanceof MessagePort && channel.port1 instanceof EventTarget,
                "port interfaces");
            const message = new MessageEvent("message", {
                data: { answer: 42 }, origin: "https://example.test",
                lastEventId: "event-7", source: channel.port1, ports: [channel.port2]
            });
            assert(message instanceof Event && message instanceof MessageEvent, "MessageEvent chain");
            assert(message.data.answer === 42 && message.origin === "https://example.test" &&
                message.lastEventId === "event-7" && message.source === channel.port1 &&
                message.ports[0] === channel.port2, "MessageEvent fields");
            assert(Object.isFrozen(message.ports), "MessageEvent ports are frozen");
            const blank = new MessageEvent("message");
            assert(blank.data === null && blank.source === null && blank.origin === "" &&
                blank.lastEventId === "" && blank.ports.length === 0, "MessageEvent defaults");

            let delivered = 0;
            let received;
            const queued = new Promise(resolve => {
                channel.port1.addEventListener("message", event => {
                    delivered++;
                    received = event;
                    resolve();
                }, { once: true });
            });
            const payload = { values: [1, 2] };
            channel.port2.postMessage(payload);
            payload.values[0] = 99;
            await delay(40);
            assert(delivered === 0, "message waits for start");
            channel.port1.start();
            await deadline(queued, "port message");
            assert(received instanceof MessageEvent && received.target === channel.port1 &&
                received.data.values[0] === 1, "port dispatch clones data and sets target");

            const handler = deadline(new Promise(resolve => {
                channel.port1.onmessage = event => resolve(event.data);
            }), "port onmessage");
            channel.port2.postMessage("second");
            assert(await handler === "second", "onmessage delivery");
            channel.port1.onmessage = null;
            channel.port1.close();
            channel.port2.postMessage("discarded");
            await delay(40);
            assert(delivered === 1, "closed port and once listener");
        } finally {
            channel.port1.close();
            channel.port2.close();
        }

        const posted = deadline(new Promise(resolve => {
            const listener = event => {
                if (!event.data || event.data.domexc !== true) return;
                window.removeEventListener("message", listener);
                resolve(event);
            };
            window.addEventListener("message", listener);
        }), "window message");
        const value = { domexc: true, count: 1 };
        window.postMessage(value, "*");
        value.count = 2;
        const event = await posted;
        assert(event instanceof MessageEvent && event.source === window &&
            event.target === window && event.origin === new URL(location.href).origin &&
            event.data.count === 1, "window MessageEvent and clone");
        exception(() => window.postMessage(null, "http://["), "SyntaxError", 12);

        const data = new DataTransfer();
        const items = data.items;
        const files = data.files;
        data.setData("Text", "first");
        data.setData("text/html", "<b>second</b>");
        data.setData("text/plain", "updated");
        assert(data.getData("TEXT") === "updated" && data.types.join("|") ===
            "text/plain|text/html", "transfer aliases and replacement order");
        assert(items === data.items && items.length === 2 && items[0] instanceof DataTransferItem,
            "live item list");
        exception(() => items.add("duplicate", "text/plain"), "NotSupportedError", 9);
        const string = deadline(new Promise(resolve => items[0].getAsString(resolve)), "string item");
        assert(await string === "updated" && items[0].getAsFile() === null, "string item callbacks");
        const file = new File(["payload"], "sample.txt", { type: "text/plain", lastModified: 7 });
        items.add(file);
        assert(files === data.files && files.length === 1 && files[0] === file &&
            files.item(0) === file && data.types.includes("Files"), "live transfer files");
        data.dropEffect = "copy";
        data.dropEffect = "invalid";
        assert(data.dropEffect === "copy", "dropEffect enum");
        data.clearData();
        assert(items.length === 1 && files.length === 1 && data.types.join("|") === "Files",
            "clearData preserves file items");
        const drag = new DragEvent("drop", { dataTransfer: data });
        const clipboard = new ClipboardEvent("paste", { clipboardData: data });
        assert(drag.dataTransfer === data && clipboard.clipboardData === data,
            "drag and clipboard transfer identity");
        items.clear();
        assert(files.length === 0 && data.types.length === 0, "file list stays live");

        const probe = document.getElementById("probe");
        const text = probe.firstChild;
        const range = new StaticRange({
            startContainer: text, startOffset: 1, endContainer: text, endOffset: 3
        });
        assert(range instanceof StaticRange && range instanceof AbstractRange &&
            range.startContainer === text && range.startOffset === 1 &&
            range.endContainer === text && range.endOffset === 3 && !range.collapsed,
            "StaticRange fields");
        const live = document.createRange();
        live.setStart(text, 1);
        live.setEnd(text, 3);
        assert(live instanceof Range && live instanceof AbstractRange,
            "live and static ranges share AbstractRange");
        text.textContent = "changed";
        assert(range.startOffset === 1 && range.endOffset === 3, "StaticRange does not track edits");
        assert(live.startOffset === 0 && live.endOffset === 0,
            "live Range still tracks character data replacement");
        const input = new InputEvent("beforeinput", { targetRanges: [range] });
        assert(input.getTargetRanges()[0] === range, "InputEvent StaticRange identity");
        exception(() => new StaticRange({
            startContainer: document.createAttribute("x"), startOffset: 0,
            endContainer: text, endOffset: 0
        }), "InvalidNodeTypeError", 24);
        exception(() => live.setStart(text, 1000), "IndexSizeError", 1);
        exception(() => customElements.define("invalid", class extends HTMLElement {}),
            "SyntaxError", 12);
        exception(() => matchMedia("all").dispatchEvent(new Event("")),
            "InvalidStateError", 11);

        const audio = document.createElement("audio");
        const video = document.createElement("video");
        assert(audio.load === HTMLMediaElement.prototype.load &&
            video.load === HTMLMediaElement.prototype.load, "media load inheritance");
        assert(audio.load() === undefined && video.load() === undefined,
            "created media elements support device-free load");

        await navigator.clipboard.writeText("DOMEXC clipboard proof");
        assert(await navigator.clipboard.readText() === "DOMEXC clipboard proof",
            "host clipboard round trip");

        const source = "onmessage = function(event) {" +
            "var copy = event.data;" +
            "postMessage({ message: event instanceof MessageEvent," +
            "error: copy instanceof DOMException && copy instanceof Error," +
            "name: copy.name, code: copy.code, text: copy.message });" +
            "};";
        const worker = new Worker("data:text/javascript," + encodeURIComponent(source));
        try {
            const reply = deadline(new Promise((resolve, reject) => {
                worker.onmessage = resolve;
                worker.onerror = event => reject(new Error(event.message));
            }), "worker message");
            worker.postMessage(new DOMException("from page", "AbortError"));
            const message = await reply;
            assert(message instanceof MessageEvent && message.target === worker &&
                message.data.message && message.data.error &&
                message.data.name === "AbortError" && message.data.code === 20 &&
                message.data.text === "from page", "worker MessageEvent and DOMException clone");
        } finally {
            worker.terminate();
        }
    }

    document.getElementById("run").addEventListener("click", function () {
        const button = this;
        const result = document.getElementById("result");
        button.disabled = true;
        result.textContent = "RUNNING: DOMEXC";
        run().then(function () {
            result.textContent = "READY: DOMEXC PASS";
        }, function (error) {
            result.textContent = "FAIL: DOMEXC " + error;
        }).finally(function () {
            button.disabled = false;
        });
    });
})();

