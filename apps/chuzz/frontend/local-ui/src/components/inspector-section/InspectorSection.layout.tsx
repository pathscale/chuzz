import { Chip, Collapsible, Text } from "@pathscale/ui";
import type { JSX } from "@solidjs/web";
import type { Layout } from "solid-layouts";
import { inspectorSection } from "./InspectorSection.recipe";

export type InspectorSectionProps = {
  children: JSX.Element;
  id: string;
  title: string;
  count: number;
  tone: "primary" | "neutral";
  open: boolean;
  onOpenChange: (open: boolean) => void;
};

const InspectorSection: Layout<typeof inspectorSection, InspectorSectionProps> = () => (
  <Collapsible id={local.id} open={local.open} onOpenChange={local.onOpenChange} {...slot.root}>
    <Collapsible.Heading>
      <Collapsible.Trigger {...slot.trigger}>
        <Text size="sm" {...slot.title}>
          {local.title}
        </Text>
        <Chip variant="flat" flavor={local.tone === "primary" ? "primary" : "neutral"} size="sm">
          {local.count}
        </Chip>
        <Collapsible.Indicator {...slot.indicator} />
      </Collapsible.Trigger>
    </Collapsible.Heading>
    <Collapsible.Content>
      <Collapsible.Body {...slot.body}>{children}</Collapsible.Body>
    </Collapsible.Content>
  </Collapsible>
);

export const InspectorSectionLayout = InspectorSection;
export default InspectorSection;
