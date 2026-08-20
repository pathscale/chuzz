import { Tab as BrowserTab, TabList, TitleBar } from "@chuzz/ui";
import { Button } from "@pathscale/ui";
import type { JSX } from "@solidjs/web";
import { For } from "solid-js";
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
  const tabsControl: {
    readonly selectedKey: string | number;
    onSelectionChange: (key: string | number) => void;
  } = {
    get selectedKey() {
      return browser.state.activeTabId;
    },
    onSelectionChange: (key) => browser.selectTab(Number(key)),
  };

  return (
    <TitleBar>
      {/* Spacing lives in the Layout with the rest of the strip's geometry,
          rather than half here and half there. */}
      <TabList {...tabsControl}>
        {/* Tabs.List currently requires ResizeObserver, which Blitz does not
            expose. The UI tab primitives retain selection, ARIA state, and
            keyboard navigation without its animated measurement layer. */}
        {/* Wrapped for the same reason as `PageArea`: a Layout resolves its
            children once, so a `For` sitting directly here would be frozen at
            the empty tab list it saw on the first render. */}
        <div class="tab-strip__items">
          <For each={browser.state.tabs}>
            {(tab) => <TabPill tab={tab} isActive={tab.id === browser.state.activeTabId} />}
          </For>
        </div>
        <Button
          variant="outline"
          size="sm"
          width="square"
          title={t("browser.newTab")}
          onClick={() => browser.openTab()}
        >
          +
        </Button>
      </TabList>

      <Button
        variant="outline"
        size="sm"
        width="square"
        title={t("browser.settings")}
        onClick={props.onOpenSettings}
      >
        <CogIcon />
      </Button>
      {/* No avatar. There was one, showing a hardcoded "N": a placeholder for
          an account system this browser does not have, which read as a letter
          nobody could explain sitting next to Settings. It goes back when there
          is an account to put in it. */}
    </TitleBar>
  );
}

/**
 * The settings gear, drawn rather than referenced.
 *
 * It was `<Icon src="icon-[mdi--cog]" />`, which asks `@iconify/tailwind4` to
 * emit a rule carrying the glyph as a mask. That plugin needs the icon set
 * installed to read the glyph from, `@iconify-json/mdi` is not a dependency of
 * this app, and the build says so once as `Invalid icon name` and then carries
 * on. Nothing matching `icon-[` reaches the stylesheet at all, so the button
 * rendered a correctly sized, correctly positioned, entirely empty span. An
 * invisible control reads as a broken one, which is how this arrived as
 * "I can't click settings".
 *
 * Inline, so it needs no icon set, no plugin and no mask support in the engine.
 */
function CogIcon(): JSX.Element {
  return (
    <svg
      viewBox="0 0 24 24"
      width="15"
      height="15"
      fill="currentColor"
      aria-hidden="true"
      xmlns="http://www.w3.org/2000/svg"
    >
      <path d="M12 15.5A3.5 3.5 0 0 1 8.5 12A3.5 3.5 0 0 1 12 8.5a3.5 3.5 0 0 1 3.5 3.5a3.5 3.5 0 0 1-3.5 3.5m7.43-2.53c.04-.32.07-.64.07-.97s-.03-.66-.07-1l2.11-1.63c.19-.15.24-.42.12-.64l-2-3.46c-.12-.22-.39-.31-.61-.22l-2.49 1c-.52-.39-1.06-.73-1.69-.98l-.37-2.65A.506.506 0 0 0 14 2h-4c-.25 0-.46.18-.5.42l-.37 2.65c-.63.25-1.17.59-1.69.98l-2.49-1c-.22-.09-.49 0-.61.22l-2 3.46c-.13.22-.07.49.12.64L4.57 11c-.04.34-.07.67-.07 1s.03.65.07.97l-2.11 1.66c-.19.15-.25.42-.12.64l2 3.46c.12.22.39.3.61.22l2.49-1.01c.52.4 1.06.74 1.69.99l.37 2.65c.04.24.25.42.5.42h4c.25 0 .46-.18.5-.42l.37-2.65c.63-.26 1.17-.59 1.69-.99l2.49 1.01c.22.08.49 0 .61-.22l2-3.46c.12-.22.07-.49-.12-.64z" />
    </svg>
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
