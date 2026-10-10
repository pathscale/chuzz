(function () {
  function assert(value, label) {
    if (!value) throw new Error(label);
  }

  function throwsName(action, name, label) {
    try {
      action();
    } catch (error) {
      assert(error.name === name, label + ": error name");
      return;
    }
    throw new Error(label + ": no exception");
  }

  document.getElementById("run").addEventListener("click", function () {
    var result = document.getElementById("result");
    var proof = document.getElementById("proof");
    try {
      proof.replaceChildren();

      var el = document.createElement("div");
      var map = el.attributes;
      assert(map === el.attributes && map.length === 0, "attributes stable and initially empty");
      var attribute = document.createAttribute("DATA-STATUS");
      assert(attribute instanceof Attr && attribute.nodeType === 2, "Attr interface and node type");
      assert(attribute.name === "data-status" && attribute.localName === "data-status", "createAttribute HTML name");
      assert(attribute.value === "" && attribute.prefix === null && attribute.namespaceURI === null, "new Attr contents");
      assert(attribute.ownerElement === null && attribute.specified === true, "new Attr ownership");
      attribute.value = "one";
      assert(el.setAttributeNode(attribute) === null, "setAttributeNode initial return");
      assert(attribute.ownerElement === el && el.getAttribute("data-status") === "one", "setAttributeNode attaches value");
      assert(map instanceof NamedNodeMap && map.length === 1, "NamedNodeMap interface and live length");
      assert(map[0] === attribute && map["data-status"] === attribute, "NamedNodeMap indexed and named access");
      assert(map.item(0) === attribute && map.item(99) === null, "NamedNodeMap item");
      assert(map.getNamedItem("data-status") === attribute, "NamedNodeMap getNamedItem identity");
      attribute.value = "two";
      assert(el.getAttribute("data-status") === "two", "Attr value setter writes through");
      el.setAttribute("data-status", "three");
      assert(attribute.value === "three" && el.getAttributeNode("data-status") === attribute, "setAttribute preserves Attr identity");

      var replacement = document.createAttribute("data-status");
      replacement.value = "four";
      assert(map.setNamedItem(replacement) === attribute, "NamedNodeMap replacement returns old Attr");
      assert(attribute.ownerElement === null && attribute.value === "three", "replaced Attr retains old value");
      assert(map[0] === replacement && replacement.ownerElement === el, "replacement identity");
      var other = document.createElement("div");
      throwsName(function () { other.setAttributeNode(replacement); }, "InUseAttributeError", "attached Attr rejected by another element");
      assert(map.removeNamedItem("data-status") === replacement, "removeNamedItem return identity");
      assert(replacement.ownerElement === null && replacement.value === "four" && map.length === 0, "removeNamedItem detaches");
      replacement.value = "detached";
      assert(!el.hasAttributes() && replacement.value === "detached", "detached Attr setter");
      throwsName(function () { map.removeNamedItem("missing"); }, "NotFoundError", "removeNamedItem missing");

      el.id = "reflected-one";
      var idAttribute = el.getAttributeNode("id");
      el.id = "reflected-two";
      assert(idAttribute.value === "reflected-two", "reflected attribute writes update Attr");
      el.removeAttribute("id");
      el.id = "new-id";
      assert(idAttribute.ownerElement === null && idAttribute.value === "reflected-two", "removed Attr remains detached after recreation");
      assert(el.getAttributeNode("id") !== idAttribute, "recreated attribute gets a new identity");

      assert(el.toggleAttribute("hidden") === true && el.hasAttribute("hidden"), "toggleAttribute adds");
      assert(el.toggleAttribute("hidden") === false && !el.hasAttribute("hidden"), "toggleAttribute removes");
      assert(el.toggleAttribute("hidden", true) === true, "toggleAttribute force true");
      assert(el.toggleAttribute("hidden", true) === true && el.getAttribute("hidden") === "", "toggleAttribute forced preservation");
      assert(el.toggleAttribute("hidden", false) === false, "toggleAttribute force false");
      throwsName(function () { document.createAttribute("bad name"); }, "InvalidCharacterError", "createAttribute validation");

      var svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
      svg.setAttributeNS("urn:doma:a", "a:flag", "first");
      svg.setAttributeNS("urn:doma:b", "b:flag", "second");
      var nsAttribute = svg.getAttributeNodeNS("urn:doma:a", "flag");
      assert(nsAttribute.localName === "flag" && nsAttribute.prefix === "a" && nsAttribute.namespaceURI === "urn:doma:a", "namespaced Attr metadata");
      svg.setAttributeNS("urn:doma:a", "c:flag", "changed");
      assert(svg.attributes.length === 2 && svg.getAttributeNS("urn:doma:b", "flag") === "second", "namespaces remain distinct");
      assert(nsAttribute === svg.getAttributeNode("c:flag") && nsAttribute.name === "c:flag" && nsAttribute.value === "changed", "namespace replacement updates prefix and preserves identity");
      assert(svg.getAttribute("a:flag") === null && svg.getAttributeNS("urn:doma:a", "flag") === "changed", "qualified and namespace lookup");
      assert(svg.getAttributeNames().join(",") === "c:flag,b:flag", "getAttributeNames order");
      assert(Array.from(svg.attributes)[0] === nsAttribute && Object.keys(svg.attributes).join(",") === "0,1", "NamedNodeMap iteration and indexed enumeration");
      svg.removeAttributeNS("urn:doma:a", "flag");
      assert(nsAttribute.ownerElement === null && nsAttribute.value === "changed", "removeAttributeNS retained value");
      assert(svg.attributes.length === 1 && svg.hasAttributes(), "removeAttributeNS live map");
      throwsName(function () { svg.setAttributeNS(null, "p:name", "x"); }, "NamespaceError", "namespace validation");
      throwsName(function () { svg.setAttributeNS("urn:doma", "p::name", "x"); }, "InvalidCharacterError", "qualified name validation");

      var fragment = document.createDocumentFragment();
      var a = document.createElement("span");
      var b = document.createElement("b");
      a.id = "fragment-a";
      b.id = "fragment-b";
      fragment.append("leading", a, document.createComment("between"), b, "trailing");
      var liveChildren = fragment.children;
      assert(fragment.nodeType === 11 && fragment.parentNode === null, "real DocumentFragment");
      assert(fragment.firstElementChild === a && fragment.lastElementChild === b && fragment.childElementCount === 2, "fragment element navigation");
      assert(a.nextElementSibling === b && b.previousElementSibling === a, "element siblings skip text and comments");
      assert(fragment.querySelector("#fragment-a") === a && fragment.querySelectorAll("span,b").length === 2, "fragment scoped selectors");
      assert(fragment.getElementById("fragment-b") === b && document.getElementById("fragment-b") === null, "fragment getElementById stays scoped");
      fragment.prepend("prefix", document.createElement("i"));
      assert(liveChildren === fragment.children && liveChildren.length === 3 && liveChildren.item(1) === a, "fragment live children");
      assert(a.getRootNode() === fragment && a.isSameNode(a) && !a.isSameNode(b), "root and node identity");
      assert(a.webkitMatchesSelector("span#fragment-a"), "webkitMatchesSelector alias");

      a.before("before", b);
      assert(a.previousSibling === b && b.previousSibling.nodeValue === "before", "before moves nodes and inserts strings");
      a.after("after", b);
      assert(a.nextSibling.nodeValue === "after" && a.nextSibling.nextSibling === b, "after order and node move");
      a.replaceWith("replacement", a);
      assert(fragment.getElementById("fragment-a") === a && a.previousSibling.nodeValue === "replacement", "replaceWith receiver among arguments");
      b.replaceWith("removed-b");
      assert(b.parentNode === null && fragment.getElementById("fragment-b") === null, "replaceWith detaches receiver");

      var host = document.createElement("div");
      host.appendChild(fragment);
      assert(fragment.childNodes.length === 0 && host.childElementCount === 2, "fragment insertion drains children");
      assert(a.getRootNode() === host, "detached tree root");
      proof.append(host);
      assert(a.getRootNode() === document && a.getRootNode({ composed: true }) === document, "connected root");
      assert(document.firstElementChild === document.documentElement && document.childElementCount === 1, "Document element navigation");

      var adjacent = document.createElement("strong");
      assert(a.insertAdjacentElement("beforebegin", adjacent) === adjacent && adjacent.nextSibling === a, "insertAdjacentElement result and insertion");
      a.insertAdjacentText("afterbegin", "<literal>");
      assert(a.firstChild.nodeValue === "<literal>", "insertAdjacentText preserves literal text");
      a.insertAdjacentText("beforeend", "end");
      assert(a.lastChild.nodeValue === "end", "insertAdjacentText beforeend");
      var unattached = document.createElement("div");
      assert(unattached.insertAdjacentElement("afterend", document.createElement("span")) === null, "adjacent element without parent");
      throwsName(function () { a.insertAdjacentText("invalid", "x"); }, "SyntaxError", "adjacent position validation");

      var original = document.createElement("section");
      original.innerHTML = '<span class="equal">A</span><!--same-->';
      var clone = original.cloneNode(true);
      assert(original.isEqualNode(clone) && !original.isSameNode(clone), "deep structural equality");
      clone.firstElementChild.setAttribute("class", "different");
      assert(!original.isEqualNode(clone), "isEqualNode compares attributes");
      clone = original.cloneNode(true);
      clone.lastChild.data = "different";
      assert(!original.isEqualNode(clone), "isEqualNode compares comments");
      var normal = document.createElement("div");
      var firstText = document.createTextNode("A");
      var secondText = document.createTextNode("B");
      normal.append(document.createTextNode(""), firstText, secondText, document.createTextNode(""));
      normal.normalize();
      assert(normal.childNodes.length === 1 && normal.firstChild === firstText && firstText.data === "AB", "normalize merges text and removes empty nodes");
      assert(secondText.parentNode === null && secondText.data === "B", "normalize preserves removed Text wrapper");

      var namespaces = document.createElement("div");
      namespaces.setAttributeNS("http://www.w3.org/2000/xmlns/", "xmlns:p", "urn:doma:parent");
      var nsChild = document.createElement("span");
      namespaces.append(nsChild);
      assert(nsChild.lookupNamespaceURI("p") === "urn:doma:parent", "lookupNamespaceURI ancestor declaration");
      nsChild.setAttributeNS("http://www.w3.org/2000/xmlns/", "xmlns:p", "urn:doma:child");
      assert(nsChild.lookupNamespaceURI("p") === "urn:doma:child", "lookupNamespaceURI nearest declaration");
      assert(svg.lookupNamespaceURI(null) === "http://www.w3.org/2000/svg", "lookupNamespaceURI element namespace");
      assert(fragment.lookupNamespaceURI("p") === null, "fragment namespace lookup");
      throwsName(function () { host.appendChild(host); }, "HierarchyRequestError", "cycle rejected");
      assert(host.parentNode === proof, "failed cycle preserves tree");

      var parsed = document.getElementById("parsed-template");
      assert(parsed.content.nodeType === 11 && parsed.content !== parsed && parsed.childNodes.length === 0, "parser template content fragment");
      assert(parsed.content.getElementById("parsed-child").textContent === "Parsed template", "parsed template children");
      assert(document.getElementById("parsed-child") === null && parsed.querySelector("span") === null, "template contents excluded from document and element queries");
      assert(parsed.content.querySelector("template").content.firstElementChild.localName === "b", "nested parsed template contents");

      var template = document.createElement("template");
      var content = template.content;
      template.innerHTML = '<span id="template-child">Template</span><template><i>Nested</i></template>';
      assert(template.content === content && content.childElementCount === 2 && template.childElementCount === 0, "template innerHTML targets stable content");
      assert(template.innerHTML === '<span id="template-child">Template</span><template><i>Nested</i></template>', "template serialization includes nested content");
      var deepTemplate = template.cloneNode(true);
      var shallowTemplate = template.cloneNode(false);
      var imported = document.importNode(template, true);
      assert(deepTemplate.content !== content && deepTemplate.content.firstElementChild !== content.firstElementChild, "deep template clone identities");
      assert(deepTemplate.content.firstElementChild.textContent === "Template" && deepTemplate.content.querySelector("template").content.firstElementChild.textContent === "Nested", "deep template clone contents");
      assert(shallowTemplate.content.nodeType === 11 && shallowTemplate.content.childNodes.length === 0, "shallow template clone empty content");
      assert(imported.content.firstElementChild.textContent === "Template" && imported.content !== content, "importNode template contents");
      var retained = content.firstElementChild;
      template.innerHTML = "<b>New template</b>";
      assert(content.firstElementChild.localName === "b" && retained.parentNode === null && retained.textContent === "Template", "template innerHTML retains old wrappers");
      var fragmentClone = content.cloneNode(false);
      assert(fragmentClone.nodeType === 11 && fragmentClone.childNodes.length === 0, "shallow DocumentFragment clone");

      var outerHost = document.createElement("div");
      var old = document.createElement("span");
      old.textContent = "Old";
      outerHost.append(old);
      old.outerHTML = "<b>New</b>tail";
      assert(old.parentNode === null && old.textContent === "Old", "outerHTML preserves detached receiver");
      assert(outerHost.firstElementChild.localName === "b" && outerHost.lastChild.nodeValue === "tail", "outerHTML inserts parsed siblings");
      var table = document.createElement("table");
      table.innerHTML = "<tbody><tr><td>Old cell</td></tr></tbody>";
      table.querySelector("td").outerHTML = "<td>New cell</td><td>Second cell</td>";
      assert(table.querySelectorAll("td").length === 2 && table.querySelector("td").textContent === "New cell", "outerHTML uses table parsing context");
      var fragmentOuter = document.createDocumentFragment();
      var fragmentOld = document.createElement("span");
      fragmentOuter.append(fragmentOld);
      fragmentOld.outerHTML = "<em>Fragment replacement</em>";
      assert(fragmentOuter.firstElementChild.localName === "em", "outerHTML in DocumentFragment");
      throwsName(function () { document.documentElement.outerHTML = "<html></html>"; }, "NoModificationAllowedError", "outerHTML document root protection");

      proof.replaceChildren(document.createTextNode("DOMA tree and attribute checks passed."));
      result.textContent = "READY: doma PASS";
    } catch (error) {
      result.textContent = "FAIL: " + error.message;
    }
  });
}());

