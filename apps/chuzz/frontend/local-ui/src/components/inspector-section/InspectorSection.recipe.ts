import { recipe } from "solid-layouts";

export const inspectorSection = recipe({
  component: "inspector-section",
  element: "div",
  slots: {
    root: { base: "inspector-section" },
    trigger: { base: "inspector-trigger" },
    title: { base: "inspector-title" },
    indicator: { base: "inspector-indicator" },
    body: { base: "inspector-body" },
  },
  props: {
    id: {},
    title: {},
    count: {},
    tone: {},
    open: {},
    onOpenChange: {},
  },
});
