import { Tabs } from "@pathscale/ui";
import type { Layout } from "solid-layouts";
import { tabList } from "./TabList.recipe";

export type TabListProps = {
  selectedKey: string | number;
  onSelectionChange: (key: string | number) => void;
};

const TabList: Layout<typeof tabList, TabListProps> = () => (
  <Tabs
    selectedKey={local.selectedKey}
    onSelectionChange={local.onSelectionChange}
    {...slot.root}
  >
    <div role="tablist" {...slot.strip}>
      {children}
    </div>
  </Tabs>
);

export const TabListLayout = TabList;
export default TabList;
