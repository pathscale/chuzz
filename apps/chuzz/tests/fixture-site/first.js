// Page one writes what page two has to see, then offers a plain anchor to it.
//
// A plain `<a href>`, deliberately: that is what `Button href=` renders across
// the fleet, and it is the activation the router does not intercept.
localStorage.setItem("endpoint", "wss://saved.example");
sessionStorage.setItem("visit", "first");

const link = document.createElement("a");
link.setAttribute("href", "/second.html");
link.setAttribute("aria-label", "Go to second");
link.textContent = "Go to second";
document.getElementById("root").appendChild(link);
