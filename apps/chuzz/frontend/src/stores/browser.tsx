import { createContext, onCleanup, onMount, useContext, type ParentProps } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { api } from "~/api";
import type { PanelSections, PanelState, StatusReadout, Tab, TabId } from "~/types";

interface BrowserState {
  tabs: Tab[];
  activeTabId: TabId;
  panel: PanelState;
  status: StatusReadout;
}

const EMPTY: BrowserState = {
  tabs: [],
  activeTabId: 0,
  panel: {
    collapsed: true,
    sections: { page: true, history: true, network: false, console: false },
  },
  status: { status: "idle", url: "", tabCount: 0, nodeCount: 0, transferred: "0 B" },
};

/**
 * Everything the interface draws, and the only place that talks to the shell.
 *
 * The store is a mirror, not a source: commands go down and events come back
 * up, and the event is what writes. A command that also wrote here would give
 * the window two versions of the truth the moment the shell disagreed, which is
 * exactly what happens when a navigation is refused or a load fails.
 */
function createBrowserStore() {
  const [state, setState] = createStore<BrowserState>({ ...EMPTY });

  onMount(() => {
    const disposers: Array<Promise<() => void>> = [];

    void (async () => {
      const [tabs, activeTabId, panel, status] = await Promise.all([
        api.listTabs(),
        api.activeTabId(),
        api.panelState(),
        api.status(),
      ]);
      setState({ tabs, activeTabId, panel, status });
    })();

    /*
     * `reconcile` on the tab list, so a title arriving for tab 3 does not
     * recreate the DOM for tabs 1 and 2. The list is replaced wholesale by the
     * shell on every change, and without this each replacement would look like
     * an entirely new set of rows.
     */
    disposers.push(api.on("tabs-changed", (tabs) => setState("tabs", reconcile(tabs, { key: "id" }))));
    disposers.push(api.on("active-tab-changed", (id) => setState("activeTabId", id)));
    disposers.push(api.on("status-changed", (status) => setState("status", status)));
    disposers.push(api.on("panel-changed", (panel) => setState("panel", panel)));

    onCleanup(() => {
      for (const pending of disposers) void pending.then((off) => off());
    });
  });

  const activeTab = (): Tab | undefined => state.tabs.find((tab) => tab.id === state.activeTabId);

  return {
    state,
    activeTab,
    selectTab: (id: TabId) => void api.selectTab(id),
    openTab: () => void api.openTab(),
    closeTab: (id: TabId) => void api.closeTab(id),
    /** Resolves false when the shell refused the input. */
    navigate: (input: string) => {
      const id = state.activeTabId;
      return api.navigate(id, input);
    },
    goBack: () => void api.goBack(state.activeTabId),
    goForward: () => void api.goForward(state.activeTabId),
    reload: () => void api.reload(state.activeTabId),
    setPanelCollapsed: (collapsed: boolean) => void api.setPanelCollapsed(collapsed),
    toggleSection: (section: keyof PanelSections) => void api.toggleSection(section),
  };
}

export type BrowserStore = ReturnType<typeof createBrowserStore>;

const BrowserContext = createContext<BrowserStore>();

export function BrowserProvider(props: ParentProps) {
  // Created in the body, not in the JSX below. A store built inside a prop
  // position is a getter: it would run under whichever scope first read it
  // rather than under this component, and anything it registered in
  // `onMount`/`onCleanup` would belong to the wrong owner.
  const store = createBrowserStore();
  return <BrowserContext.Provider value={store}>{props.children}</BrowserContext.Provider>;
}

export function useBrowser(): BrowserStore {
  const store = useContext(BrowserContext);
  if (!store) throw new Error("useBrowser used outside BrowserProvider");
  return store;
}
