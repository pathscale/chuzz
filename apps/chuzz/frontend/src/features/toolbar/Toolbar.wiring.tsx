import { defineComponent } from "solid-layouts";
import { ToolbarLayout } from "./Toolbar.layout";
import { toolbar } from "./Toolbar.recipe";
import { createToolbar } from "./toolbar";

/** Generated, once the generator exists. */
export const Toolbar = defineComponent({
  recipe: toolbar,
  name: "Toolbar",
  setup: createToolbar as never,
  layout: ToolbarLayout as never,
});
