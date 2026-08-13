import { Button, Icon } from "@pathscale/test-ui";
import { Flex } from "@pathscale/ui";
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
export function TitleBar(props: { onOpenSettings: () => void }): JSX.Element {
  const browser = useBrowser();

  return (
    <Flex as="div" align="center" gap="sm" class="chrome-titlebar">
      <Button
        variant="ghost"
        squareSize={30}
        title={t("chrome.back")}
        onClick={() => browser.goBack()}
      >
        {"‹"}
      </Button>

      {/* Spacing lives in chrome.css with the rest of the strip's geometry,
          rather than half here and half there. */}
      <Flex as="div" align="center" class="chrome-tab-strip">
        <For each={browser.state.tabs}>
          {(tab) => <TabPill tab={tab} isActive={tab.id === browser.state.activeTabId} />}
        </For>
        <Button
          variant="outline"
          squareSize={34}
          title={t("chrome.newTab")}
          onClick={() => browser.openTab()}
        >
          +
        </Button>
      </Flex>

      <Button
        variant="outline"
        squareSize={32}
        title={t("chrome.forward")}
        onClick={() => browser.goForward()}
      >
        {"›"}
      </Button>
      <Button
        variant="outline"
        squareSize={32}
        title={t("chrome.reload")}
        onClick={() => browser.reload()}
      >
        {"↻"}
      </Button>
      <Button
        variant="outline"
        squareSize={32}
        title={t("chrome.settings")}
        onClick={props.onOpenSettings}
      >
        <Icon name="icon-[mdi--cog]" width={15} height={15} />
      </Button>
      <div class="chrome-avatar">N</div>
    </Flex>
  );
}

function TabPill(props: { tab: Tab; isActive: boolean }): JSX.Element {
  const browser = useBrowser();

  return (
    <div
      class="chrome-tab"
      data-active={props.isActive ? "" : undefined}
      onClick={() => browser.selectTab(props.tab.id)}
    >
      <span class="chrome-tab-dot" data-loading={props.tab.status === "loading" ? "" : undefined} />
      <span class="chrome-tab-title">{props.tab.title}</span>
      <Button
        variant="ghost"
        squareSize={18}
        class="chrome-tab-close"
        data-visible={props.isActive ? "" : undefined}
        title={t("chrome.closeTab")}
        onClick={(event) => {
          // Without this the click also selects the tab that is about to be
          // removed, and the strip flickers through a selection nobody asked for.
          event.stopPropagation();
          browser.closeTab(props.tab.id);
        }}
      >
        {"×"}
      </Button>
    </div>
  );
}
