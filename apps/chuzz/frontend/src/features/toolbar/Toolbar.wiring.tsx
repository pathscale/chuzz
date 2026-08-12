import type { JSX } from "solid-js";
import { ToolbarLayout } from "./Toolbar.layout";
import { createToolbar } from "./toolbar";

/** Build the model in the component body, hand it to the layout. Generated,
 * once the generator exists. */
export function Toolbar(): JSX.Element {
  const model = createToolbar();
  return <ToolbarLayout model={model} />;
}
