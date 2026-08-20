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
    alias: [
      { find: /^~\//, replacement: `${resolve(import.meta.dirname, "src")}/` },
      // solid-layouts ships one build arm per Solid major and defaults its
      // bare entry to the 1.9 one, which calls `splitProps`. Solid 2 replaced
      // that with `omit`, and a bundler links both arms of the package's
      // runtime check, so the default entry fails at link time with
      // "export 'splitProps' was not found in 'solid-js'". `./solid-2` is the
      // arm built against this major.
      // Exact matches, so `solid-layouts/solid-2/...` is not rewritten again
      // into itself. Vite treats a string alias as a prefix and has no `$`
      // terminator, which is why these are regular expressions.
      { find: /^solid-layouts$/, replacement: "solid-layouts/solid-2" },
      { find: /^solid-layouts\/recipe$/, replacement: "solid-layouts/solid-2/recipe" },
      { find: /^solid-layouts\/cx$/, replacement: "solid-layouts/solid-2/cx" },
      {
        find: /^solid-layouts\/application-boundary$/,
        replacement: "solid-layouts/solid-2/application-boundary",
      },
    ],
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
      deps: { inline: [/@pathscale\/ui/, /solid-layouts/] },
    },
  },
});
