import { recipe } from "solid-layouts";

export const titleBar = recipe({
  component: "title-bar",
  element: "div",
  slots: { root: { base: "title-bar" } },
});
