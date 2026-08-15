import type { DiagnosticsState, PanelState, StatusReadout, Tab, TabId } from "~/types";
import type { BrowserApi, BrowserEvents, Unlisten } from "./client";

const HOME_URL = "about:blank";

/**
 * An in-memory shell, so the interface runs under `rsbuild dev` with no engine
 * behind it.
 *
 * It is deliberately not a browser: `navigate` accepts anything that parses as
 * a URL or a bare hostname and refuses everything else, which is the one piece
 * of `nav.rs` policy the interface can observe. Everything else here exists to
 * make the layout render with plausible content.
 */
export function createMockApi(): BrowserApi {
  let nextId: TabId = 1;
  let tabs: Tab[] = [
    {
      id: nextId++,
      title: "New Tab",
      url: HOME_URL,
      status: "blank",
      canGoBack: false,
      canGoForward: false,
    },
  ];
  let activeId: TabId = tabs[0].id;
  // Collapsed by default, so the window opens on a full-width page. The
  // inspector is a thing you ask for, not a thing that takes a third of the
  // window before you have looked at anything.
  let panel: PanelState = {
    collapsed: true,
    sections: { page: true, history: true, network: false, console: false, debugging: true },
  };

  // Off by default, as in the real window: the inspection plane lets any local
  // process drive the browser, so it is asked for rather than assumed.
  let diagnostics: DiagnosticsState = { inspection: false, profiling: false, locked: false };

  const handlers: { [K in keyof BrowserEvents]: Set<(payload: BrowserEvents[K]) => void> } = {
    "tabs-changed": new Set(),
    "active-tab-changed": new Set(),
    "status-changed": new Set(),
    "debug-entry": new Set(),
    "panel-changed": new Set(),
  };

  function emit<K extends keyof BrowserEvents>(event: K, payload: BrowserEvents[K]): void {
    for (const handler of handlers[event]) handler(payload);
  }

  function readout(): StatusReadout {
    const active = tabs.find((tab) => tab.id === activeId);
    return {
      status: active?.status ?? "blank",
      url: active?.url ?? HOME_URL,
      tabCount: tabs.length,
      nodeCount: 18,
      transferred: "1.2 kB",
    };
  }

  function announce(): void {
    emit("tabs-changed", tabs);
    emit("status-changed", readout());
  }

  /** The subset of `nav.rs` the interface can see: URL, bare hostname, or nothing. */
  function resolve(input: string): string | null {
    const typed = input.trim();
    if (typed === "") return null;
    if (/^[a-z][a-z0-9+.-]*:/i.test(typed)) return typed;
    if (/^[a-z0-9-]+(\.[a-z0-9-]+)+(\/.*)?$/i.test(typed)) return `https://${typed}`;
    return null;
  }

  function titleFor(url: string): string {
    try {
      return new URL(url).host || "New Tab";
    } catch {
      return "New Tab";
    }
  }

  return {
    listTabs: async () => tabs,
    activeTabId: async () => activeId,

    selectTab: async (id) => {
      if (!tabs.some((tab) => tab.id === id)) return;
      activeId = id;
      emit("active-tab-changed", id);
      emit("status-changed", readout());
    },

    openTab: async (url) => {
      const target = url ?? HOME_URL;
      const tab: Tab = {
        id: nextId++,
        title: titleFor(target),
        url: target,
        status: "blank",
        canGoBack: false,
        canGoForward: false,
      };
      tabs = [...tabs, tab];
      activeId = tab.id;
      announce();
      emit("active-tab-changed", activeId);
      return tab;
    },

    closeTab: async (id) => {
      // Matches `tab_strip::close_tab`: the last tab never closes, and focus
      // falls to the tab on the left.
      if (tabs.length <= 1) return;
      const index = tabs.findIndex((tab) => tab.id === id);
      if (index < 0) return;
      if (id === activeId) {
        activeId = tabs[index === 0 ? 1 : index - 1].id;
        emit("active-tab-changed", activeId);
      }
      tabs = tabs.filter((tab) => tab.id !== id);
      announce();
    },

    navigate: async (id, input) => {
      const url = resolve(input);
      if (url === null) return false;
      tabs = tabs.map((tab) =>
        tab.id === id ? { ...tab, url, title: titleFor(url), canGoBack: true } : tab,
      );
      announce();
      return true;
    },

    goBack: async (id) => {
      tabs = tabs.map((tab) =>
        tab.id === id ? { ...tab, canGoBack: false, canGoForward: true } : tab,
      );
      announce();
    },

    goForward: async (id) => {
      tabs = tabs.map((tab) =>
        tab.id === id ? { ...tab, canGoBack: true, canGoForward: false } : tab,
      );
      announce();
    },

    reload: async () => {
      announce();
    },

    panelState: async () => panel,

    setPanelCollapsed: async (collapsed) => {
      panel = { ...panel, collapsed };
      emit("panel-changed", panel);
    },

    toggleSection: async (section) => {
      panel = { ...panel, sections: { ...panel.sections, [section]: !panel.sections[section] } };
      emit("panel-changed", panel);
    },

    status: async () => readout(),

    // The mock has no engine behind it, so there is nothing to narrate. An
    // empty stream is the honest answer, and the panel renders its own empty
    // state for it.
    debugLog: async () => [],

    // There is no runtime under `rsbuild dev`, so these only remember what
    // they were told. Never `locked`: the environment override is a property
    // of the real process, and pretending otherwise would make the switches
    // untestable in the browser.
    diagnostics: async () => diagnostics,
    setDiagnostics: async (inspection, profiling) => {
      diagnostics = { inspection, profiling: inspection && profiling, locked: false };
      return diagnostics;
    },

    async on<K extends keyof BrowserEvents>(
      event: K,
      handler: (payload: BrowserEvents[K]) => void,
    ): Promise<Unlisten> {
      handlers[event].add(handler);
      return () => handlers[event].delete(handler);
    },
  };
}
