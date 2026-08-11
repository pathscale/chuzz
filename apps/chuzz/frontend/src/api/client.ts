import type { PanelState, StatusReadout, Tab, TabId } from "~/types";

/**
 * The shell surface, as the chrome sees it.
 *
 * Commands are `invoke()` calls; {@link BrowserEvent} names are `listen()`
 * topics. The browser mutates its own state without being asked (a load
 * finishes, a page sets its title, a navigation is refused), so **every change
 * arrives as an event**. A command's return value updates the caller
 * optimistically and the event is what keeps the rest of the window honest.
 *
 * Two implementations satisfy this: `./tauri` talks to Rust, `./mock` is an
 * in-memory stand-in that lets the chrome run under `rsbuild dev` with no
 * browser engine behind it. Nothing above this file knows which one it has.
 */
export interface BrowserApi {
  /** Every open tab, in strip order. */
  listTabs(): Promise<Tab[]>;
  /** Which tab is showing. */
  activeTabId(): Promise<TabId>;

  selectTab(id: TabId): Promise<void>;
  /**
   * Opens a tab on the home URL and returns it.
   *
   * The new tab is always selected: a browser that opened a tab you could not
   * see would be indistinguishable from one that did nothing.
   */
  openTab(url?: string): Promise<Tab>;
  /**
   * Closes a tab. The last tab never closes, matching `tab_strip::close_tab`,
   * so this resolves without doing anything rather than rejecting.
   */
  closeTab(id: TabId): Promise<void>;

  /**
   * What a typed string means is Rust's decision, not the chrome's: a URL, a
   * bare hostname, or nothing at all. A string that is neither is **not** a
   * search, and this resolves `false` so the address bar can show it was
   * refused rather than silently doing nothing.
   */
  navigate(id: TabId, input: string): Promise<boolean>;
  goBack(id: TabId): Promise<void>;
  goForward(id: TabId): Promise<void>;
  reload(id: TabId): Promise<void>;

  panelState(): Promise<PanelState>;
  setPanelCollapsed(collapsed: boolean): Promise<void>;
  toggleSection(section: keyof PanelState["sections"]): Promise<void>;

  status(): Promise<StatusReadout>;

  on<K extends keyof BrowserEvents>(
    event: K,
    handler: (payload: BrowserEvents[K]) => void,
  ): Promise<Unlisten>;
}

export type Unlisten = () => void;

/**
 * Events the shell pushes up.
 *
 * `tabs-changed` carries the whole list rather than a delta. The list is small
 * and bounded by how many tabs a person opens; a delta protocol here would be
 * a reconciliation bug waiting to happen for no measurable gain.
 */
export interface BrowserEvents {
  "tabs-changed": Tab[];
  "active-tab-changed": TabId;
  "status-changed": StatusReadout;
  "panel-changed": PanelState;
}

export type BrowserEvent = keyof BrowserEvents;
