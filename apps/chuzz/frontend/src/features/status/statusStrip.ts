import { useBrowser } from "~/stores/browser";

/**
 * The status strip's readout. No JSX, no class names.
 *
 * Node count and transferred bytes are still mock on the Rust side; they are
 * wired through the real readout anyway, so the day they become real is a
 * change in one function rather than in the markup.
 */
export function createStatusStrip() {
  const browser = useBrowser();
  const status = () => browser.state.status;

  return {
    // A `state` key of the recipe, so it reaches the class map by name.
    loading: () => status().status === "loading",
    url: () => status().url,
    tabCount: () => status().tabCount,
    nodeCount: () => status().nodeCount,
    transferred: () => status().transferred,
  };
}

export type StatusStripModel = ReturnType<typeof createStatusStrip>;
