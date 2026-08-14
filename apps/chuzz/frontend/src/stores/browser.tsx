import { createContext, onCleanup, onMount, type ParentProps, useContext } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { api } from "~/api";
import { type BrowserShortcut, resolveBrowserShortcut } from "~/lib/shortcuts";
import { syncDiagnostics } from "~/stores/diagnostics";
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
    const unlisteners: Array<() => void> = [];
    let disposed = false;

    // Tauri installs listeners asynchronously. Mirror AgencyZero's lifecycle
    // handling so an unmount during registration cannot leave a live listener
    // holding this Solid owner after the chrome has gone away.
    const track = (pending: Promise<() => void>) => {
      void pending.then((unlisten) => {
        if (disposed) unlisten();
        else unlisteners.push(unlisten);
      });
    };

    // The diagnostics switches are window-wide rather than per-tab, so they
    // are adopted here alongside the rest of the startup read.
    void syncDiagnostics();

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
    track(api.on("tabs-changed", (tabs) => setState("tabs", reconcile(tabs, { key: "id" }))));
    track(api.on("active-tab-changed", (id) => setState("activeTabId", id)));
    track(api.on("status-changed", (status) => setState("status", status)));
    track(api.on("panel-changed", (panel) => setState("panel", panel)));

    const focusAddress = () => {
      const address = document.getElementById("chuzz-address-bar");
      if (address instanceof HTMLInputElement) {
        // Boa exposes the DOM shape before every method has an implementation.
        // Keep this usable in browser dev builds without calling a non-callable
        // placeholder in the native Blitz document.
        if (typeof address.focus === "function") address.focus();
        if (typeof address.select === "function") address.select();
      }
    };
    const cycleTab = (offset: number) => {
      const index = state.tabs.findIndex((tab) => tab.id === state.activeTabId);
      if (index < 0 || state.tabs.length === 0) return;
      const next = (index + offset + state.tabs.length) % state.tabs.length;
      void api.selectTab(state.tabs[next].id);
    };
    const runShortcut = (shortcut: BrowserShortcut) => {
      switch (shortcut) {
        case "new-tab":
          void api.openTab().then(focusAddress);
          break;
        case "close-tab":
          void api.closeTab(state.activeTabId);
          break;
        case "previous-tab":
          cycleTab(-1);
          break;
        case "next-tab":
          cycleTab(1);
          break;
        case "reload":
          void api.reload(state.activeTabId);
          break;
        case "focus-address":
          focusAddress();
          break;
        case "back":
          void api.goBack(state.activeTabId);
          break;
        case "forward":
          void api.goForward(state.activeTabId);
          break;
      }
    };
    const onKeyDown = (event: KeyboardEvent) => {
      // The PathScale Input delegates its field handler. Keep submission at
      // the shell boundary too, where Blitz already delivers the key reliably
      // for the browser shortcuts. This also makes DOM-control and physical
      // keyboard input take the same route.
      if (event.key === "Enter" || event.code === "Enter") {
        const target = event.target;
        if (target instanceof HTMLInputElement && target.id === "chuzz-address-bar") {
          if (typeof event.preventDefault === "function") event.preventDefault();
          void api.navigate(state.activeTabId, target.value);
          return;
        }
      }
      const shortcut = resolveBrowserShortcut(event);
      if (!shortcut) return;
      if (typeof event.preventDefault === "function") event.preventDefault();
      if (typeof event.stopPropagation === "function") event.stopPropagation();
      runShortcut(shortcut);
    };
    window.addEventListener("keydown", onKeyDown, true);

    onCleanup(() => {
      disposed = true;
      window.removeEventListener("keydown", onKeyDown, true);
      for (const unlisten of unlisteners) unlisten();
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
    setPanelCollapsed: (collapsed: boolean) => {
      // The panel is window-local UI. Apply it immediately, then persist the
      // same state through Rust. Waiting for the event round trip made the
      // handle appear dead whenever event delivery lagged or was unavailable.
      setState("panel", "collapsed", collapsed);
      void api.setPanelCollapsed(collapsed);
    },
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
