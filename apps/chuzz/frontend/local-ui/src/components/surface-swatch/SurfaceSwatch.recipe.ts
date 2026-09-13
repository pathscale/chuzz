import { recipe } from "solid-layouts";

export const surfaceSwatch = recipe({
  component: "surface-swatch",
  element: "span",
  slots: { root: { base: "surface-swatch" } },
  props: {
    controlId: {},
    color: {},
    label: {},
    selected: {},
    x: {},
    y: {},
    onSelect: {},
  },
});
