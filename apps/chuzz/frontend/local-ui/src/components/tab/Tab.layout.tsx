import { Badge, Button, Tabs, Text } from "@pathscale/ui";
import type { Layout } from "solid-layouts";
import { tab } from "./Tab.recipe";

export type TabStatus = "blank" | "loading" | "ready" | "warning" | "error";

export type TabProps = {
  id: string | number;
  title: string;
  status: TabStatus;
  active: boolean;
  closeLabel: string;
  onClose: () => void;
};

/**
 * What each state looks like, in one table.
 *
 * `Badge` rather than `Chip`, and rather than a bare span. A chip is a labelled
 * token and brings a chip's geometry with it: the dot was a 15x22 capsule
 * because `.tab-dot`'s 7px could not outweigh the chip's own padding and line
 * height. A badge is a marker, which is the whole component, so an empty one
 * sized here is a dot, and the flavours it already ships are exactly the five
 * meanings needed, which is why this maps onto the library rather than
 * inventing five colours locally.
 *
 * Matches AgencyZero's tab dot in meaning, not in mechanism: there the colours
 * are Tailwind classes on a span, because that application has no Layout
 * library under it. Here the same five states go through the component that
 * owns markers, so a theme change reaches this the way it reaches everything
 * else.
 */
const FLAVOUR = {
  blank: "neutral",
  loading: "warning",
  ready: "success",
  warning: "info",
  error: "destructive",
} as const;

const Tab: Layout<typeof tab, TabProps> = () => (
  <div {...slot.root}>
    <Tabs.Tab id={local.id} {...slot.tab}>
      <Badge
        variant="solid"
        flavor={FLAVOUR[local.status] ?? FLAVOUR.blank}
        aria-label={local.status}
        {...slot.dot}
      />
      {/* `title` as well as the text, so a name the strip had to cut short can
          still be read without opening the tab. */}
      <Text size="sm" title={local.title} {...slot.title}>
        {local.title}
      </Text>
    </Tabs.Tab>
    <Button
      variant="ghost"
      size="sm"
      width="square"
      title={local.closeLabel}
      aria-label={local.closeLabel}
      onClick={(event) => {
        event.stopPropagation();
        local.onClose();
      }}
      {...slot.close}
    >
      ×
    </Button>
  </div>
);

export const TabLayout = Tab;
export default Tab;
