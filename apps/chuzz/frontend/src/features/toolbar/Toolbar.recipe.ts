import { recipe } from "solid-layouts";

/**
 * The toolbar's design vocabulary.
 *
 * `refused` is the interesting one. It was written inline as
 * `data-refused={refused() ? "" : undefined}`, an attribute whose presence
 * carried the meaning. Declared here it also gets a modifier class, and the
 * stylesheet matches `[data-refused="true"]` rather than bare presence, which
 * matters because the recipe reports both states rather than omitting one.
 */
export const toolbar = recipe({
  component: "chrome-toolbar",
  element: "div",
  slots: {
    root: { base: "chrome-toolbar" },
    button: { base: "chrome-tool-button" },
    url: { base: "chrome-url-bar" },
  },
  state: {
    refused: { true: { url: "chrome-url-bar--refused" } },
    // Values the layout reads. Declared so they arrive unwrapped, without
    // parentheses at the call site.
    //
    // Handlers are deliberately absent. A declared state key is treated as an
    // accessor and called, so listing `submit` here invoked a navigation on
    // every render, which is what the stack overflow turned out to be. A model
    // member that is not declared state passes through untouched, which is
    // exactly what a handler needs.
    typed: {},
    canGoBack: {},
    canGoForward: {},
  },
});
