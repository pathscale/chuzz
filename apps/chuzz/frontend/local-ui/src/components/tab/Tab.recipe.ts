import { recipe } from "solid-layouts";

export const tab = recipe({
  component: "tab",
  element: "div",
  slots: {
    root: { base: "tab-shell" },
    tab: { base: "tab" },
    dot: { base: "tab-dot" },
    title: { base: "tab-title" },
    close: { base: "tab-close" },
  },
  props: {
    id: {},
    title: {},
    status: {},
    active: {
      true: { close: "tab-close-visible" },
      false: {},
    },
    closeLabel: {},
    onClose: {},
  },
});
