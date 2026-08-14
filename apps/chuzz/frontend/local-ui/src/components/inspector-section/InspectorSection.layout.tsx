import { Chip, Disclosure, Text } from "@pathscale/ui";
import type { JSX } from "solid-js";
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
  <Disclosure id={local.id} isOpen={local.open} onOpenChange={local.onOpenChange} {...slot.root}>
    <Disclosure.Heading>
      <Disclosure.Trigger {...slot.trigger}>
        <Text size="sm" {...slot.title}>
          {local.title}
        </Text>
        <Chip variant="flat" color={local.tone === "primary" ? "primary" : "default"} size="sm">
          {local.count}
        </Chip>
        <Disclosure.Indicator {...slot.indicator} />
      </Disclosure.Trigger>
    </Disclosure.Heading>
    <Disclosure.Content>
      <Disclosure.Body {...slot.body}>{children}</Disclosure.Body>
    </Disclosure.Content>
  </Disclosure>
);

export const InspectorSectionLayout = InspectorSection;
export default InspectorSection;
