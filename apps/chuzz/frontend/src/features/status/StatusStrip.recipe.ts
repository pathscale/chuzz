import { recipe } from "solid-layouts";

/**
 * The status strip's design vocabulary.
 *
 * The classes are the ones already in `styles/chrome.css`. What changes is
 * that `loading` is declared once here instead of being written inline as a
 * `data-loading={... ? "" : undefined}` on the dot and a ternary on the label,
 * with nothing relating the two.
 */
export const statusStrip = recipe({
  component: "chrome-status-strip",
  element: "div",
  slots: {
    root: { base: "chrome-status-strip" },
    dot: { base: "chrome-status-dot" },
    url: { base: "chrome-status-url" },
    spacer: { base: "chrome-status-spacer" },
    accent: { base: "chrome-status-accent" },
  },
  state: {
    // Reaches the dot's class and, because state mirrors to every slot, puts
    // `data-loading` on the root as well. The existing rule
    // `.chrome-status-dot[data-loading]` keeps working unchanged.
    loading: { true: { dot: "chrome-status-dot--loading" } },
  },
});
