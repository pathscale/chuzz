(function () {
    "use strict";

    function assert(condition, message) {
        if (!condition) throw new Error(message);
    }

    function fontStyle() {
        return '<style>' +
            '@font-face { font-family: "Fira Sans"; ' +
            'src: url("./fonts/FiraSans-Regular.woff2") format("woff2"); }' +
            'body { font-family: "Fira Sans"; margin: 20px; color: #102030; ' +
            'background: #fff; } #result { font-size: 20px; }' +
            '</style>';
    }

    document.getElementById("run").addEventListener("click", async function () {
        let result = document.getElementById("result");
        try {
            const proof = globalThis.__docwriteProof;
            assert(proof.futureHidden, "parser stops before later source markup");
            assert(proof.parentRestored && proof.writtenCurrent, "nested currentScript restoration");
            assert(proof.external, "written external script blocks remaining source");
            assert(proof.position, "written markup uses the parser insertion point");
            assert(
                proof.order.join("|") === "writer|inline|external|writer-end|later",
                "written script execution order"
            );
            assert(document.compatMode === "CSS1Compat", "doctype standards mode");
            assert(document.contentType === "text/html", "HTML content type");
            assert(typeof document.referrer === "string", "navigation referrer string");
            assert(typeof document.hasFocus() === "boolean", "host focus state");

            const fonts = document.fonts;
            assert(fonts instanceof FontFaceSet, "FontFaceSet interface");
            assert(fonts === document.fonts, "stable document.fonts identity");
            assert(await fonts.ready === fonts, "ready resolves to the set");
            assert(fonts.status === "loaded", "CSS fonts settle");
            assert(fonts.check('16px "Fira Sans"'), "bundled CSS face is loaded");
            const cssFaces = await fonts.load('16px "Fira Sans"');
            assert(cssFaces.length > 0 && cssFaces.every(face => face instanceof FontFace),
                "load returns real CSS FontFace objects");

            let syntax = false;
            try { fonts.check("not a font shorthand"); }
            catch (failure) { syntax = failure.name === "SyntaxError"; }
            assert(syntax, "check rejects invalid font syntax");

            const face = new FontFace(
                "DOCWRITE URL",
                'url("./fonts/FiraSans-Regular.woff2")',
                { style: "italic", weight: "700" }
            );
            assert(face.family === "DOCWRITE URL" && face.style === "italic" &&
                face.weight === "700" && face.status === "unloaded",
                "FontFace descriptors and initial status");

            const previousReady = fonts.ready;
            let loading = 0;
            let done = 0;
            const onLoading = function (event) {
                assert(event instanceof FontFaceSetLoadEvent && event.target === fonts &&
                    event.isTrusted, "loading event interface and target");
                loading++;
            };
            const onDone = function (event) {
                assert(event instanceof FontFaceSetLoadEvent &&
                    event.fontfaces.includes(face), "loadingdone includes the loaded face");
                done++;
            };
            fonts.addEventListener("loading", onLoading);
            fonts.addEventListener("loadingdone", onDone);
            assert(fonts.add(face) === fonts && fonts.has(face), "add and has");
            const loaded = face.load();
            assert(loaded === face.loaded, "stable loaded promise");
            assert(fonts.ready !== previousReady, "new loading cycle has a new ready promise");
            assert(await loaded === face && face.status === "loaded", "URL face loads");
            await fonts.ready;
            fonts.removeEventListener("loading", onLoading);
            fonts.removeEventListener("loadingdone", onDone);
            assert(loading === 1 && done === 1, "one loading cycle");
            assert(fonts.check('italic 700 16px "DOCWRITE URL"'), "descriptor matching");
            assert(Array.from(fonts).includes(face), "FontFaceSet iteration");
            assert(fonts.delete(face) && !fonts.has(face) && !fonts.delete(face),
                "delete updates membership");

            const response = await fetch("./fonts/FiraSans-Regular.woff2");
            assert(response.ok, "bundled font fetch");
            const buffer = await response.arrayBuffer();
            const length = buffer.byteLength;
            const binary = new FontFace("DOCWRITE Buffer", buffer);
            assert(buffer.byteLength === length, "constructor preserves its ArrayBuffer");
            assert(await binary.load() === binary && binary.status === "loaded",
                "ArrayBuffer face loads");
            fonts.add(binary);
            assert(fonts.check('16px "DOCWRITE Buffer"'), "binary face is registered");
            fonts.delete(binary);

            const originalRoot = document.documentElement;
            assert(document.open() === document, "open returns the document");
            assert(document.documentElement === null && document.body === null,
                "open clears the old tree");
            document.write('<!doctype html><html><head>' + fontStyle() +
                '</head><body><h1 id="new');
            document.write('">New stream</h1><p id="result" role="status">Checking stream</p>');
            assert(document.getElementById("new").textContent === "New stream",
                "split writes update the new document synchronously");
            document.writeln(
                '<script>globalThis.__docwriteClosedScript = ' +
                'document.currentScript !== null;<\/script>'
            );
            document.write("</body></html>");
            document.close();
            assert(globalThis.__docwriteClosedScript, "new stream executes its scripts");
            assert(document.documentElement !== originalRoot, "new root identity");
            assert(document.compatMode === "CSS1Compat", "new stream doctype");
            await document.fonts.ready;

            document.open();
            document.write('<html><head>' + fontStyle() + '</head><body>' +
                '<h1>DOCWRITE platform proof</h1>' +
                '<p id="result" role="status">Checking quirks</p></body></html>');
            document.close();
            assert(document.compatMode === "BackCompat", "parser reports missing-doctype quirks");
            await document.fonts.ready;
            result = document.getElementById("result");
            result.textContent = "READY: DOCWRITE PASS";
        } catch (failure) {
            result = document.getElementById("result") || result;
            if (!result.isConnected && document.body) document.body.appendChild(result);
            result.textContent = "FAIL: " + failure.message;
        }
    });
})();
