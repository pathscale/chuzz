import { recipe } from "solid-layouts";

export const settingsDialog = recipe({
  component: "settings-dialog",
  element: "div",
  slots: {
    root: { base: "" },
    content: { base: "settings-dialog" },
    header: { base: "settings-header" },
    title: { base: "settings-title" },
    body: { base: "settings-body" },
    row: { base: "settings-row" },
    label: { base: "settings-label" },
    modeGroup: { base: "settings-mode" },
  },
  props: {
    open: {},
    onClose: {},
    title: {},
    modeLabel: {},
    darkLabel: {},
    lightLabel: {},
    mode: {},
    onModeChange: {},
  },
});
