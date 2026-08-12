import type { JSX } from "solid-js";
import { StatusStripLayout } from "./StatusStrip.layout";
import { createStatusStrip } from "./statusStrip";

/** Build the model in the component body, hand it to the layout. Generated,
 * once the generator exists. */
export function StatusStrip(): JSX.Element {
  const model = createStatusStrip();
  return <StatusStripLayout model={model} />;
}
