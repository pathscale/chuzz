import { resolve } from "node:path";
import { defineConfig } from "vitest/config";
import solid from "vite-plugin-solid";

/**
 * There was no config, so `vitest` ran in Node with no JSX transform: the only
 * tests that could exist were pure functions. Every defect in the tab strip was
 * therefore untestable by construction, which is a large part of why seven of
 * them shipped together.
 *
 * `vite-plugin-solid` and `jsdom` were already dev dependencies. Nothing here
 * is new to the project; it is the wiring that was missing.
 */
export default defineConfig({
  plugins: [solid()],
  resolve: {
    alias: { "~": resolve(import.meta.dirname, "src") },
    // The same condition the app builds with. Without it the Solid runtime
    // resolves to its server build and every render returns a string.
    conditions: ["development", "browser"],
  },
  test: {
    environment: "jsdom",
    setupFiles: ["src/test-setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
    server: {
      // `@pathscale/ui` ships a `.css` import beside each component, which
      // Node cannot load and Vite can. Inlining the dependency puts it through
      // the transform pipeline instead of `import`.
      deps: { inline: [/@pathscale\/ui/] },
    },
  },
});
