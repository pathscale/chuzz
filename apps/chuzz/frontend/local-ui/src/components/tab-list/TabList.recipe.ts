import { recipe } from "solid-layouts";

export const tabList = recipe({
  component: "tab-list",
  element: "div",
  slots: {
    root: { base: "tabs" },
    strip: { base: "tab-strip" },
  },
  props: {
    selectedKey: {},
    onSelectionChange: {},
  },
});
