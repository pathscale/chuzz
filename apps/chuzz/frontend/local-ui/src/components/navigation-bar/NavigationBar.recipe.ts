import { recipe } from "solid-layouts";

export const navigationBar = recipe({
  component: "navigation-bar",
  element: "div",
  slots: { root: { base: "navigation-bar" } },
});
