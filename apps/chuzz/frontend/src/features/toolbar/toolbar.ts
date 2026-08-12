import { createEffect, createSignal } from "solid-js";
import { useBrowser } from "~/stores/browser";

/**
 * The address field's behaviour. No JSX, no class names.
 *
 * Keeps the one subtlety the Rust original had: the field tracks the active
 * tab's URL, but only when that URL actually changes. An effect that re-ran on
 * every render would overwrite each keystroke with the current address, and
 * the field could never hold anything typed.
 */
export function createToolbar() {
  const browser = useBrowser();
  const [typed, setTyped] = createSignal("");
  const [refused, setRefused] = createSignal(false);

  createEffect((previous: string | undefined) => {
    const url = browser.activeTab()?.url ?? "";
    if (url !== previous) {
      setTyped(url);
      setRefused(false);
    }
    return url;
  });

  return {
    typed,
    // A `state` key of the recipe, so it reaches the class map by name.
    refused,
    canGoBack: () => Boolean(browser.activeTab()?.canGoBack),
    canGoForward: () => Boolean(browser.activeTab()?.canGoForward),
    goBack: () => browser.goBack(),
    goForward: () => browser.goForward(),
    reload: () => browser.reload(),
    setTyped: (value: string) => {
      setTyped(value);
      setRefused(false);
    },
    submit: async () => {
      // `nav.rs` decides what a typed string means. A value that is neither a
      // URL nor a bare hostname is not a search, and the shell says so rather
      // than navigating somewhere unrelated; showing that beats looking broken.
      const accepted = await browser.navigate(typed());
      setRefused(!accepted);
    },
  };
}

export type ToolbarModel = ReturnType<typeof createToolbar>;
