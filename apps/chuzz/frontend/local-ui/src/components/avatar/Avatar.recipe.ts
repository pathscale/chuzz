import { recipe } from "solid-layouts";

export const avatar = recipe({
  component: "avatar",
  element: "span",
  slots: { root: { base: "avatar" } },
  props: { label: {} },
});
