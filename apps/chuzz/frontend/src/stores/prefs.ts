import { createStore } from "solid-js/store";
import { applyTheme, DEFAULT_WASH } from "~/lib/theme";
import type { ColorMode, ThemeSettings } from "~/types";

const STORAGE_KEY = "chuzz.prefs";

export const DEFAULT_THEME: ThemeSettings = {
  surface: "",
  accent: "",
  softness: 0,
  wash: DEFAULT_WASH,
  textBrightness: 0,
};

export interface Prefs {
  colorMode: ColorMode;
  theme: ThemeSettings;
}

const DEFAULTS: Prefs = { colorMode: "dark", theme: { ...DEFAULT_THEME } };

/**
 * Read what was stored, field by field.
 *
 * A stored record from an older build is merged over the defaults rather than
 * trusted whole: a missing axis then renders at its default instead of
 * `undefined`, which `applyTheme` would clamp to 0 and quietly flatten the
 * palette. The alternative, versioning the record, buys nothing while the
 * shape is five numbers.
 */
function load(): Prefs {
  if (typeof localStorage === "undefined") return DEFAULTS;
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw === null) return DEFAULTS;
    const stored = JSON.parse(raw) as Partial<Prefs>;
    return {
      colorMode: stored.colorMode === "light" ? "light" : "dark",
      theme: { ...DEFAULT_THEME, ...(stored.theme ?? {}) },
    };
  } catch {
    return DEFAULTS;
  }
}

const [prefs, setPrefs] = createStore<Prefs>(load());

export { prefs };

function persist(): void {
  if (typeof localStorage === "undefined") return;
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify({ ...prefs }));
  } catch {
    // A full or disabled store is not worth failing a render over. The window
    // keeps the setting for this session and forgets it on quit.
  }
}

/**
 * Push the theme at the document and record it.
 *
 * `data-theme` and `data-color-mode` are what `theme.css` selects on;
 * `applyTheme` writes the axes those rules read. Both have to happen, and in
 * this order, or the first paint after a mode switch resolves the new
 * selector against the old axes.
 */
export function syncTheme(): void {
  const root = document.documentElement;
  root.setAttribute("data-theme", "24x-dark");
  root.setAttribute("data-color-mode", prefs.colorMode);
  applyTheme(prefs.theme, root);
}

export function setColorMode(mode: ColorMode): void {
  setPrefs("colorMode", mode);
  syncTheme();
  persist();
}

export function setTheme<K extends keyof ThemeSettings>(key: K, value: ThemeSettings[K]): void {
  setPrefs("theme", key, value);
  syncTheme();
  persist();
}

export function resetTheme(): void {
  setPrefs("theme", { ...DEFAULT_THEME });
  syncTheme();
  persist();
}

/** Whether the theme is untouched, so Reset can disable itself. */
export function isDefaultTheme(): boolean {
  return (
    prefs.theme.surface === DEFAULT_THEME.surface &&
    prefs.theme.accent === DEFAULT_THEME.accent &&
    prefs.theme.softness === DEFAULT_THEME.softness &&
    prefs.theme.wash === DEFAULT_THEME.wash &&
    prefs.theme.textBrightness === DEFAULT_THEME.textBrightness
  );
}
