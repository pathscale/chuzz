import { recipe } from "solid-layouts";

export const surfaceWheel = recipe({
  component: "surface-wheel",
  element: "div",
  slots: { root: { base: "surface-wheel" } },
  props: {
    value: {},
    onChange: {},
    label: {},
  },
});
