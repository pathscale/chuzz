import { Chip, Separator, Text } from "@pathscale/ui";
import type { Layout } from "solid-layouts";
import { statusBar } from "./StatusBar.recipe";

export type StatusBarProps = {
  loading: boolean;
  loadingLabel: string;
  idleLabel: string;
  url: string;
  tabCount: number;
  tabsLabel: string;
  nodeCount: number;
  nodesLabel: string;
  transferred: string;
};

const StatusBar: Layout<typeof statusBar, StatusBarProps> = () => (
  <div {...slot.root}>
    <Chip variant="flat" color={local.loading ? "primary" : "success"} size="sm">
      {local.loading ? local.loadingLabel : local.idleLabel}
    </Chip>
    <Separator orientation="vertical" {...slot.separatorAfterStatus} />
    <Text size="xs" variant="muted" {...slot.url}>
      {local.url}
    </Text>
    <span aria-hidden="true" />
    <Text size="xs" variant="muted">
      {local.tabCount} {local.tabsLabel}
    </Text>
    <Separator orientation="vertical" {...slot.separatorAfterTabs} />
    <Text size="xs" variant="muted">
      {local.nodeCount} {local.nodesLabel}
    </Text>
    <Separator orientation="vertical" {...slot.separatorAfterNodes} />
    <Text size="xs" {...slot.accent}>
      {local.transferred}
    </Text>
  </div>
);

export const StatusBarLayout = StatusBar;
export default StatusBar;
