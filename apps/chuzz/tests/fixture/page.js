const button = document.createElement("button");
button.setAttribute("aria-label", "Press me");
button.textContent = "Press me";
let presses = 0;
button.addEventListener("click", () => {
  presses += 1;
  button.setAttribute("aria-label", `Pressed ${presses}`);
});
document.getElementById("root").appendChild(button);
const input = document.createElement("input");
input.setAttribute("aria-label", "Fixture value");
input.value = "before";
document.getElementById("root").appendChild(input);

// What the page can see of the web platform, reported into the tree.
//
// This is the reason the headless host is a mode of the browser rather than a
// second one. Every API below is supplied by `document_loader`'s shim, and a
// host built without it reports a fleet site as a blank page: `@solidjs/router`
// destructures `performance.getEntriesByType` during module evaluation, so the
// whole application dies before its first render, with one log line naming the
// entry chunk.
//
// Each is probed the way a page uses it, not by `typeof`. A stub that exists
// and answers nothing passes a typeof check and still breaks the page.
const platform = [];
try {
  if (new URLSearchParams("a=1").get("a") === "1") platform.push("URLSearchParams");
} catch {}
try {
  if (typeof matchMedia("(prefers-color-scheme: dark)").matches === "boolean") {
    platform.push("matchMedia");
  }
} catch {}
try {
  localStorage.setItem("probe", "1");
  if (localStorage.getItem("probe") === "1") platform.push("localStorage");
} catch {}
try {
  sessionStorage.setItem("probe", "1");
  if (sessionStorage.getItem("probe") === "1") platform.push("sessionStorage");
} catch {}
try {
  if (Array.isArray(performance.getEntriesByType("navigation"))) {
    platform.push("getEntriesByType");
  }
} catch {}
for (const name of ["MutationObserver", "ResizeObserver", "IntersectionObserver"]) {
  try {
    new globalThis[name](() => {});
    platform.push(name);
  } catch {}
}

const report = document.createElement("input");
report.setAttribute("aria-label", "Platform report");
report.value = platform.join(" ");
document.getElementById("root").appendChild(report);

globalThis.__mounted = true;
