import { recipe } from "solid-layouts";

export const statusBar = recipe({
  component: "status-bar",
  element: "div",
  slots: {
    root: { base: "status-bar" },
    separatorAfterStatus: { base: "status-separator" },
    separatorAfterTabs: { base: "status-separator" },
    separatorAfterNodes: { base: "status-separator" },
    url: { base: "status-url" },
    accent: { base: "status-accent" },
  },
  props: {
    loading: {},
    loadingLabel: {},
    idleLabel: {},
    url: {},
    tabCount: {},
    tabsLabel: {},
    nodeCount: {},
    nodesLabel: {},
    transferred: {},
  },
});
