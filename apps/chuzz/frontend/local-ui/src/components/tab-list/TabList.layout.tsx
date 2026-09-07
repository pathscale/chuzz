import { Tabs } from "@pathscale/ui";
import type { JSX } from "@solidjs/web";
import type { Layout } from "solid-layouts";
import { tabList } from "./TabList.recipe";

export type TabListProps = {
  children: JSX.Element;
  selectedKey: string | number;
  onSelectionChange: (key: string | number) => void;
};

/**
 * The cast on `onSelectionChange` is gone.
 *
 * `Tabs` used to declare the callback against its own key type, so handing it
 * a `(key: string | number) => void` needed `as unknown as` to get through.
 * Its key is `string | number` now, which is what this Layout already accepts,
 * so the two agree and the assignment stands on its own. Keeping the cast
 * would keep the one place in this file where a future change to either side
 * could disagree without the typecheck saying so.
 */
const TabList: Layout<typeof tabList, TabListProps> = () => (
  <div {...slot.root}>
    <Tabs selectedKey={local.selectedKey} onSelectionChange={local.onSelectionChange}>
      <div role="tablist" {...slot.strip}>
        {children}
      </div>
    </Tabs>
  </div>
);

export const TabListLayout = TabList;
export default TabList;
