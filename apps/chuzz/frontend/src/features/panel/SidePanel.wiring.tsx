import { defineComponent } from "solid-layouts";
import { SidePanelLayout } from "./SidePanel.layout";
import { sidePanel } from "./SidePanel.recipe";
import { createSidePanel } from "./sidePanel";

/** Generated, once the generator exists. */
export const SidePanel = defineComponent({
  recipe: sidePanel,
  name: "SidePanel",
  setup: createSidePanel as never,
  layout: SidePanelLayout as never,
});
