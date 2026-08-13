import type { JSX } from "solid-js";
import type { Layout } from "solid-layouts";
import { panelHandle } from "./PanelHandle.recipe";

export type PanelHandleProps = JSX.ButtonHTMLAttributes<HTMLButtonElement>;

const PanelHandle: Layout<typeof panelHandle, PanelHandleProps> = () => (
  <button type="button" {...slot.root} />
);

export const PanelHandleLayout = PanelHandle;
export default PanelHandle;
