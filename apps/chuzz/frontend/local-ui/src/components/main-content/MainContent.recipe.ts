import { recipe } from "solid-layouts";

export const mainContent = recipe({
  component: "main-content",
  element: "div",
  slots: { root: { base: "main-content" } },
});
