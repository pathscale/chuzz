import { Button, Chip, Tabs, Text } from "@pathscale/ui";
import type { Layout } from "solid-layouts";
import { tab } from "./Tab.recipe";

export type TabProps = {
  id: string | number;
  title: string;
  status: string;
  active: boolean;
  closeLabel: string;
  onClose: () => void;
};

const Tab: Layout<typeof tab, TabProps> = () => (
  <div {...slot.root}>
    <Tabs.Tab id={local.id} {...slot.tab}>
      <Chip
        variant="solid"
        flavor={local.status === "loading" ? "primary" : "success"}
        size="sm"
        aria-label={local.status}
        {...slot.dot}
      />
      <Text size="sm" {...slot.title}>
        {local.title}
      </Text>
    </Tabs.Tab>
    <Button
      variant="ghost"
      size="sm"
      width="square" title={local.closeLabel}
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
