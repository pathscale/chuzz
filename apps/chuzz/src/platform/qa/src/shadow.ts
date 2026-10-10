(() => {
    "use strict";

    const check = (condition, message) => {
        if (!condition) throw new Error(message);
    };
    const fixture = document.getElementById("fixture");
    const result = document.getElementById("result");

    document.getElementById("run").addEventListener("click", async () => {
        try {
            fixture.replaceChildren();
            const host = document.createElement("div");
            host.id = "styled-host";
            host.className = "card";
            host.style.setProperty("--shadow-ink", "rgb(240, 248, 255)");
            fixture.appendChild(host);
            const root = host.attachShadow({ mode: "open" });
            root.innerHTML =
                '<style>'
                + ':host { display: block; padding: 16px; background: rgb(30, 55, 75); }'
                + ':host(.card) { border-radius: 8px; }'
                + 'button { background: rgb(0, 110, 145); color: var(--shadow-ink); '
                + 'font: inherit; padding: 12px 20px; }'
                + 'slot { display: contents; }'
                + '::slotted(span) { color: rgb(255, 210, 80); font-weight: bold; }'
                + '</style>'
                + '<button id="inside" part="action" type="button">Shadow button</button>'
                + '<slot name="label"></slot><slot><span id="fallback">Fallback label</span></slot>';

            check(root instanceof ShadowRoot, "ShadowRoot interface");
            check(host.shadowRoot === root && root.host === host && root.mode === "open",
                "Open shadow root identity");
            check(root.parentNode === null && root.getRootNode() === root, "Shadow tree boundary");
            check(root.getRootNode({ composed: true }) === document, "Composed root");
            check(root.getElementById("inside") === root.querySelector("#inside"), "Shadow ID lookup");
            check(document.querySelector("#inside") === null, "Document query crossed shadow boundary");
            check(document.getElementById("inside") === null, "Document ID index crossed shadow boundary");
            check(root.querySelector("#inside").matches("#inside"), "Shadow scoped selector");

            let rejected = false;
            try { host.attachShadow({ mode: "open" }); } catch (_) { rejected = true; }
            check(rejected, "Second attachShadow did not throw");

            const slot = root.querySelector('slot[name="label"]');
            const defaultSlot = root.querySelector("slot:not([name])");
            let changes = 0;
            slot.addEventListener("slotchange", () => { changes++; });
            const light = document.createElement("span");
            light.slot = "label";
            light.textContent = "Slotted label";
            host.appendChild(light);
            check(slot.assignedNodes()[0] === light && light.assignedSlot === slot,
                "Named slot assignment");
            check(slot.assignedElements()[0] === light, "assignedElements");
            check(defaultSlot.assignedNodes().length === 0, "Fallback counted as assignment");
            check(defaultSlot.assignedNodes({ flatten: true })[0].id === "fallback",
                "Flattened fallback");

            light.slot = "missing";
            check(defaultSlot.assignedNodes().length === 0, "Unknown slot used default slot");
            check(light.assignedSlot === null, "Unknown slot retained an assignment");
            light.slot = "label";
            const text = document.createTextNode(" Default text");
            host.appendChild(text);
            check(defaultSlot.assignedNodes()[0] === text && text.assignedSlot === defaultSlot,
                "Text-node slot assignment");
            check(defaultSlot.assignedElements().length === 0, "Text counted as assigned element");
            check(!defaultSlot.matches(":empty"), "Slot selector ignored DOM fallback children");
            text.remove();
            await new Promise(resolve => setTimeout(resolve, 0));
            check(changes > 0, "slotchange was not delivered");

            const sheet = new CSSStyleSheet();
            const rules = sheet.cssRules;
            sheet.replaceSync("button { min-height: 48px; }");
            check(rules.length === 1 && rules[0].cssText.includes("min-height"), "Live CSSRuleList");
            root.adoptedStyleSheets = [sheet];
            check(root.adoptedStyleSheets[0] === sheet, "Adopted stylesheet identity");
            const inserted = sheet.insertRule("button { letter-spacing: 1px; }", 1);
            check(inserted === 1 && rules.length === 2, "insertRule");
            sheet.deleteRule(1);
            check(rules.length === 1, "deleteRule");
            check(await sheet.replace("button { min-height: 48px; }") === sheet, "replace result");

            const documentSheet = new CSSStyleSheet();
            documentSheet.replaceSync("#result { min-height: 24px; }");
            document.adoptedStyleSheets = [documentSheet];
            check(document.adoptedStyleSheets[0] === documentSheet, "Document adoptedStyleSheets");
            const extraSheet = new CSSStyleSheet();
            extraSheet.replaceSync("button { cursor: pointer; }");
            root.adoptedStyleSheets.push(extraSheet);
            check(root.adoptedStyleSheets.length === 2, "Observable adoptedStyleSheets");

            const closedHost = document.createElement("div");
            fixture.appendChild(closedHost);
            const closed = closedHost.attachShadow({ mode: "closed" });
            closed.innerHTML = '<button type="button">Closed shadow button</button>';
            const button = closed.firstChild;
            check(closedHost.shadowRoot === null && closed.mode === "closed", "Closed root exposed");
            let outside = false;
            closedHost.addEventListener("qa-shadow", event => {
                outside = event.target === closedHost
                    && !event.composedPath().includes(button)
                    && !event.composedPath().includes(closed);
            });
            button.dispatchEvent(new CustomEvent("qa-shadow", { bubbles: true, composed: true }));
            check(outside, "Closed composed path or retargeting");
            let escaped = false;
            closedHost.addEventListener("qa-local", () => { escaped = true; });
            button.dispatchEvent(new Event("qa-local", { bubbles: true }));
            check(!escaped, "Non-composed event escaped shadow root");

            let openPath = false;
            host.addEventListener("qa-open", event => {
                openPath = event.target === host && event.composedPath().includes(root);
            });
            root.querySelector("#inside").dispatchEvent(
                new Event("qa-open", { bubbles: true, composed: true })
            );
            check(openPath, "Open composed path or retargeting");

            const inside = root.querySelector("#inside");
            const style = getComputedStyle(inside);
            check(style.backgroundColor === "rgb(0, 110, 145)", "Shadow stylesheet did not apply");
            check(style.color === "rgb(240, 248, 255)", "Custom property did not inherit through host");
            check(getComputedStyle(light).color === "rgb(255, 210, 80)", "::slotted did not apply");
            check(style.borderTopWidth === "3px", "::part did not apply");
            check(inside.getBoundingClientRect().height >= 48, "Shadow button did not lay out");

            // DOM order deliberately differs from slot traversal order. These
            // otherwise identical spans also exercise Stylo's sharing path.
            const probe = document.createElement("div");
            fixture.appendChild(probe);
            const second = document.createElement("span");
            second.className = "sample";
            second.slot = "second";
            second.textContent = "Second slot";
            const first = document.createElement("span");
            first.className = "sample";
            first.slot = "first";
            first.textContent = "First slot";
            probe.appendChild(second);
            probe.appendChild(first);
            const probeRoot = probe.attachShadow({ mode: "open" });
            probeRoot.innerHTML =
                '<style>'
                + ':host { display: block; }'
                + 'slot { display: contents; }'
                + '#first { --probe-ink: rgb(10, 20, 30); }'
                + '#second { --probe-ink: rgb(40, 50, 60); }'
                + 'span, ::slotted(.sample) { display: block; min-height: 20px; '
                + 'color: var(--probe-ink); }'
                + '</style>'
                + '<slot id="first" name="first"><span id="empty">Probe fallback</span></slot>'
                + '<slot id="second" name="second"></slot>';
            check(getComputedStyle(first).color === "rgb(10, 20, 30)",
                "First slot was not styled before its assigned node");
            check(getComputedStyle(second).color === "rgb(40, 50, 60)",
                "Style sharing used a different slot's inherited style");
            check(first.getBoundingClientRect().height >= 20
                && second.getBoundingClientRect().height >= 20,
                "Assigned nodes did not produce layout boxes");
            check(second.matches(":first-child") && first.matches(":last-child"),
                "Slot traversal changed DOM sibling selectors");
            check(probe.querySelector("span") === second
                && probeRoot.querySelector("#first").firstElementChild.id === "empty",
                "Slot traversal changed DOM child queries");

            probeRoot.querySelector("#first").style.setProperty(
                "--probe-ink", "rgb(70, 80, 90)"
            );
            check(getComputedStyle(first).color === "rgb(70, 80, 90)",
                "Slot mutation did not restyle its assigned subtree");
            first.slot = "second";
            check(getComputedStyle(first).color === "rgb(40, 50, 60)",
                "Reassignment retained the previous inheritance parent");
            const fallback = probeRoot.querySelector("#empty");
            check(getComputedStyle(fallback).color === "rgb(70, 80, 90)"
                && fallback.getBoundingClientRect().height >= 20,
                "Reassignment did not restore styled fallback content");
            first.slot = "first";
            check(getComputedStyle(first).color === "rgb(70, 80, 90)",
                "Reassignment back to the first slot did not restyle");
            second.remove();
            const replacement = document.createElement("span");
            replacement.className = "sample";
            replacement.slot = "second";
            replacement.textContent = "Replacement slot child";
            probe.appendChild(replacement);
            check(getComputedStyle(replacement).color === "rgb(40, 50, 60)"
                && replacement.getBoundingClientRect().height >= 20,
                "Insertion after removal did not style and lay out the new assignment");

            const nestedHost = document.createElement("div");
            nestedHost.style.setProperty("--nested-ink", "rgb(100, 110, 120)");
            probeRoot.appendChild(nestedHost);
            const nestedLabel = document.createElement("span");
            nestedLabel.textContent = "Nested slot";
            nestedHost.appendChild(nestedLabel);
            const nestedRoot = nestedHost.attachShadow({ mode: "open" });
            nestedRoot.innerHTML =
                '<style>:host { display: block; } slot { display: contents; }'
                + '::slotted(span) { display: block; min-height: 20px; '
                + 'color: var(--nested-ink); }</style><slot></slot>';
            check(getComputedStyle(nestedLabel).color === "rgb(100, 110, 120)"
                && nestedLabel.getBoundingClientRect().height >= 20,
                "Nested root did not style parents before assigned descendants");
            probe.remove();

            inside.addEventListener("click", () => {
                result.textContent = "PASS: shadow button interaction";
            });
            result.textContent = "PASS: shadow";
        } catch (error) {
            result.textContent = "FAIL: shadow: " + error.message;
            console.error(error);
        }
    });
})();

