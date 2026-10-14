import { Text } from "@pathscale/ui";
import type { JSX } from "@solidjs/web";
import type { Layout } from "solid-layouts";
import { sleepView } from "./SleepView.recipe";

export type SleepViewProps = {
  tabId: string;
  label: string;
  reason: string;
  children: JSX.Element;
};

const SleepView: Layout<typeof sleepView, SleepViewProps> = () => (
  <section {...slot.root} aria-label={local.label}>
    <img {...slot.image} id={`chuzz-saved-${local.tabId}`} alt="Saved view" />
    <div {...slot.caption}>
      <Text>{local.label}</Text>
      <Text size="sm">{local.reason}</Text>
      <div {...slot.actions}>{children}</div>
    </div>
  </section>
);

export const SleepViewLayout = SleepView;
export default SleepView;
