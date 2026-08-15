import { recipe } from "solid-layouts";

/**
 * A tab in the strip.
 *
 * `status` is a presentational axis rather than something the layout branches
 * on, so the five states are declared here and the layout stays one shape. It
 * carries no colour of its own: the dot is a `Badge` and the colour arrives as
 * its `flavor`, chosen in the layout from this same value. Putting the mapping
 * in one place is what stops a sixth state being added to one of them.
 */
export const tab = recipe({
  component: "tab",
  element: "div",
  slots: {
    root: { base: "tab-shell" },
    tab: { base: "tab" },
    dot: { base: "tab-dot" },
    title: { base: "tab-title" },
    close: { base: "tab-close" },
  },
  props: {
    id: {},
    title: {},
    status: {
      blank: { dot: "tab-dot-blank" },
      loading: { dot: "tab-dot-loading" },
      ready: { dot: "tab-dot-ready" },
      warning: { dot: "tab-dot-warning" },
      error: { dot: "tab-dot-error" },
    },
    active: {
      true: { close: "tab-close-visible" },
      false: {},
    },
    closeLabel: {},
    onClose: {},
  },
});
