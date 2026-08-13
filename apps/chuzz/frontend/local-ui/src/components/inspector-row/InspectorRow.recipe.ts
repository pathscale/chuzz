import { recipe } from "solid-layouts";

export const inspectorRow = recipe({
  component: "inspector-row",
  element: "div",
  slots: {
    root: { base: "inspector-row" },
    label: { base: "inspector-row__label" },
    value: { base: "inspector-row__value" },
  },
  props: {
    label: {},
    value: {},
  },
});
