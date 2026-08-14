import { recipe } from "solid-layouts";

export const addressBar = recipe({
  component: "address-bar",
  element: "input",
  slots: { root: { base: "address-bar" } },
  props: {
    id: {},
    value: {},
    invalid: {},
    placeholder: {},
    onInput: {},
  },
});
