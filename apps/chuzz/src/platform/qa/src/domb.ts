(function () {
    "use strict";

    const result = document.getElementById("result");
    const sandbox = document.getElementById("sandbox");

    function assert(value, description) {
        if (!value) throw new Error(description);
    }

    function close(actual, expected, description) {
        assert(Math.abs(actual - expected) < 0.00001, description);
    }

    function illegal(constructor) {
        let error;
        try { new constructor(); } catch (caught) { error = caught; }
        assert(error instanceof TypeError && error.message === "Illegal constructor",
            constructor.name + " rejects direct construction");
    }

    async function run() {
        sandbox.textContent = "";

        const doctype = document.doctype;
        assert(doctype && doctype.nodeType === 10
            && doctype.name === "html" && doctype.publicId === "" && doctype.systemId === "",
            "live document doctype fields");
        const root = document.documentElement;
        const documentChildren = Array.from(document.childNodes);
        assert(root && root.localName === "html" && doctype.parentNode === document
            && documentChildren.indexOf(doctype) !== -1
            && documentChildren.indexOf(doctype) < documentChildren.indexOf(root),
            "doctype is a document child before documentElement");

        const tags = {
            a: "HTMLAnchorElement", area: "HTMLAreaElement", audio: "HTMLAudioElement",
            base: "HTMLBaseElement", body: "HTMLBodyElement", br: "HTMLBRElement",
            button: "HTMLButtonElement", canvas: "HTMLCanvasElement",
            caption: "HTMLTableCaptionElement", col: "HTMLTableColElement",
            colgroup: "HTMLTableColElement", data: "HTMLDataElement",
            datalist: "HTMLDataListElement", del: "HTMLModElement",
            details: "HTMLDetailsElement", dialog: "HTMLDialogElement",
            dir: "HTMLDirectoryElement", div: "HTMLDivElement", dl: "HTMLDListElement",
            embed: "HTMLEmbedElement", fieldset: "HTMLFieldSetElement",
            font: "HTMLFontElement", form: "HTMLFormElement", frame: "HTMLFrameElement",
            frameset: "HTMLFrameSetElement", h1: "HTMLHeadingElement",
            h2: "HTMLHeadingElement", h3: "HTMLHeadingElement", h4: "HTMLHeadingElement",
            h5: "HTMLHeadingElement", h6: "HTMLHeadingElement", head: "HTMLHeadElement",
            hr: "HTMLHRElement", html: "HTMLHtmlElement", iframe: "HTMLIFrameElement",
            img: "HTMLImageElement", input: "HTMLInputElement", ins: "HTMLModElement",
            label: "HTMLLabelElement", legend: "HTMLLegendElement", li: "HTMLLIElement",
            link: "HTMLLinkElement", map: "HTMLMapElement", marquee: "HTMLMarqueeElement",
            menu: "HTMLMenuElement", meta: "HTMLMetaElement", meter: "HTMLMeterElement",
            object: "HTMLObjectElement", ol: "HTMLOListElement",
            optgroup: "HTMLOptGroupElement", option: "HTMLOptionElement",
            output: "HTMLOutputElement", p: "HTMLParagraphElement", param: "HTMLParamElement",
            picture: "HTMLPictureElement", pre: "HTMLPreElement", progress: "HTMLProgressElement",
            q: "HTMLQuoteElement", blockquote: "HTMLQuoteElement", script: "HTMLScriptElement",
            select: "HTMLSelectElement", selectedcontent: "HTMLSelectedContentElement",
            slot: "HTMLSlotElement", source: "HTMLSourceElement", span: "HTMLSpanElement",
            style: "HTMLStyleElement", table: "HTMLTableElement", tbody: "HTMLTableSectionElement",
            td: "HTMLTableCellElement", template: "HTMLTemplateElement",
            textarea: "HTMLTextAreaElement", tfoot: "HTMLTableSectionElement",
            th: "HTMLTableCellElement", thead: "HTMLTableSectionElement", time: "HTMLTimeElement",
            title: "HTMLTitleElement", tr: "HTMLTableRowElement", track: "HTMLTrackElement",
            ul: "HTMLUListElement", video: "HTMLVideoElement"
        };
        for (const tag of Object.keys(tags)) {
            const element = document.createElement(tag);
            const name = tags[tag];
            assert(element.constructor === window[name], tag + " constructor identity");
            assert(element instanceof window[name] && element instanceof HTMLElement
                && element instanceof Element && element instanceof Node
                && element instanceof EventTarget, tag + " prototype chain");
            assert(Object.prototype.toString.call(element) === "[object " + name + "]",
                tag + " toStringTag");
            illegal(window[name]);
        }
        assert(document.createElement("article").constructor === HTMLElement, "generic HTML interface");
        assert(document.createElement("domb-unknown").constructor === HTMLElement, "custom-name base interface");
        assert(document.createElement("dombunknown").constructor === HTMLUnknownElement, "unknown HTML interface");
        assert(document instanceof HTMLDocument && document instanceof Document
            && document instanceof Node && document instanceof EventTarget, "document prototype chain");
        assert(!(Object.create(null) instanceof Node), "Node rejects duck typing");

        const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
        const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
        assert(svg.constructor === SVGSVGElement && svg instanceof SVGElement
            && svg instanceof Element && !(svg instanceof HTMLElement), "SVG root chain");
        assert(path.constructor === SVGElement, "SVG descendant interface");
        const text = new Text("text");
        const comment = new Comment("comment");
        const fragment = new DocumentFragment();
        assert(text.data === "text" && text instanceof Text && text instanceof CharacterData,
            "Text construction");
        assert(comment.data === "comment" && comment.nodeType === 8
            && comment instanceof Comment && !(comment instanceof Text), "Comment construction");
        assert(fragment.nodeType === 11 && fragment instanceof DocumentFragment
            && fragment instanceof Node, "DocumentFragment construction");

        class Bus extends EventTarget {}
        const bus = new Bus();
        let calls = 0;
        const callback = function (event) {
            assert(this === bus && event.target === bus && event.currentTarget === bus,
                "EventTarget dispatch targets");
            calls++;
            event.preventDefault();
        };
        bus.addEventListener("domb", callback, { once: true });
        const event = new Event("domb", { cancelable: true });
        assert(bus.dispatchEvent(event) === false && calls === 1, "EventTarget cancellation");
        assert(event.currentTarget === null && event.eventPhase === 0, "EventTarget dispatch cleanup");
        bus.dispatchEvent(new Event("domb"));
        assert(calls === 1, "EventTarget once listener removal");
        const listener = { handleEvent: function () { calls++; } };
        bus.addEventListener("object", listener);
        bus.dispatchEvent(new Event("object"));
        bus.removeEventListener("object", listener);
        bus.dispatchEvent(new Event("object"));
        assert(calls === 2, "EventTarget object listener and removal");

        const parent = document.createElement("div");
        sandbox.appendChild(parent);
        const nodes = parent.childNodes;
        const children = parent.children;
        assert(nodes instanceof NodeList && children instanceof HTMLCollection,
            "named collection interfaces");
        assert(nodes === parent.childNodes && children === parent.children, "live collection identity");
        parent.appendChild(document.createTextNode("prefix"));
        const span = document.createElement("span");
        span.id = "domb-child";
        span.setAttribute("name", "named-child");
        span.className = "chosen";
        parent.appendChild(span);
        assert(nodes.length === 2 && nodes.item(0).nodeType === 3
            && children.length === 1 && children[0] === span, "live collection insertion");
        assert(children.namedItem("domb-child") === span && children["named-child"] === span,
            "HTMLCollection named lookup");
        assert(nodes.item(99) === null && nodes[99] === undefined, "collection out of range");
        const entries = Array.from(nodes.entries());
        assert(entries.length === 2 && entries[1][0] === 1 && entries[1][1] === span,
            "NodeList entries");
        let visited = 0;
        nodes.forEach(function (node, index, list) {
            assert(list === nodes && node === nodes[index], "NodeList forEach arguments");
            visited++;
        });
        assert(visited === 2, "NodeList forEach traversal");

        const staticList = parent.querySelectorAll(".chosen");
        const liveList = parent.getElementsByClassName("chosen");
        const liveTags = parent.getElementsByTagName("span");
        assert(staticList instanceof NodeList && staticList.length === 1, "static NodeList interface");
        const second = document.createElement("span");
        second.className = "chosen";
        parent.appendChild(second);
        assert(staticList.length === 1 && liveList.length === 2 && liveTags.length === 2,
            "static and live query membership");
        parent.removeChild(span);
        assert(nodes.length === 2 && children.length === 1 && liveList.length === 1
            && staticList[0] === span, "collection removal and snapshot retention");
        span.id = "changed";
        assert(children.namedItem("domb-child") === null, "live named lookup after removal");

        const tokens = second.classList;
        assert(tokens instanceof DOMTokenList && tokens === second.classList, "DOMTokenList identity");
        second.setAttribute("class", "one one two");
        assert(tokens.length === 2 && tokens[0] === "one"
            && Array.from(tokens).join(",") === "one,two", "live ordered token set");
        tokens.add("three", "four");
        tokens.remove("two");
        assert(tokens.replace("three", "five") && tokens.contains("five"), "token mutations");
        assert(tokens.toggle("four", false) === false && !tokens.contains("four"), "forced token toggle");
        tokens.value = "alpha beta";
        assert(second.getAttribute("class") === "alpha beta", "DOMTokenList value write through");

        const reflected = document.createElement("div");
        for (const pair of [["title", "title"], ["lang", "lang"], ["accessKey", "accesskey"]]) {
            reflected[pair[0]] = "hello";
            assert(reflected.getAttribute(pair[1]) === "hello", pair[0] + " write through");
            reflected.setAttribute(pair[1], "world");
            assert(reflected[pair[0]] === "world", pair[0] + " live reflection");
        }
        assert(reflected.tabIndex === -1, "default div tabIndex");
        reflected.tabIndex = 7;
        assert(reflected.getAttribute("tabindex") === "7" && reflected.tabIndex === 7, "tabIndex reflection");
        reflected.hidden = true;
        assert(reflected.hasAttribute("hidden") && reflected.hidden === true, "hidden reflection");
        reflected.hidden = false;
        assert(!reflected.hasAttribute("hidden"), "hidden removal");
        reflected.inert = true;
        assert(reflected.hasAttribute("inert"), "inert reflection");
        reflected.dir = "RTL";
        assert(reflected.dir === "rtl", "direction canonicalisation");
        reflected.dir = "invalid";
        assert(reflected.dir === "", "direction invalid default");
        reflected.draggable = true;
        assert(reflected.getAttribute("draggable") === "true" && reflected.draggable, "draggable reflection");
        reflected.contentEditable = "true";
        const editableChild = document.createElement("span");
        reflected.appendChild(editableChild);
        assert(editableChild.contentEditable === "inherit" && editableChild.isContentEditable,
            "inherited editability");
        editableChild.contentEditable = "false";
        assert(!editableChild.isContentEditable, "editability override");
        reflected.translate = false;
        assert(!editableChild.translate && reflected.getAttribute("translate") === "no", "translate inheritance");
        reflected.spellcheck = false;
        assert(!editableChild.spellcheck, "spellcheck inheritance");
        reflected.role = "button";
        reflected.ariaLabel = "Open";
        reflected.ariaRowIndex = "3";
        assert(reflected.getAttribute("role") === "button"
            && reflected.getAttribute("aria-label") === "Open"
            && reflected.getAttribute("aria-rowindex") === "3", "ARIA write through");
        reflected.ariaLabel = null;
        assert(reflected.ariaLabel === null && !reflected.hasAttribute("aria-label"),
            "nullable ARIA reflection");

        const rendered = document.createElement("div");
        rendered.style.position = "relative";
        sandbox.appendChild(rendered);
        rendered.innerText = "first\r\nsecond\n\nlast";
        assert(rendered.querySelectorAll("br").length === 3
            && rendered.textContent === "firstsecondlast", "innerText newline tree replacement");
        assert(rendered.innerText === "first\nsecond\n\nlast", "rendered innerText");
        const detached = document.createElement("div");
        detached.textContent = "detached text";
        assert(detached.innerText === "detached text", "detached innerText fallback");
        const replacement = document.createElement("span");
        rendered.appendChild(replacement);
        replacement.outerText = "outer\ntext";
        assert(replacement.parentNode === null && rendered.querySelectorAll("br").length === 4,
            "outerText tree replacement");

        const box = document.createElement("div");
        box.style.width = "80px";
        box.style.height = "30px";
        rendered.appendChild(box);
        assert(box.offsetParent === rendered, "offsetParent positioned ancestor");
        const rect = box.getBoundingClientRect();
        assert(rect instanceof DOMRect && rect instanceof DOMRectReadOnly, "native bounding DOMRect");
        close(rect.width, 80, "bounding width");
        close(rect.height, 30, "bounding height");
        const rects = box.getClientRects();
        assert(rects.length === 1 && rects.item(0) instanceof DOMRect, "client rectangle list");
        box.style.display = "none";
        assert(box.getClientRects().length === 0 && box.offsetParent === null, "hidden geometry");

        const checkbox = document.createElement("input");
        checkbox.type = "checkbox";
        sandbox.appendChild(checkbox);
        let clicked = 0;
        checkbox.addEventListener("click", function (event) {
            assert(event instanceof Event && event.isTrusted === false, "synthetic native click event");
            assert(checkbox.checked, "click preactivation");
            clicked++;
            event.preventDefault();
        }, { once: true });
        checkbox.click();
        assert(clicked === 1 && checkbox.checked === false, "canceled checkbox activation");
        checkbox.disabled = true;
        checkbox.click();
        assert(clicked === 1, "disabled click suppression");

        const negative = new DOMRect(10, 20, -4, -6);
        assert(negative.left === 6 && negative.right === 10
            && negative.top === 14 && negative.bottom === 20, "negative rectangle edges");
        negative.width = 5;
        assert(negative.right === 15, "mutable rectangle derived edges");
        const readonly = DOMRectReadOnly.fromRect({ x: 1, y: 2, width: 3, height: 4 });
        assert(readonly instanceof DOMRectReadOnly && !(readonly instanceof DOMRect)
            && readonly.toJSON().bottom === 6, "readonly rectangle factory");

        const matrix = new DOMMatrix([1, 0, 0, 1, 4, 5]).translate(2, 3).scale(2, 3);
        const transformed = matrix.transformPoint(new DOMPoint(1, 1));
        close(transformed.x, 8, "2D matrix transformed x");
        close(transformed.y, 11, "2D matrix transformed y");
        const restored = matrix.inverse().transformPoint(transformed);
        close(restored.x, 1, "matrix inverse x");
        close(restored.y, 1, "matrix inverse y");
        const rotated = new DOMMatrix().rotate(90).transformPoint(new DOMPoint(1, 0));
        close(rotated.x, 0, "matrix rotation x");
        close(rotated.y, 1, "matrix rotation y");
        assert(matrix.toString().startsWith("matrix("), "2D matrix serialisation");
        const copied = DOMMatrixReadOnly.fromMatrix(matrix);
        assert(copied instanceof DOMMatrixReadOnly && !(copied instanceof DOMMatrix)
            && copied.e === matrix.e, "matrix dictionary factory");
        const m3 = new DOMMatrix().translate(1, 2, 3).scale(2, 3, 4).rotate(20, 30, 40);
        assert(!m3.is2D && m3.toString().startsWith("matrix3d("), "3D matrix representation");
        const p3 = new DOMPointReadOnly(2, 3, 4, 1);
        const q3 = p3.matrixTransform(m3);
        const r3 = m3.inverse().transformPoint(q3);
        close(r3.x, 2, "3D inverse x");
        close(r3.y, 3, "3D inverse y");
        close(r3.z, 4, "3D inverse z");
        const self = new DOMMatrix();
        assert(self.translateSelf(3, 4) === self && self.e === 3 && self.f === 4,
            "mutable matrix operations");
        const singular = new DOMMatrix([0, 0, 0, 0, 0, 0]).inverse();
        assert(Number.isNaN(singular.a) && singular.is2D === false, "singular inverse");
        assert(DOMPoint.fromPoint({ x: 8 }).w === 1, "point dictionary defaults");

        const link = document.createElement("a");
        link.href = "https://user:pw@example.test:8080/a/b?x=1#frag";
        assert(link.protocol === "https:" && link.host === "example.test:8080"
            && link.hostname === "example.test" && link.port === "8080"
            && link.pathname === "/a/b" && link.search === "?x=1" && link.hash === "#frag"
            && link.username === "user" && link.password === "pw"
            && link.origin === "https://example.test:8080", "anchor URL parts");
        link.pathname = "/c";
        link.search = "y=2";
        link.hash = "";
        assert(link.href === "https://user:pw@example.test:8080/c?y=2", "anchor URL setters");
        const relative = document.createElement("area");
        relative.setAttribute("href", "rel/page");
        assert(relative.pathname.endsWith("/rel/page") && relative.protocol === location.protocol,
            "relative area URL");
        assert(document.createElement("a").pathname === "" && sandbox.pathname === undefined,
            "missing href and non-link elements");

        const meta = document.createElement("meta");
        meta.content = "width=device-width";
        assert(meta.content === "width=device-width"
            && meta.getAttribute("content") === "width=device-width", "meta content reflection");
        assert(document.createElement("template").content instanceof DocumentFragment, "template content");

        const image = new Image();
        image.alt = "Referer probe";
        sandbox.appendChild(image);
        const imageLoaded = new Promise(function (resolve, reject) {
            image.addEventListener("load", resolve, { once: true });
            image.addEventListener("error", function () {
                reject(new Error("Referer probe image failed to load"));
            }, { once: true });
        });
        // The fixture host is another origin than the page server, so the policy
        // (strict-origin-when-cross-origin) must send only the page origin.
        image.src = "http://127.0.0.1:49123/referer.png?run=" + Date.now();
        await imageLoaded;

        const response = await fetch("http://127.0.0.1:49123/api/image-referer?run=" + Date.now());
        const referer = await response.text();
        assert(referer === location.origin + "/", "cross-origin image Referer is the page origin");
    }

    document.getElementById("run").addEventListener("click", async function () {
        try {
            await run();
            result.textContent = "READY: domb PASS";
        } catch (error) {
            result.textContent = "FAIL: " + error.message;
        }
    });
}());
