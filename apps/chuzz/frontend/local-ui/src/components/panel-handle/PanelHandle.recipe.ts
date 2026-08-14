import { recipe } from "solid-layouts";

export const panelHandle = recipe({
  component: "panel-handle",
  element: "button",
  slots: {
    root: { base: "panel-handle" },
    chevron: { base: "panel-handle-chevron" },
  },
  props: {
    title: {},
    onClick: {},
    collapsed: {
      true: "panel-handle-collapsed",
      false: "panel-handle-expanded",
    },
  },
});
