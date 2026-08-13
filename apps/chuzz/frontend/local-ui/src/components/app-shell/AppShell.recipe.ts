import { recipe } from "solid-layouts";

export const appShell = recipe({
  component: "app-shell",
  element: "div",
  slots: { root: { base: "app-shell" } },
});
