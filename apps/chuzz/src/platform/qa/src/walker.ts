(() => {
    "use strict";

    const check = (condition, message) => {
        if (!condition) throw new Error(message);
    };
    const fixture = document.getElementById("fixture");
    const result = document.getElementById("result");

    function run() {
        fixture.innerHTML =
            '<section id="walk-root"><div id="skip"><span id="first">First</span></div>'
            + '<div id="reject"><span id="hidden">Hidden</span></div>'
            + '<span id="last">Last</span></section>';
        const root = fixture.firstChild;
        const filter = node => {
            if (node.id === "skip") return NodeFilter.FILTER_SKIP;
            if (node.id === "reject") return NodeFilter.FILTER_REJECT;
            return NodeFilter.FILTER_ACCEPT;
        };
        const walker = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT, filter);
        check(walker instanceof TreeWalker, "TreeWalker interface identity");
        check(walker.root === root && walker.filter === filter, "Walker properties");
        check(walker.whatToShow === NodeFilter.SHOW_ELEMENT, "Walker mask");
        check(walker.currentNode === root, "Initial walker cursor");
        check(walker.nextNode().id === "first", "FILTER_SKIP did not preserve descendants");
        check(walker.nextNode().id === "last", "FILTER_REJECT did not prune descendants");
        check(walker.nextNode() === null && walker.currentNode.id === "last", "End cursor changed");
        check(walker.previousNode().id === "first", "Reverse traversal entered a rejected subtree");
        walker.currentNode = root;
        check(walker.firstChild().id === "first", "Filtered firstChild");
        check(walker.nextSibling().id === "last", "Filtered sibling navigation");
        check(walker.previousSibling().id === "first", "Filtered previousSibling");
        check(walker.parentNode() === root, "Filtered parentNode");
        check(walker.parentNode() === null && walker.currentNode === root, "Root boundary");
        check(walker.lastChild().id === "last", "Filtered lastChild");
        check(walker.previousNode().id === "first", "Reverse traversal after lastChild");
        check(walker.previousNode() === root, "Reverse traversal omitted root");
        check(walker.previousNode() === null, "Reverse traversal escaped root");

        const iterator = document.createNodeIterator(root, NodeFilter.SHOW_ELEMENT,
            node => node.id === "reject" ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT);
        check(iterator instanceof NodeIterator, "NodeIterator interface identity");
        check(iterator.referenceNode === root && iterator.pointerBeforeReferenceNode,
            "Initial iterator position");
        const visited = [];
        let node;
        while ((node = iterator.nextNode())) visited.push(node.id);
        check(visited.includes("hidden"), "Iterator incorrectly pruned FILTER_REJECT descendants");
        check(iterator.referenceNode.id === "last" && !iterator.pointerBeforeReferenceNode,
            "Iterator end changed position");
        check(iterator.previousNode().id === "last" && iterator.pointerBeforeReferenceNode,
            "Iterator reverse pointer");
        check(iterator.nextNode().id === "last", "Iterator direction switch");
        iterator.detach();
        check(iterator.previousNode().id === "last", "detach disabled traversal");

        let objectCalls = 0;
        const objectFilter = {
            acceptNode(node) {
                check(this === objectFilter, "acceptNode receiver");
                objectCalls++;
                return NodeFilter.FILTER_ACCEPT;
            }
        };
        const textWalker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, objectFilter);
        check(textWalker.nextNode().nodeValue === "First", "Text mask or object filter");
        check(textWalker.nextNode().nodeValue === "Hidden", "Text traversal skipped descendants");
        check(textWalker.nextNode().nodeValue === "Last", "Text traversal order");
        check(textWalker.nextNode() === null && objectCalls === 3, "Masked nodes reached filter");

        const documentWalker = document.createTreeWalker(document, NodeFilter.SHOW_ALL, null, false);
        check(documentWalker.currentNode === document && documentWalker.nextNode() !== null,
            "webcomponents document walker signature");

        let recursive;
        let recursiveError = false;
        recursive = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT, () => {
            try {
                recursive.nextNode();
            } catch (error) {
                recursiveError = error.name === "InvalidStateError";
            }
            return NodeFilter.FILTER_ACCEPT;
        });
        check(recursive.nextNode().id === "skip" && recursiveError, "Recursive filter guard");

        let shouldThrow = true;
        const throwing = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT, () => {
            if (shouldThrow) throw new Error("filter exception");
            return NodeFilter.FILTER_ACCEPT;
        });
        let propagated = false;
        try {
            throwing.nextNode();
        } catch (error) {
            propagated = error.message === "filter exception";
        }
        check(propagated && throwing.currentNode === root, "Filter exception changed cursor");
        shouldThrow = false;
        check(throwing.nextNode().id === "skip", "Filter active flag was not cleared");

        const mutable = document.createNodeIterator(root, NodeFilter.SHOW_ELEMENT);
        check(mutable.nextNode() === root, "Iterator omitted root");
        const removed = mutable.nextNode();
        removed.remove();
        check(mutable.referenceNode === root && !mutable.pointerBeforeReferenceNode,
            "Iterator predecessor adjustment");
        check(mutable.nextNode().id === "reject", "Iterator removal adjustment");

        const before = document.createNodeIterator(root, NodeFilter.SHOW_ELEMENT);
        before.nextNode();
        const rejected = before.nextNode();
        before.previousNode();
        check(before.pointerBeforeReferenceNode, "Iterator before pointer setup");
        rejected.remove();
        check(before.referenceNode.id === "last" && before.pointerBeforeReferenceNode,
            "Iterator successor adjustment");
        check(before.nextNode().id === "last", "Iterator skipped adjusted successor");

        const tail = document.createNodeIterator(root, NodeFilter.SHOW_ELEMENT);
        tail.nextNode();
        const last = tail.nextNode();
        tail.previousNode();
        last.remove();
        check(tail.referenceNode === root && !tail.pointerBeforeReferenceNode,
            "Iterator before-pointer fallback");
        check(tail.nextNode() === null, "Iterator entered removed tail");

        const live = document.createNodeIterator(root, NodeFilter.SHOW_ELEMENT);
        live.nextNode();
        const added = document.createElement("span");
        added.id = "added";
        root.appendChild(added);
        check(live.nextNode() === added, "Iterator did not observe insertion");

        const replacement = document.createNodeIterator(root, NodeFilter.SHOW_ELEMENT);
        replacement.nextNode();
        replacement.nextNode();
        root.innerHTML = '<b id="replacement">Replacement</b>';
        check(replacement.referenceNode === root, "innerHTML did not adjust iterator");
        check(replacement.nextNode().id === "replacement", "innerHTML live traversal");
        root.textContent = "Replacement text";
        check(replacement.referenceNode === root, "textContent did not adjust iterator");

        const moveRoot = document.createElement("div");
        moveRoot.innerHTML = '<b id="move-a">A</b><b id="move-b">B</b>';
        fixture.appendChild(moveRoot);
        const moving = document.createNodeIterator(moveRoot, NodeFilter.SHOW_ELEMENT);
        moving.nextNode();
        const moved = moving.nextNode();
        moveRoot.appendChild(moved);
        check(moving.referenceNode === moveRoot, "Move did not run pre-removing steps");
        check(moving.nextNode().id === "move-b", "Move traversal order");
        check(moving.nextNode() === moved, "Moved node was not revisited");

        const detached = document.createNodeIterator(moveRoot, NodeFilter.SHOW_ELEMENT);
        detached.nextNode();
        moveRoot.remove();
        check(detached.referenceNode === moveRoot, "Removing iterator root changed reference");
        check(detached.nextNode().id === "move-b", "Detached-root traversal");

        const heldWalker = document.createTreeWalker(moveRoot, NodeFilter.SHOW_ELEMENT);
        const held = heldWalker.nextNode();
        held.remove();
        check(heldWalker.currentNode === held, "Removal relocated TreeWalker currentNode");

        const output = document.createElement("p");
        output.textContent = "Traversal rendered";
        fixture.appendChild(output);
    }

    document.getElementById("run").addEventListener("click", () => {
        try {
            run();
            result.textContent = "PASS: walker";
        } catch (error) {
            result.textContent = "FAIL: walker: " + error.message;
            console.error(error);
        }
    });
})();
