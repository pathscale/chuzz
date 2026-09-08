// Read during boot, the way an application reads its settings, rather than
// after a click. Seeding a store a moment later would be the same as not
// seeding it.
const out = document.createElement("input");
out.setAttribute("aria-label", "carried");
out.value = [
  "local=" + String(localStorage.getItem("endpoint")),
  "session=" + String(sessionStorage.getItem("visit"))
].join(" ");
document.getElementById("root").appendChild(out);
