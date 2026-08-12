import type { JSX } from "solid-js";
import { SidePanelLayout } from "./SidePanel.layout";
import { createSidePanel } from "./sidePanel";

/**
 * The wiring: build the model, hand it to the layout. Generated, once the
 * generator exists.
 *
 * The `const model = ...` line is load-bearing. Calling `createSidePanel()` in
 * the JSX prop position instead type-checks and appears to work, but props are
 * getters: the call would happen under the layout's reactive scope rather than
 * this component's, and anything the model reads later from an event handler
 * would have lost its owner.
 */
export function SidePanel(): JSX.Element {
  const model = createSidePanel();
  return <SidePanelLayout model={model} />;
}
