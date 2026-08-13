import { Text } from "@pathscale/ui";
import { Show } from "solid-js";
import type { Layout } from "solid-layouts";
import { sidePanel } from "./SidePanel.recipe";

export type SidePanelProps = {
  title: string;
  collapsed: boolean;
};

const SidePanel: Layout<typeof sidePanel, SidePanelProps> = () => (
  <aside {...slot.root}>
    <Show when={!local.collapsed}>
      <div {...slot.header}>
        <Text size="sm" {...slot.title}>
          {local.title}
        </Text>
      </div>
      <div {...slot.scroll}>{children}</div>
    </Show>
  </aside>
);

export const SidePanelLayout = SidePanel;
export default SidePanel;
