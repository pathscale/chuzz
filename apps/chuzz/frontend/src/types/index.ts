/**
 * The browser's view of itself.
 *
 * These mirror the Rust types the shell exposes over IPC. Browser policy
 * (what a typed string means, what the back stack looks like, when a load is
 * abandoned) stays in Rust; this is only what the chrome has to draw.
 */

export type TabId = number;

export type LoadStatus = "idle" | "loading";

/**
 * A tab as the chrome sees it.
 *
 * `title` is already resolved by the Rust side: it falls back to the host, then
 * to "New Tab", so the chrome never has to decide what an untitled page is
 * called. Two places deciding that is how a tab strip and a window title end up
 * disagreeing.
 */
export interface Tab {
  id: TabId;
  title: string;
  url: string;
  status: LoadStatus;
  canGoBack: boolean;
  canGoForward: boolean;
}

/** Which inspector sections are open. Persisted by the shell, not the chrome. */
export interface PanelSections {
  page: boolean;
  history: boolean;
  network: boolean;
  console: boolean;
}

export interface PanelState {
  collapsed: boolean;
  sections: PanelSections;
}

/** One row in an inspector section. */
export interface InspectorRow {
  label: string;
  value: string;
}

export interface InspectorSection {
  key: keyof PanelSections;
  title: string;
  count: number;
  tone: "primary" | "neutral";
  rows: InspectorRow[];
}

export type ColorMode = "light" | "dark";

/**
 * The theme axes, lifted from AgencyZero unchanged.
 *
 * Kept identical on purpose: `lib/theme.ts` and `ThemePicker` were taken
 * wholesale, and a divergence here is what would make moving all three into
 * `@pathscale/ui` later a merge instead of a move.
 */
export interface ThemeSettings {
  /** Colour washed into the workspace surfaces. Empty keeps them neutral. */
  surface: string;
  /**
   * Accent as `#rrggbb`. Empty means the palette's own yellow — deliberately
   * not the literal, so the record cannot drift from the stylesheet.
   */
  accent: string;
  /**
   * Lightness added to every surface in oklch points, and taken off every text
   * rung. One number: lifting the desk without bringing the text down trades
   * one glare for another. 0 is the palette as designed.
   */
  softness: number;
  /**
   * How much of the surface colour is mixed into every surface, as a percentage.
   * Ignored while `surface` is empty.
   */
  wash: number;
  /**
   * Lightness added back to every text rung, in oklch points. Softness dims the
   * text as it lifts the surfaces; this is the counterweight, so "less glare"
   * and "less faded" stop being one number. Negative dims further.
   */
  textBrightness: number;
}

/** The bottom strip's counters. Everything but the URL is still mock in Rust. */
export interface StatusReadout {
  status: LoadStatus;
  url: string;
  tabCount: number;
  nodeCount: number;
  transferred: string;
}
