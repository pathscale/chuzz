import { defineComponent } from "solid-layouts";
import { StatusStripLayout } from "./StatusStrip.layout";
import { statusStrip } from "./StatusStrip.recipe";
import { createStatusStrip } from "./statusStrip";

/** Generated, once the generator exists. */
export const StatusStrip = defineComponent({
  recipe: statusStrip,
  name: "StatusStrip",
  setup: createStatusStrip as never,
  layout: StatusStripLayout as never,
});
