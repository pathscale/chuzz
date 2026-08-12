import { recipe } from "solid-layouts";

/**
 * The inspector's design vocabulary.
 *
 * These class names already existed in `styles/chrome.css`; what is new is
 * that the state-to-class mapping is declared here instead of being rebuilt
 * inline at each element. `data-open` used to be written by hand on the
 * section and nowhere else, so a rule could not target the body or the
 * indicator on open-ness without a descendant selector.
 */

export const sidePanel = recipe({
  component: "chrome-side-panel",
  element: "div",
  slots: {
    root: { base: "chrome-side-panel" },
    header: { base: "chrome-side-panel-header" },
    title: { base: "chrome-side-panel-title" },
    scroll: { base: "chrome-side-panel-scroll" },
  },
  state: {
    collapsed: { true: { root: "chrome-side-panel--collapsed" } },
  },
});

export const panelSection = recipe({
  component: "chrome-section",
  element: "div",
  slots: {
    root: { base: "chrome-section" },
    trigger: { base: "chrome-section-trigger" },
    title: { base: "chrome-section-title" },
    indicator: { base: "chrome-section-indicator" },
    body: { base: "chrome-section-body" },
  },
  props: {
    tone: {
      neutral: "",
      primary: { title: "chrome-section-title--primary" },
    },
  },
  state: {
    // Reaches four slots at once. Written inline this was one `data-open` on
    // the root and a ternary on the icon name, with nothing tying them
    // together.
    open: {
      true: {
        root: "chrome-section--open",
        indicator: "chrome-section-indicator--open",
      },
    },
  },
});

export const panelRow = recipe({
  component: "chrome-section-row",
  element: "div",
  slots: {
    root: { base: "chrome-section-row" },
    label: { base: "chrome-section-row-label" },
    value: { base: "chrome-section-row-value" },
  },
});
