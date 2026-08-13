import { recipe } from "solid-layouts";

export const surfaceSwatch = recipe({
  component: "surface-swatch",
  element: "button",
  slots: { root: { base: "surface-swatch" } },
  props: {
    color: {},
    label: {},
    x: {},
    y: {},
  },
});
