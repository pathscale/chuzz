(() => {
    "use strict";

    const check = (condition, message) => {
        if (!condition) throw new Error(message);
    };

    document.getElementById("run").addEventListener("click", () => {
        const result = document.getElementById("result");
        const fixture = document.getElementById("fixture");
        try {
            fixture.textContent = "";
            const parser = new DOMParser();
            const serializer = new XMLSerializer();
            const parsed = parser.parseFromString(
                '<!doctype html><html><head><title>Detached title</title>'
                + '<style>body { display: none; }</style></head>'
                + '<body><section id="parsed"><b>Parsed content</b></section>'
                + '<script>globalThis.__qaParsedScriptExecuted = true;</script></body></html>',
                "text/html"
            );
            check(parsed !== document && parsed.defaultView === null, "Parsed document is not inert");
            check(parsed instanceof Document && parsed instanceof HTMLDocument, "HTML document interface");
            check(parsed.contentType === "text/html", "HTML contentType");
            check(parsed.title === "Detached title", "Parsed title");
            check(document.title === "DOMParser QA", "Parser changed the live title");
            check(parsed.getElementById("parsed") === parsed.querySelector("#parsed"), "Detached document lookup");
            check(parsed.querySelectorAll("#parsed").length === 1, "Detached querySelectorAll");
            check(parsed.body.ownerDocument === parsed, "Parsed node ownerDocument");
            check(parsed.ownerDocument === null, "Document ownerDocument");
            check(globalThis.__qaParsedScriptExecuted === undefined, "Parsed script executed");
            check(document.getElementById("parsed") === null, "Parser changed the live document");

            const colliding = document.createElement("div");
            colliding.id = "parsed";
            fixture.appendChild(colliding);
            check(parsed.getElementById("parsed") !== colliding, "Lookup used the live ID index");
            check(document.getElementById("parsed") === colliding, "Parsed ID polluted the live index");
            colliding.remove();

            const section = parsed.getElementById("parsed");
            section.id = "renamed";
            check(parsed.getElementById("parsed") === null, "Detached ID rename left a stale entry");
            check(parsed.getElementById("renamed") === section, "Detached ID rename was not indexed");
            section.remove();
            check(section.ownerDocument === parsed, "Removal lost document ownership");
            check(parsed.getElementById("renamed") === null, "Removed element remained searchable");
            parsed.body.appendChild(section);
            check(parsed.getElementById("renamed") === section, "Reinsertion lost the ID entry");

            const imported = document.importNode(section, true);
            check(imported.ownerDocument === document, "Import retained the source owner");
            check(imported.firstChild.ownerDocument === document, "Import did not adopt descendants");
            fixture.appendChild(imported);
            check(imported.textContent === "Parsed content", "Imported parsed content");

            const importedScript = document.importNode(parsed.querySelector("script"), true);
            fixture.appendChild(importedScript);
            check(globalThis.__qaParsedScriptExecuted === undefined, "Imported parser script executed");

            const htmlXML = serializer.serializeToString(parsed);
            check(htmlXML.includes("<!DOCTYPE html>"), "HTML doctype was lost");
            check(htmlXML.includes('xmlns="http://www.w3.org/1999/xhtml"'), "HTML XML serialization namespace");
            check(htmlXML.includes("&lt;") || htmlXML.includes("Parsed content"), "HTML serialization");

            for (const type of [
                "application/xml",
                "text/xml",
                "application/xhtml+xml",
                "image/svg+xml"
            ]) {
                const xml = parser.parseFromString(
                    '<?xml version="1.0"?><?qa keep?><!DOCTYPE Root>'
                    + '<Root xmlns:p="urn:qa" id="xml-root">'
                    + '<p:Item p:value="&quot;&amp;">A &amp; B &lt; C</p:Item>'
                    + '<!-- retained --></Root>',
                    type
                );
                check(xml instanceof XMLDocument, "XML document interface: " + type);
                check(xml.contentType === type && xml.defaultView === null, "XML document metadata: " + type);
                check(xml.documentElement.nodeName === "Root", "XML name case: " + type);
                check(xml.documentElement.tagName === "Root", "XML tagName case: " + type);
                check(xml.querySelector("Root") === xml.documentElement, "XML root selector: " + type);
                check(xml.querySelector("root") === null, "XML selector folded case: " + type);
                check(xml.getElementById("xml-root") === xml.documentElement, "XML ID lookup: " + type);
                check(xml.doctype.name === "Root" && xml.doctype.nodeType === 10, "XML doctype: " + type);
                check(xml.firstChild.nodeType === 7 && xml.firstChild.target === "qa", "XML processing instruction: " + type);

                const serialized = serializer.serializeToString(xml);
                check(serialized.includes("<?qa keep?>"), "PI serialization: " + type);
                check(serialized.includes("<!-- retained -->"), "Comment serialization: " + type);
                const roundtrip = parser.parseFromString(serialized, type);
                check(roundtrip.documentElement.nodeName === "Root", "XML roundtrip: " + type);
                check(roundtrip.documentElement.firstChild.namespaceURI === "urn:qa", "Namespace roundtrip: " + type);
                check(roundtrip.documentElement.firstChild.textContent === "A & B < C", "Text escaping: " + type);

                const standalone = serializer.serializeToString(xml.documentElement.firstChild);
                const standaloneDoc = parser.parseFromString(standalone, type);
                check(standaloneDoc.documentElement.namespaceURI === "urn:qa", "Standalone namespace repair: " + type);

                for (const malformed of ["<root><broken></root>", "<one/><two/>", "<root>&missing;</root>", ""]) {
                    const invalid = parser.parseFromString(malformed, type);
                    check(invalid.documentElement.localName === "parsererror", "XML parsererror: " + type);
                    check(invalid.defaultView === null && invalid.contentType === type, "Error document metadata: " + type);
                }
            }

            const forcedHTML = parser.parseFromString('<?xml version="1.0"?><Root>HTML mode</Root>', "text/html");
            check(forcedHTML.documentElement.localName === "html" && forcedHTML.body !== null, "MIME type did not select HTML parser");

            let unsupported = false;
            try {
                parser.parseFromString("<root/>", "application/json");
            } catch (error) {
                unsupported = error instanceof TypeError;
            }
            check(unsupported, "Unsupported MIME type did not throw TypeError");

            result.textContent = "PASS: parser";
        } catch (error) {
            result.textContent = "FAIL: parser: " + error.message;
            console.error(error);
        }
    });
})();
