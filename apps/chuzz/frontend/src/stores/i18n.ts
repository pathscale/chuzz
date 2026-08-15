/**
 * Strings, keyed the way AgencyZero keys them.
 *
 * Chuzz ships one language today. This exists so the surfaces lifted from
 * AgencyZero (`ThemePicker` above all) compile unmodified: keeping their
 * `t("appearance.softness")` calls intact is what makes the eventual move into
 * `@pathscale/ui` a lift rather than a rewrite. Adding languages later means
 * replacing this file, not touching the components.
 */

const en = {
  appearance: {
    title: "Appearance",
    hint: "mode, accent, and surface depth",
    mode: "Mode",
    modeHint: "switches the complete interface palette",
    dark: "Dark",
    light: "Light",
    colourStrength: "Colour strength",
    colourStrengthHint: "how far the picked colour reaches into the surfaces",
    softness: "Softness",
    softnessHint: "how far the surfaces move from the palette edge",
    textBrightness: "Text brightness",
    textBrightnessHint: "how far the text rises off the surface",
    surfaceColour: "Surface colour",
    accentColour: "Accent colour",
    accentColourHint: "independent from the workspace surface colour",
    designedYellow: "Designed yellow accent",
    reset: "Reset",
    resetHint: "back to the designed yellow accent and default surface settings",
    resetButton: "Reset to default",
  },
  diagnostics: {
    title: "Diagnostics",
    // Two layers, named for what each one is for rather than for what it is
    // made of. The first is what an agent needs to use the browser; the second
    // is what someone needs to work out why the browser is misbehaving. Both
    // are compiled in and both are off until asked for.
    inspection: "Agent control",
    inspectionHint:
      "read the page and drive the pointer and keyboard, over a local socket only you can open",
    profiling: "Deep debugging",
    profilingHint:
      "screenshots, layout and style snapshots, renderer metrics and intrusive profiling; costs frame time, and needs agent control on to be read",
    on: "On",
    off: "Off",
    locked: "held on by CHUZZ_CONTROL for this run",
  },
  browser: {
    newTab: "New tab",
    closeTab: "Close tab",
    settings: "Settings",
    addressPlaceholder: "Search or enter address",
    inspector: "Inspector",
    showInspector: "Show inspector",
    hideInspector: "Hide inspector",
    page: "Page",
    history: "History",
    network: "Network",
    console: "Console",
  },
} as const;

type Leaves<T, Prefix extends string = ""> = {
  [K in keyof T & string]: T[K] extends string ? `${Prefix}${K}` : Leaves<T[K], `${Prefix}${K}.`>;
}[keyof T & string];

export type UiMessage = Leaves<typeof en>;

/**
 * Dotted lookup, returning the key itself when nothing matches.
 *
 * A missing string should be visible in the window rather than an empty gap:
 * an untranslated label reads as a bug, a blank one reads as a layout problem
 * and gets chased in the wrong file.
 */
export function t(message: UiMessage): string {
  let node: unknown = en;
  for (const part of message.split(".")) {
    if (typeof node !== "object" || node === null) return message;
    node = (node as Record<string, unknown>)[part];
  }
  return typeof node === "string" ? node : message;
}
