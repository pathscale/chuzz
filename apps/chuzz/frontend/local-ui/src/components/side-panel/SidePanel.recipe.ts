import { recipe } from "solid-layouts";

export const sidePanel = recipe({
  component: "side-panel",
  element: "aside",
  slots: {
    root: { base: "side-panel" },
    header: { base: "side-panel-header" },
    title: { base: "side-panel-title" },
    scroll: { base: "side-panel-scroll" },
  },
  props: {
    title: {},
    collapsed: {
      true: "side-panel-collapsed",
      false: "side-panel-expanded",
    },
  },
});
