import type {
  DebugEntry,
  DiagnosticsState,
  PanelState,
  StatusReadout,
  Tab,
  TabId,
  UserAgentState,
} from "~/types";
import type { BrowserApi, BrowserEvents, Unlisten } from "./client";

interface IzumoEvent<T> {
  event: string;
  id: number;
  payload: T;
}

interface IzumoBridge {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  listen<T>(
    event: string,
    handler: (message: IzumoEvent<T>) => void,
  ): Promise<Unlisten>;
}

declare global {
  interface Window {
    __IZUMO__?: IzumoBridge;
  }
}

/**
 * The real client calls the invoke/listen bridge injected by izumo.
 *
 * Command names stay explicit so a missing or renamed Rust command is visible
 * here instead of silently falling back to the mock implementation.
 */
export function createIzumoApi(): BrowserApi {
  const izumo = window.__IZUMO__;
  if (!izumo) {
    throw new Error("The izumo IPC bridge is unavailable");
  }

  return {
    listTabs: () => izumo.invoke<Tab[]>("list_tabs"),
    activeTabId: () => izumo.invoke<TabId>("active_tab_id"),

    selectTab: (id) => izumo.invoke<void>("select_tab", { id }),
    openTab: (url) => izumo.invoke<Tab>("open_tab", { url: url ?? null }),
    closeTab: (id) => izumo.invoke<void>("close_tab", { id }),

    navigate: (id, input) => izumo.invoke<boolean>("navigate", { id, input }),
    goBack: (id) => izumo.invoke<void>("go_back", { id }),
    goForward: (id) => izumo.invoke<void>("go_forward", { id }),
    reload: (id) => izumo.invoke<void>("reload", { id }),

    panelState: () => izumo.invoke<PanelState>("panel_state"),
    setPanelCollapsed: (collapsed) =>
      izumo.invoke<void>("set_panel_collapsed", { collapsed }),
    toggleSection: (section) => izumo.invoke<void>("toggle_section", { section }),

    status: () => izumo.invoke<StatusReadout>("status"),

    userAgent: () => izumo.invoke<UserAgentState>("user_agent"),
    setUserAgentSpoofing: (spoofing) =>
      izumo.invoke<UserAgentState>("set_user_agent_spoofing", { spoofing }),

    debugLog: (since) => izumo.invoke<DebugEntry[]>("debug_log", { since: since ?? null }),

    diagnostics: () => izumo.invoke<DiagnosticsState>("diagnostics"),
    setDiagnostics: (inspection, profiling) =>
      izumo.invoke<DiagnosticsState>("set_diagnostics", { inspection, profiling }),

    async on<K extends keyof BrowserEvents>(
      event: K,
      handler: (payload: BrowserEvents[K]) => void,
    ): Promise<Unlisten> {
      return izumo.listen<BrowserEvents[K]>(event, (message) => handler(message.payload));
    },
  };
}

