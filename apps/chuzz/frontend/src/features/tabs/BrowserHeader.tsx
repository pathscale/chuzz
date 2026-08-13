import type { TabsRootProps } from "@pathscale/ui";
import { Avatar, Tab as BrowserTab, TabList, TitleBar } from "@chuzz/ui";
import { Button, Icon } from "@pathscale/ui";
import { For, type JSX } from "solid-js";
import { useBrowser } from "~/stores/browser";
import { t } from "~/stores/i18n";
import type { Tab } from "~/types";

/**
 * The titlebar: tabs sit in the window's title row, beside the traffic lights.
 *
 * Ported from `tab_strip.rs`. Two behaviours from there are load-bearing and
 * easy to lose: the close affordance is always mounted and only faded, so the
 * pill never resizes under the pointer, and clicking it stops propagation so
 * the click does not also select the tab it is about to remove.
 */
export function BrowserHeader(props: { onOpenSettings: () => void }): JSX.Element {
  const browser = useBrowser();
  const tabsControl: Omit<TabsRootProps, "children"> = {
    get selectedKey() {
      return browser.state.activeTabId;
    },
    onSelectionChange: ((key: string | number) =>
      browser.selectTab(Number(key))) as TabsRootProps["onSelectionChange"],
  };

  return (
    <TitleBar>
      <Button
        variant="ghost"
        size="sm"
        isIconOnly
        title={t("browser.back")}
        onClick={() => browser.goBack()}
      >
        {"‹"}
      </Button>

      {/* Spacing lives in the Layout with the rest of the strip's geometry,
          rather than half here and half there. */}
      <TabList {...tabsControl}>
        {/* Tabs.List currently requires ResizeObserver, which Blitz does not
            expose. The UI tab primitives retain selection, ARIA state, and
            keyboard navigation without its animated measurement layer. */}
        <For each={browser.state.tabs}>
          {(tab) => <TabPill tab={tab} isActive={tab.id === browser.state.activeTabId} />}
        </For>
        <Button
          variant="outline"
          size="sm"
          isIconOnly
          title={t("browser.newTab")}
          onClick={() => browser.openTab()}
        >
          +
        </Button>
      </TabList>

      <Button
        variant="outline"
        size="sm"
        isIconOnly
        title={t("browser.forward")}
        onClick={() => browser.goForward()}
      >
        {"›"}
      </Button>
      <Button
        variant="outline"
        size="sm"
        isIconOnly
        title={t("browser.reload")}
        onClick={() => browser.reload()}
      >
        {"↻"}
      </Button>
      <Button
        variant="outline"
        size="sm"
        isIconOnly
        title={t("browser.settings")}
        onClick={props.onOpenSettings}
      >
        <Icon name="icon-[mdi--cog]" width={15} height={15} />
      </Button>
      <Avatar label="N" />
    </TitleBar>
  );
}

function TabPill(props: { tab: Tab; isActive: boolean }): JSX.Element {
  const browser = useBrowser();

  return (
    <BrowserTab
      id={props.tab.id}
      title={props.tab.title}
      status={props.tab.status}
      active={props.isActive}
      closeLabel={t("browser.closeTab")}
      onClose={() => browser.closeTab(props.tab.id)}
    />
  );
}
