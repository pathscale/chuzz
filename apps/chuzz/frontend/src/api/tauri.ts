import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { BrowserApi, BrowserEvents, Unlisten } from "./client";
import type { PanelState, StatusReadout, Tab, TabId } from "~/types";

/**
 * The real client: `invoke()` for commands, `listen()` for events.
 *
 * Command names are written out below rather than derived from the method name.
 * AgencyZero derives nothing here either, and its `api/index.ts` records why:
 * a name that does not map is not a loud failure, it is a method that quietly
 * does the wrong thing. Keeping the list explicit means a missing command is a
 * compile error in this file instead of a silent no-op at runtime.
 */
export function createTauriApi(): BrowserApi {
  return {
    listTabs: () => invoke<Tab[]>("list_tabs"),
    activeTabId: () => invoke<TabId>("active_tab_id"),

    selectTab: (id) => invoke<void>("select_tab", { id }),
    openTab: (url) => invoke<Tab>("open_tab", { url: url ?? null }),
    closeTab: (id) => invoke<void>("close_tab", { id }),

    navigate: (id, input) => invoke<boolean>("navigate", { id, input }),
    goBack: (id) => invoke<void>("go_back", { id }),
    goForward: (id) => invoke<void>("go_forward", { id }),
    reload: (id) => invoke<void>("reload", { id }),

    panelState: () => invoke<PanelState>("panel_state"),
    setPanelCollapsed: (collapsed) => invoke<void>("set_panel_collapsed", { collapsed }),
    toggleSection: (section) => invoke<void>("toggle_section", { section }),

    status: () => invoke<StatusReadout>("status"),

    async on<K extends keyof BrowserEvents>(
      event: K,
      handler: (payload: BrowserEvents[K]) => void,
    ): Promise<Unlisten> {
      const unlisten = await listen<BrowserEvents[K]>(event, (message) => handler(message.payload));
      return unlisten;
    },
  };
}
