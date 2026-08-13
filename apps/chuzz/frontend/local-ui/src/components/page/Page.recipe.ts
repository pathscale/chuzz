import { recipe } from "solid-layouts";

export const page = recipe({
  component: "page",
  element: "web-view",
  slots: { root: { base: "page" } },
  props: {
    tabId: {},
    active: {
      true: "page-active",
      false: "page-hidden",
    },
  },
});
