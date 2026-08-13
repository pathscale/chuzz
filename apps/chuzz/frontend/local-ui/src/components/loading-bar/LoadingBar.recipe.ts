import { recipe } from "solid-layouts";

export const loadingBar = recipe({
  component: "loading-bar",
  element: "div",
  slots: { root: { base: "loading-bar" } },
  props: {
    loading: {
      true: "loading-bar-visible",
      false: "loading-bar-hidden",
    },
    label: {},
  },
});
