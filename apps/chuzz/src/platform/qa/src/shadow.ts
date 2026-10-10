(() => {
    "use strict";

    const check = (condition, message) => {
        if (!condition) throw new Error(message);
    };
    const fixture = document.getElementById("fixture");
    const result = document.getElementById("result");
    const parsedApp = document.getElementById("parsed-app");
    const parsedMasthead = document.getElementById("parsed-masthead");
    const parsedLogo = document.getElementById("parsed-logo");
    const parsedSearch = document.getElementById("parsed-search");
    const parsedIcons = document.getElementById("parsed-icons");
    const appTemplate = document.getElementById("parsed-app-template");
    const mastheadTemplate = document.getElementById("parsed-masthead-template");
    const parsedConstructors = [];

    class MastheadApp extends HTMLElement {
        constructor() {
            super();
            parsedConstructors.push(this);
            this.parserChildren = Array.from(this.children);
            const root = this.attachShadow({ mode: "open" });
            // Resolve the empty root before inserting the cloned named slot.
            // Insertion must dirty a host whose children already exist.
            this.getBoundingClientRect();
            root.appendChild(appTemplate.content.cloneNode(true));
        }
    }

    class Masthead extends HTMLElement {
        constructor() {
            super();
            parsedConstructors.push(this);
            this.parserChildren = Array.from(this.children);
            const root = this.attachShadow({ mode: "open" });
            this.getBoundingClientRect();
            root.appendChild(mastheadTemplate.content.cloneNode(true));
        }
    }

    const checkParsedMasthead = label => {
        // Style and layout reads must distribute without an assignment read
        // first. Keep these nodes identical to the original parsed markup.
        check(getComputedStyle(parsedMasthead).backgroundColor === "rgb(30, 55, 75)",
            label + ": parsed nested host stylesheet");
        for (const [node, color] of [
            [parsedLogo, "rgb(150, 210, 240)"],
            [parsedSearch, "rgb(255, 210, 80)"],
            [parsedIcons, "rgb(150, 210, 240)"]
        ]) {
            check(getComputedStyle(node).color === color,
                label + ": named slotted descendant style: " + node.id);
            const box = node.getBoundingClientRect();
            check(box.height >= 32 && box.width > 0,
                label + ": named slotted descendant layout: " + node.id);
        }
        check(parsedMasthead.getBoundingClientRect().height >= 56,
            label + ": parsed nested host layout");

        const appSlot = parsedApp.shadowRoot.querySelector('slot[name="masthead"]');
        check(appSlot.assignedElements().length === 1
            && appSlot.assignedElements()[0] === parsedMasthead
            && parsedMasthead.assignedSlot === appSlot,
            label + ": parsed masthead assignment");
        for (const [name, node] of [
            ["logo", parsedLogo],
            ["search", parsedSearch],
            ["icons", parsedIcons]
        ]) {
            const slot = parsedMasthead.shadowRoot.querySelector('slot[name="' + name + '"]');
            check(slot.assignedElements().length === 1
                && slot.assignedElements()[0] === node
                && node.assignedSlot === slot,
                label + ": nested named slot assignment: " + name);
        }
        check(parsedMasthead.parentNode === parsedApp
            && parsedLogo.parentNode === parsedMasthead
            && parsedSearch.parentNode === parsedMasthead
            && parsedIcons.parentNode === parsedMasthead,
            label + ": composition changed parsed DOM parents");
    };

    document.getElementById("run").addEventListener("click", async () => {
        try {
            fixture.replaceChildren();
            if (!customElements.get("qa-masthead-app")) {
                check(parsedApp.shadowRoot === null && parsedMasthead.shadowRoot === null,
                    "Parsed hosts upgraded before their definitions");
                check(parsedMasthead.slot === "masthead"
                    && parsedLogo.slot === "logo"
                    && parsedSearch.slot === "search"
                    && parsedIcons.slot === "icons",
                    "Parser-created slot attributes");
                customElements.define("qa-masthead-app", MastheadApp);
                customElements.define("qa-masthead", Masthead);
            }
            check(parsedApp instanceof MastheadApp && parsedMasthead instanceof Masthead
                && parsedConstructors.length === 2
                && parsedConstructors[0] === parsedApp
                && parsedConstructors[1] === parsedMasthead,
                "Late parsed upgrade constructor identity");
            check(parsedApp.parserChildren[0] === parsedMasthead
                && parsedMasthead.parserChildren.length === 3
                && parsedMasthead.parserChildren[0] === parsedLogo
                && parsedMasthead.parserChildren[1] === parsedSearch
                && parsedMasthead.parserChildren[2] === parsedIcons,
                "Late constructors did not receive parser-created children");
            checkParsedMasthead("Cloned parsed templates");

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
            // An element the outer root's own `span` rule does not match: across tree
            // contexts the outer normal declaration would win over ::slotted.
            const nestedLabel = document.createElement("em");
            nestedLabel.textContent = "Nested slot";
            nestedHost.appendChild(nestedLabel);
            const nestedRoot = nestedHost.attachShadow({ mode: "open" });
            nestedRoot.innerHTML =
                '<style>:host { display: block; } slot { display: contents; }'
                + '::slotted(em) { display: block; min-height: 20px; '
                + 'color: var(--nested-ink); }</style><slot></slot>';
            check(getComputedStyle(nestedLabel).color === "rgb(100, 110, 120)"
                && nestedLabel.getBoundingClientRect().height >= 20,
                "Nested root did not style parents before assigned descendants");
            probe.remove();

            const donor = document.createElement("div");
            const witness = document.createElement("span");
            witness.slot = "masthead";
            witness.textContent = "Moved slot witness";
            donor.appendChild(witness);
            fixture.appendChild(donor);
            const donorRoot = donor.attachShadow({ mode: "open" });
            donorRoot.appendChild(appTemplate.content.cloneNode(true));
            const checkWitness = label => {
                check(getComputedStyle(witness).color === "rgb(150, 210, 240)"
                    && witness.getBoundingClientRect().height >= 32,
                    label + ": moved slot witness style and layout");
                const slot = donorRoot.querySelector('slot[name="masthead"]');
                check(slot.assignedElements().includes(witness)
                    && witness.assignedSlot === slot,
                    label + ": moved slot witness assignment");
            };
            checkWitness("Before slot-container swap");
            checkParsedMasthead("Before slot-container swap");

            // Both roots have assigned children. Swapping their slot
            // containers makes cleanup by either previous owner observable,
            // regardless of the order dirty hosts are recomputed.
            const appContainer = parsedApp.shadowRoot.querySelector("#masthead-container");
            const donorContainer = donorRoot.querySelector("#masthead-container");
            donorRoot.appendChild(appContainer);
            parsedApp.shadowRoot.appendChild(donorContainer);
            checkParsedMasthead("After slot-container swap");
            checkWitness("After slot-container swap");
            parsedApp.shadowRoot.appendChild(appContainer);
            donorRoot.appendChild(donorContainer);
            checkParsedMasthead("After slot-container restoration");
            checkWitness("After slot-container restoration");

            // Moving the nested host must also replace its reverse assignment
            // without the previous host clearing the newly assignedSlot.
            donor.appendChild(parsedMasthead);
            check(parsedMasthead.getBoundingClientRect().height >= 56,
                "Moved nested host layout");
            const donorSlot = donorRoot.querySelector('slot[name="masthead"]');
            check(donorSlot.assignedElements().includes(parsedMasthead)
                && parsedMasthead.assignedSlot === donorSlot,
                "Moved nested host reverse assignment");
            parsedApp.appendChild(parsedMasthead);
            checkParsedMasthead("After nested-host restoration");
            donor.remove();
            await new Promise(resolve => setTimeout(resolve, 0));
            checkParsedMasthead("After mutation checkpoint");

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

(() => {
    "use strict";

    const check = (condition, message) => {
        if (!condition) throw new Error(message);
    };
    const iconTemplate = document.getElementById("masthead2-icon-template");
    const searchTemplate = document.getElementById("masthead2-search-template");
    const bandTemplate = document.getElementById("masthead2-band-template");
    const fixture = document.getElementById("masthead2-fixture");
    const result = document.getElementById("masthead2-result");
    const svgNamespace = "http://www.w3.org/2000/svg";

    class Icon extends HTMLElement {
        constructor() {
            super();
            const root = this.attachShadow({ mode: "open" });
            this.getBoundingClientRect();
            root.appendChild(document.importNode(iconTemplate.content, true));
        }

        connectedCallback() {
            if (this.shadowRoot.querySelector('svg[role="img"]')) return;
            const original = this.shadowRoot.getElementById("qa-masthead2-glyph");
            const glyph = original.cloneNode(true);
            const svg = document.createElementNS(svgNamespace, "svg");
            svg.setAttribute("viewBox", "0 0 24 24");
            svg.setAttribute("preserveAspectRatio", "xMidYMid meet");
            svg.setAttribute("role", "img");
            svg.setAttribute("aria-label", this.getAttribute("data-label"));
            svg.style.cssText =
                "pointer-events: none; display: block; width: 100%; height: 100%;";
            svg.appendChild(glyph).removeAttribute("id");
            this.shadowRoot.appendChild(svg);
            check(original.id === "qa-masthead2-glyph" && glyph !== original,
                "Icon cloning changed the source definition");
        }
    }

    class Search extends HTMLElement {
        constructor() {
            super();
            const root = this.attachShadow({ mode: "open" });
            this.getBoundingClientRect();
            root.appendChild(document.importNode(searchTemplate.content, true));
        }
    }

    class Conditional extends HTMLTemplateElement {
        constructor() {
            super();
            this.rendered = null;
        }

        static get observedAttributes() { return ["data-open"]; }

        connectedCallback() { this.render(); }

        attributeChangedCallback() {
            if (this.isConnected) this.render();
        }

        render() {
            if (this.rendered) {
                this.rendered.remove();
                this.rendered = null;
            }
            if (!this.hasAttribute("data-open")) return;
            const fragment = document.importNode(this.content, true);
            this.rendered = fragment.querySelector(".sign-in");
            this.parentNode.insertBefore(fragment, this);
        }
    }

    class Band extends HTMLElement {
        constructor() {
            super();
            const root = this.attachShadow({ mode: "open" });
            this.getBoundingClientRect();
            root.appendChild(document.importNode(bandTemplate.content, true));
        }
    }

    customElements.define("qa-masthead2-icon", Icon);
    customElements.define("qa-masthead2-search", Search);
    customElements.define("qa-masthead2-if", Conditional, { extends: "template" });
    customElements.define("qa-masthead2-band", Band);

    document.getElementById("run-masthead2").addEventListener("click", async () => {
        try {
            const band = document.createElement("qa-masthead2-band");
            fixture.replaceChildren(band);
            const root = band.shadowRoot;
            check(band.getBoundingClientRect().height >= 56, "Cloned masthead band layout");

            const icons = Array.from(root.querySelectorAll("qa-masthead2-icon"));
            check(icons.length === 2 && icons.every(icon => icon instanceof Icon),
                "Imported nested icon upgrades");
            const localDefinitions = [];
            for (const [index, icon] of icons.entries()) {
                const svg = icon.shadowRoot.querySelector('svg[role="img"]');
                const color = index === 0 ? "rgb(150, 210, 240)" : "rgb(255, 210, 80)";
                check(svg && svg.namespaceURI === svgNamespace
                    && svg.firstElementChild.namespaceURI === svgNamespace,
                    "Cloned SVG namespace and insertion");
                check(svg.getAttribute("viewBox") === "0 0 24 24",
                    "Cloned SVG viewBox spelling");
                check(getComputedStyle(svg).color === color
                    && getComputedStyle(svg).display === "block",
                    "Nested cloned SVG computed style");
                const box = svg.getBoundingClientRect();
                check(box.width >= 39 && box.height >= 39, "Nested cloned SVG layout");
                const definition = icon.shadowRoot.getElementById("qa-masthead2-shape");
                check(definition.getRootNode() === icon.shadowRoot
                    && definition !== document.getElementById("qa-masthead2-shape"),
                    "Icon definitions remain scoped to their root");
                localDefinitions.push(definition);
                check(svg.querySelector("use").getAttribute("href") === "#qa-masthead2-alias",
                    "Cloned icon retained its sibling-definition reference");
                check(icon.shadowRoot.getElementById("qa-masthead2-alias")
                    .firstElementChild.getAttributeNS(
                        "http://www.w3.org/1999/xlink", "href"
                    ) === "#qa-masthead2-shape", "Imported xlink reference");
            }
            check(localDefinitions[0] !== localDefinitions[1],
                "Icon roots share a source definition");

            const search = root.querySelector("qa-masthead2-search");
            const input = search.querySelector("input");
            check(search instanceof Search && input.slot === "search-input",
                "Imported search upgrade and named light input");
            check(getComputedStyle(input).backgroundColor === "rgb(255, 255, 255)"
                && getComputedStyle(input).borderTopWidth === "2px",
                "Imported slotted search computed style");
            const inputBox = input.getBoundingClientRect();
            check(inputBox.width >= 259 && inputBox.height >= 39,
                "Imported slotted search layout");
            const slot = search.shadowRoot.getElementById("input-slot");
            check(getComputedStyle(slot).display === "contents"
                && slot.assignedElements()[0] === input
                && input.assignedSlot === slot, "Search contents slot assignment");

            const conditional = root.getElementById("masthead2-if");
            check(conditional instanceof Conditional
                && conditional.content.querySelector(".sign-in"),
                "Imported customized template and content");
            const checkSignIn = () => {
                const button = root.querySelector(".sign-in");
                check(button && button === conditional.rendered
                    && getComputedStyle(button).backgroundColor === "rgb(0, 110, 145)",
                    "Conditional Sign in style");
                const box = button.getBoundingClientRect();
                check(box.height >= 39 && box.width > 40, "Conditional Sign in layout");
            };
            checkSignIn();
            conditional.removeAttribute("data-open");
            check(root.querySelector(".sign-in") === null, "Conditional removal");
            conditional.setAttribute("data-open", "");
            checkSignIn();
            check(bandTemplate.content.querySelector("#masthead2-if")
                .hasAttribute("data-open"), "Stamping changed the source template");
            await new Promise(resolve => setTimeout(resolve, 0));
            checkSignIn();
            result.textContent = "PASS: cloned masthead";
        } catch (error) {
            result.textContent = "FAIL: cloned masthead: " + error.message;
            console.error(error);
        }
    });
})();
