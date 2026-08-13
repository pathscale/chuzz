import { recipe } from "solid-layouts";

export const viewport = recipe({
  component: "viewport",
  element: "div",
  slots: { root: { base: "viewport" } },
});
