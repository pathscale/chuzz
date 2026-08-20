import type { JSX } from "@solidjs/web";
import type { Layout } from "solid-layouts";
import { panelHandle } from "./PanelHandle.recipe";

export type PanelHandleProps = {
  title: string;
  collapsed?: boolean;
  onClick: JSX.EventHandlerUnion<HTMLButtonElement, MouseEvent>;
};

/**
 * The seam between the page and the inspector, and the control that collapses it.
 *
 * It had been reduced to a bare 5px strip: a real hit target, but nothing to
 * look at and no indication of which way it would go. AgencyZero's panels use a
 * round chevron that flips with state, so this restores that affordance on the
 * seam — the strip stays as the grab area, the chevron sits on it.
 */
const PanelHandle: Layout<typeof panelHandle, PanelHandleProps> = () => (
  <button
    type="button"
    {...slot.root}
    title={local.title}
    aria-label={local.title}
    aria-expanded={local.collapsed ? "false" : "true"}
    onClick={local.onClick}
  >
    <span {...slot.chevron} aria-hidden="true">
      {local.collapsed ? "‹" : "›"}
    </span>
  </button>
);

export const PanelHandleLayout = PanelHandle;
export default PanelHandle;
