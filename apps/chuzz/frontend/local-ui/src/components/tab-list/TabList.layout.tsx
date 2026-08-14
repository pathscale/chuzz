import { Tabs } from "@pathscale/ui";
import type { TabsRootProps } from "@pathscale/ui";
import type { JSX } from "solid-js";
import type { Layout } from "solid-layouts";
import { tabList } from "./TabList.recipe";

export type TabListProps = {
  children: JSX.Element;
  selectedKey: string | number;
  onSelectionChange: (key: string | number) => void;
};

const TabList: Layout<typeof tabList, TabListProps> = () => (
  <div {...slot.root}>
    <Tabs
      selectedKey={local.selectedKey}
      onSelectionChange={
        local.onSelectionChange as unknown as TabsRootProps["onSelectionChange"]
      }
    >
      <div role="tablist" {...slot.strip}>
        {children}
      </div>
    </Tabs>
  </div>
);

export const TabListLayout = TabList;
export default TabList;
