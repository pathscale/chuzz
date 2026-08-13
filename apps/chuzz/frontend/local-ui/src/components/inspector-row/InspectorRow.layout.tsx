import { Text } from "@pathscale/ui";
import type { Layout } from "solid-layouts";
import { inspectorRow } from "./InspectorRow.recipe";

export type InspectorRowProps = {
  label: string;
  value: string;
};

const InspectorRow: Layout<typeof inspectorRow, InspectorRowProps> = () => (
  <div {...slot.root}>
    <Text size="xs" variant="muted" {...slot.label}>
      {local.label}
    </Text>
    <Text size="xs" {...slot.value}>
      {local.value}
    </Text>
  </div>
);

export const InspectorRowLayout = InspectorRow;
export default InspectorRow;
