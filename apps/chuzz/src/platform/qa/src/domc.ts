(function () {
    "use strict";

    const proof = globalThis.__domcProof;
    proof.deferred = document.readyState === "interactive" &&
        document.currentScript !== null && document.currentScript.id === "suite";

    const viewportQuery = matchMedia("(min-width: 900px)");
    let nativeChanges = 0;
    viewportQuery.addEventListener("change", function (event) {
        nativeChanges++;
        const correct = event instanceof MediaQueryListEvent &&
            event.matches === viewportQuery.matches && event.media === viewportQuery.media &&
            event.target === viewportQuery && event.isTrusted === true;
        document.getElementById("media-result").textContent = correct
            ? "READY: DOMC native media change PASS " + nativeChanges
            : "FAIL: native media change fields";
    });

    function assert(condition, message) {
        if (!condition) throw new Error(message);
    }

    document.getElementById("run").addEventListener("click", function () {
        const cleanup = [];
        const result = document.getElementById("result");
        try {
            assert(proof.classic === "boot", "inline classic currentScript");
            assert(proof.deferred, "deferred script is interactive and has currentScript");
            assert(proof.module, "module currentScript is null");
            assert(proof.microtask, "microtask currentScript is null");
            assert(document.currentScript === null, "input callback currentScript is null");
            assert(document.readyState === "complete", "document is complete");
            assert(
                proof.order.join("|") ===
                    "loading:loading|interactive|DOMContentLoaded:interactive|complete|load:complete",
                "readiness transition and event ordering"
            );
            assert(document.visibilityState === "visible" && document.hidden === false,
                "visible document state");
            assert(document.characterSet === "UTF-8", "characterSet");
            assert(document.location === location, "document.location identity");
            assert(document.domain === location.hostname, "domain reflects URL host");

            const probe = document.getElementById("probe");
            const inline = document.getElementById("inline-probe");
            const computed = getComputedStyle(probe);
            assert(computed instanceof CSSStyleDeclaration, "computed declaration interface");
            assert(Object.prototype.toString.call(computed) === "[object CSSStyleDeclaration]",
                "computed declaration tag");
            assert(getComputedStyle(inline).display === "inline", "span display is inline");
            assert(computed.display === "block" &&
                computed.getPropertyValue("display") === "block", "div display is block");
            assert(computed.letterSpacing === "7px" &&
                computed.getPropertyValue("letter-spacing") === "7px",
                "longhand and camelCase serialization");
            assert(computed.getPropertyValue("--DomcCase").trim() === "CaseValue",
                "custom property value and case");
            assert(computed.getPropertyValue("--domccase") === "", "custom property case sensitivity");
            assert(computed.length > 20 && computed.item(0) === computed[0],
                "computed declaration length and indexed item");
            assert(Array.from(computed).includes("letter-spacing"), "declaration iteration");
            assert(computed.item(4294967295) === "", "out-of-range declaration item");
            assert(computed.cssText === "" &&
                computed.getPropertyPriority("letter-spacing") === "",
                "computed cssText and priority");
            assert(computed.getPropertyValue("not-a-css-property") === "",
                "unknown computed property");

            let readOnly = false;
            try { computed.setProperty("display", "none"); }
            catch (error) { readOnly = error.name === "NoModificationAllowedError"; }
            assert(readOnly, "computed declaration rejects setProperty");
            readOnly = false;
            try { computed.display = "none"; }
            catch (error) { readOnly = error.name === "NoModificationAllowedError"; }
            assert(readOnly && computed.display === "block", "computed declaration rejects assignment");

            const previousStyle = probe.getAttribute("style");
            cleanup.push(function () {
                if (previousStyle === null) probe.removeAttribute("style");
                else probe.setAttribute("style", previousStyle);
            });
            probe.style.setProperty("display", "inline-block");
            assert(computed.display === "inline-block", "computed declaration stays live");
            probe.style.setProperty("letter-spacing", "11px", "important");
            assert(probe.style.getPropertyPriority("letter-spacing") === "important" &&
                probe.style.getPropertyValue("letter-spacing") === "11px" &&
                computed.letterSpacing === "11px", "inline important priority and restyle");
            probe.style.setProperty("--Punctuation", "\"a;b:c\"");
            assert(probe.style.getPropertyValue("--Punctuation").trim() === "\"a;b:c\"",
                "CSS parsing preserves punctuation");
            assert(probe.style.cssText.includes("!important"), "inline cssText serialization");
            assert(probe.style.length > 0 && probe.style.item(0) !== "",
                "inline declaration length and item");
            assert(probe.style.removeProperty("letter-spacing") === "11px" &&
                computed.letterSpacing === "7px", "removeProperty returns value and restyles");

            const before = getComputedStyle(probe, "::before");
            assert(before.display === "inline-block" && before.letterSpacing === "3px",
                "pseudo-element computed style");
            assert(getComputedStyle(probe, ":before").display === before.display,
                "legacy pseudo-element spelling");

            assert(CSS.supports("display", "grid"), "supports property and value");
            assert(CSS.supports("(display: grid) and (color: red)"), "supports condition");
            assert(!CSS.supports("display", "definitely-invalid"), "supports rejects invalid value");
            assert(!CSS.supports("(display: grid) and nonsense"), "supports consumes entire condition");
            assert(CSS.escape("1 a#") === "\\31 \\ a\\#", "CSS.escape digits and punctuation");
            assert(CSS.escape("\0") === "\uFFFD" && CSS.escape("-") === "\\-",
                "CSS.escape null and lone hyphen");
            const escaped = document.createElement("div");
            escaped.id = "1 a#";
            document.body.appendChild(escaped);
            cleanup.push(() => escaped.remove());
            assert(document.querySelector("#" + CSS.escape(escaped.id)) === escaped,
                "escaped identifier selects the real node");

            const all = matchMedia("(min-width: 1px)");
            assert(all instanceof MediaQueryList && all.matches === (innerWidth >= 1),
                "media query uses viewport");
            assert(!matchMedia("(min-width: 1000000px)").matches, "unmatched media query");
            assert(!matchMedia("invalid query syntax").matches, "invalid media query");
            let legacy = 0;
            const listener = function (event) {
                assert(event.target === all && event.currentTarget === all,
                    "media event target");
                legacy++;
            };
            all.addListener(listener);
            all.dispatchEvent(new MediaQueryListEvent("change", {
                matches: all.matches,
                media: all.media
            }));
            all.removeListener(listener);
            all.dispatchEvent(new Event("change"));
            assert(legacy === 1, "legacy media listener registration and removal");
            let once = 0;
            all.addEventListener("change", () => once++, { once: true });
            all.dispatchEvent(new Event("change"));
            all.dispatchEvent(new Event("change"));
            assert(once === 1, "media once listener");

            const container = document.createElement("div");
            document.body.appendChild(container);
            cleanup.push(() => container.remove());
            const forms = document.forms;
            const images = document.images;
            const links = document.links;
            const scripts = document.scripts;
            const embeds = document.embeds;
            const named = document.getElementsByName("domc-name");
            const sizes = [forms.length, images.length, links.length, scripts.length, embeds.length];
            assert(forms instanceof HTMLCollection && named instanceof NodeList,
                "document collection interfaces");
            assert(document.plugins === embeds, "plugins aliases embeds");

            const form = document.createElement("form");
            form.id = "domc-form";
            form.setAttribute("name", "domc-name");
            const image = document.createElement("img");
            const link = document.createElement("a");
            link.setAttribute("href", "#domc");
            const script = document.createElement("script");
            script.setAttribute("type", "application/json");
            const embed = document.createElement("embed");
            for (const node of [form, image, link, script, embed]) container.appendChild(node);
            assert(forms.length === sizes[0] + 1 && images.length === sizes[1] + 1 &&
                links.length === sizes[2] + 1 && scripts.length === sizes[3] + 1 &&
                embeds.length === sizes[4] + 1, "live document collection insertion");
            assert(forms.namedItem("domc-form") === form && forms["domc-form"] === form,
                "collection named lookup");
            assert(named.length === 1 && named.item(0) === form &&
                named.entries().next().value[1] === form, "live getElementsByName");
            form.setAttribute("name", "different");
            assert(named.length === 0, "getElementsByName observes attribute changes");

            let preserved = 0;
            form.addEventListener("domc-adoption", () => preserved++);
            assert(document.adoptNode(form) === form && form.parentNode === null &&
                form.ownerDocument === document, "adoptNode detaches and preserves identity");
            form.dispatchEvent(new Event("domc-adoption"));
            assert(preserved === 1 && forms.length === sizes[0], "adoption preserves listeners");
            container.appendChild(form);

            const sheets = document.styleSheets;
            const sheetCount = sheets.length;
            const style = document.createElement("style");
            style.textContent = "#probe { word-spacing: 13px; }";
            document.head.appendChild(style);
            cleanup.push(() => style.remove());
            assert(sheets.length === sheetCount + 1, "live StyleSheetList insertion");
            const sheet = Array.from(sheets).find(value => value.ownerNode === style);
            assert(sheet && sheet.href === null && sheet.disabled === false,
                "inline stylesheet metadata");
            assert(sheet.cssRules.length === 1 &&
                sheet.cssRules.item(0).cssText.includes("word-spacing"), "available stylesheet rules");
            assert(computed.wordSpacing === "13px", "inserted stylesheet affects rendering");
            sheet.disabled = true;
            assert(computed.wordSpacing !== "13px", "disabled stylesheet stops affecting rendering");
            sheet.disabled = false;
            assert(computed.wordSpacing === "13px", "stylesheet reenables");
            sheet.media.mediaText = "(min-width: 1000000px)";
            assert(sheet.media.length === 1 && !computed.wordSpacing.includes("13px"),
                "stylesheet media changes affect rendering");
            sheet.media.mediaText = "";
            assert(computed.wordSpacing === "13px", "stylesheet media reset");
            style.remove();
            assert(sheets.length === sheetCount, "live StyleSheetList removal");

            const front = document.getElementById("front");
            const back = document.getElementById("back");
            assert(document.elementFromPoint(40, 240) === front, "foremost point hit");
            const hits = document.elementsFromPoint(40, 240);
            assert(hits[0] === front && hits.indexOf(back) > 0 &&
                new Set(hits).size === hits.length, "all point hits in paint order");
            front.style.pointerEvents = "none";
            cleanup.push(() => front.style.removeProperty("pointer-events"));
            assert(document.elementFromPoint(40, 240) === back, "hit testing respects pointer-events");
            assert(document.elementFromPoint(-1, 0) === null &&
                document.elementsFromPoint(-1, 0).length === 0, "out-of-viewport hit testing");

            result.textContent = "READY: DOMC PASS";
        } catch (error) {
            result.textContent = "FAIL: " + error.message;
        } finally {
            for (let index = cleanup.length - 1; index >= 0; index--) {
                try { cleanup[index](); } catch (error) { console.error(error); }
            }
        }
    });
})();
