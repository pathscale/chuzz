(() => {
    "use strict";

    const fixture = document.getElementById("fixture");
    const result = document.getElementById("result");

    function check(condition, message) {
        if (!condition) throw new Error(message);
    }

    function throws(name, operation) {
        let caught = false;
        try {
            operation();
        } catch (error) {
            caught = error.name === name;
        }
        check(caught, "Expected " + name);
    }

    function textRange(text, start, end) {
        const range = document.createRange();
        range.setStart(text, start);
        range.setEnd(text, end);
        return range;
    }

    function run() {
        fixture.replaceChildren();
        const selection = getSelection();
        selection.removeAllRanges();

        const initial = document.createRange();
        check(initial instanceof Range && initial.constructor === Range, "Range interface");
        check(initial.startContainer === document && initial.startOffset === 0
            && initial.collapsed, "Initial document boundary");
        check(new Range().startContainer === document, "Range constructor");

        const paragraph = document.createElement("p");
        paragraph.appendChild(document.createTextNode("Hello "));
        const bold = document.createElement("b");
        bold.textContent = "brave";
        paragraph.appendChild(bold);
        paragraph.appendChild(document.createTextNode(" world"));
        fixture.appendChild(paragraph);

        const cross = textRange(paragraph.firstChild, 3, 6);
        cross.setEnd(paragraph.lastChild, 3);
        check(cross.toString() === "lo brave wo", "Cross-container string");
        check(cross.commonAncestorContainer === paragraph, "Common ancestor");
        const copied = cross.cloneContents();
        check(copied.nodeType === 11 && copied.textContent === "lo brave wo",
            "cloneContents text");
        check(copied.childNodes.length === 3 && copied.childNodes[1].tagName === "B"
            && copied.childNodes[1] !== bold, "cloneContents structure and identity");
        check(paragraph.textContent === "Hello brave world", "Clone changed source");

        const independent = cross.cloneRange();
        cross.collapse(true);
        check(cross.collapsed && independent.toString() === "lo brave wo",
            "cloneRange and collapse");
        throws("IndexSizeError", () => cross.setStart(paragraph.firstChild, 999));

        const unicode = document.createTextNode("A\u{1f600}BC");
        paragraph.appendChild(unicode);
        const emoji = textRange(unicode, 1, 3);
        check(unicode.length === 5 && emoji.toString() === "\u{1f600}",
            "UTF-16 boundaries");
        unicode.insertData(0, "Q");
        check(emoji.startOffset === 2 && emoji.endOffset === 4
            && emoji.toString() === "\u{1f600}", "Text insertion updates");
        unicode.deleteData(0, 1);
        check(emoji.startOffset === 1 && emoji.endOffset === 3, "Text deletion updates");
        unicode.replaceData(3, 1, "xyz");
        check(emoji.toString() === "\u{1f600}", "Unrelated text replacement");

        const splitHost = document.createElement("p");
        const splitSource = document.createTextNode("abcdef");
        splitHost.appendChild(splitSource);
        fixture.appendChild(splitHost);
        const splitRange = textRange(splitSource, 4, 6);
        const afterText = document.createRange();
        afterText.setStart(splitHost, 1);
        afterText.collapse(true);
        const tail = splitSource.splitText(3);
        check(splitRange.startContainer === tail && splitRange.startOffset === 1
            && splitRange.toString() === "ef", "splitText transfer");
        check(afterText.startOffset === 2, "splitText parent boundary");
        splitHost.normalize();
        check(splitRange.startContainer === splitSource && splitRange.startOffset === 4
            && splitRange.endOffset === 6 && splitRange.toString() === "ef",
            "normalize boundary transfer");

        const parent = document.createElement("div");
        const first = document.createElement("span");
        first.textContent = "first";
        const second = document.createElement("span");
        second.textContent = "second";
        parent.append(first, second);
        fixture.appendChild(parent);
        const inside = textRange(second.firstChild, 1, 4);
        const after = document.createRange();
        after.setStart(parent, 2);
        after.collapse(true);
        first.remove();
        check(after.startOffset === 1, "Sibling removal shift");
        second.remove();
        check(inside.startContainer === parent && inside.startOffset === 0
            && inside.collapsed && after.startOffset === 0, "Subtree removal retarget");
        check(second.textContent === "second", "Removal destroyed held node");

        const fragment = document.createDocumentFragment();
        const a = document.createElement("i");
        a.textContent = "a";
        const b = document.createElement("i");
        b.textContent = "b";
        fragment.append(a, b);
        const inFragment = document.createRange();
        inFragment.selectNodeContents(fragment);
        parent.appendChild(fragment);
        check(fragment.childNodes.length === 0 && inFragment.collapsed
            && inFragment.startContainer === fragment, "Fragment removal boundaries");

        const childBoundary = document.createRange();
        childBoundary.setStart(parent, 2);
        childBoundary.collapse(true);
        const extra = document.createElement("i");
        parent.insertBefore(extra, parent.firstChild);
        check(childBoundary.startOffset === 3, "Insertion before boundary");
        const equality = document.createRange();
        equality.setStart(parent, 0);
        equality.collapse(true);
        parent.insertBefore(document.createElement("i"), parent.firstChild);
        check(equality.startOffset === 0, "Insertion at boundary");

        const moveRange = textRange(a.firstChild, 0, 1);
        const moveTarget = document.createElement("div");
        fixture.appendChild(moveTarget);
        moveTarget.appendChild(a);
        check(moveRange.startContainer === parent && moveRange.collapsed,
            "Move performs live removal steps");

        const editor = document.createElement("p");
        editor.appendChild(document.createTextNode("0123456789"));
        fixture.appendChild(editor);
        const extracted = textRange(editor.firstChild, 2, 5);
        const extractedFragment = extracted.extractContents();
        check(extractedFragment.textContent === "234" && editor.textContent === "0156789"
            && extracted.collapsed && extracted.startOffset === 2,
            "extractContents CharacterData");

        editor.textContent = "abcdef";
        const deleted = textRange(editor.firstChild, 1, 4);
        deleted.deleteContents();
        check(editor.textContent === "aef" && deleted.collapsed
            && deleted.startOffset === 1, "deleteContents CharacterData");

        editor.textContent = "left ";
        const middle = document.createElement("strong");
        middle.textContent = "middle";
        editor.append(middle, document.createTextNode(" right"));
        const structural = textRange(editor.firstChild, 2, 5);
        structural.setEnd(editor.lastChild, 3);
        const structuralFragment = structural.extractContents();
        check(structuralFragment.textContent === "ft middle ri"
            && structuralFragment.childNodes[1] === middle,
            "Extract preserves fully contained node identity");
        check(editor.textContent === "leght" && structural.collapsed,
            "Cross-container extraction collapse");

        editor.textContent = "abcd";
        const insertion = textRange(editor.firstChild, 2, 2);
        const inserted = document.createElement("strong");
        inserted.textContent = "INSERT";
        insertion.insertNode(inserted);
        check(editor.textContent === "abINSERTcd" && editor.childNodes.length === 3,
            "insertNode splits Text");
        check(!insertion.collapsed && insertion.toString() === "INSERT",
            "Collapsed insertion expands range");

        editor.textContent = "abcdef";
        const surround = textRange(editor.firstChild, 1, 4);
        const wrapper = document.createElement("strong");
        wrapper.textContent = "discard";
        surround.surroundContents(wrapper);
        check(editor.textContent === "abcdef" && wrapper.textContent === "bcd"
            && surround.toString() === "bcd", "surroundContents");

        editor.innerHTML = "<b>abc</b><i>def</i>";
        const partial = textRange(editor.firstChild.firstChild, 1, 3);
        partial.setEnd(editor.lastChild.firstChild, 2);
        const before = editor.innerHTML;
        throws("InvalidStateError", () => partial.surroundContents(document.createElement("u")));
        check(editor.innerHTML === before, "Rejected surround mutated source");

        const parsedDocument = document.implementation.createHTMLDocument("range");
        const parsedRange = parsedDocument.createRange();
        check(parsedRange.startContainer === parsedDocument
            && parsedDocument.getSelection() === null, "Detached document range scope");
        const foreign = parsedDocument.createTextNode("foreign");
        parsedDocument.body.appendChild(foreign);
        partial.setStart(foreign, 2);
        check(partial.collapsed && partial.endContainer === foreign,
            "Changing roots collapses opposite endpoint");

        editor.textContent = "Range selection rendered";
        const selected = document.createRange();
        selected.selectNodeContents(editor);
        selection.addRange(selected);
        check(selection instanceof Selection && selection === document.getSelection()
            && selection === getSelection(), "Selection interface and identity");
        check(selection.rangeCount === 1 && selection.getRangeAt(0) === selected,
            "Selection retains original Range");
        check(selection.toString() === "Range selection rendered", "Selection string");
        throws("IndexSizeError", () => selection.getRangeAt(1));
        selected.collapse(true);
        check(selection.isCollapsed && selection.rangeCount === 1,
            "Collapsed selection still has a range");
        selection.collapse(editor.firstChild, 6);
        check(selection.anchorNode === editor.firstChild && selection.anchorOffset === 6
            && selection.focusOffset === 6, "Selection collapse");

        selection.removeAllRanges();
        selected.selectNodeContents(editor);
        selection.addRange(selected);
        editor.firstChild.insertData(0, "Live ");
        check(selection.getRangeAt(0) === selected
            && selection.toString() === "Live Range selection rendered",
            "Selected range stays live");
        const box = selected.getBoundingClientRect();
        check(box instanceof DOMRect && box.width > 0 && box.height > 0
            && Number.isFinite(box.x) && Number.isFinite(box.y), "Range geometry");
        check(selected.getClientRects().length > 0, "Range line rectangles");

        selection.removeAllRanges();
        check(selection.rangeCount === 0 && selection.isCollapsed
            && selection.toString() === "", "Selection clear");
        throws("IndexSizeError", () => selection.getRangeAt(0));
    }

    document.getElementById("run").addEventListener("click", () => {
        try {
            run();
            result.textContent = "READY: range PASS";
            result.setAttribute("aria-label", "READY: range PASS");
        } catch (error) {
            result.textContent = "FAIL: range: " + error.message;
            result.setAttribute("aria-label", "FAIL: range");
            console.error(error);
        }
    });
})();
