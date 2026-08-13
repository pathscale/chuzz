import { recipe } from "solid-layouts";

export const panelHandle = recipe({
  component: "panel-handle",
  element: "button",
  slots: { root: { base: "panel-handle" } },
});
