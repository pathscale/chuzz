import { createRequire } from "node:module";
import { defineConfig } from "@rsbuild/core";
import { pluginBabel } from "@rsbuild/plugin-babel";
import ForkTsCheckerWebpackPlugin from "fork-ts-checker-webpack-plugin";
import { pluginSolidLayoutsApplication } from "rsbuild-plugin-solid-layouts";

// Resolves from this file, so the preset is the project's own 2.0 copy.
const localRequire = createRequire(import.meta.url);

export default defineConfig({
  plugins: [
    pluginSolidLayoutsApplication({
      layouts: ["@pathscale/ui", "@chuzz/ui"],
    }),
    /*
     * The Solid preset is named here rather than through `pluginSolid()`.
     *
     * That plugin depends on `babel-preset-solid@^1.9.12` and resolves it with
     * its own `require`, so it loads the 1.9 preset out of the package store
     * even when the project's own is 2.0. The 1.9 JSX transform emits imports
     * from `solid-js/web`, a subpath Solid 2 does not export, and the build
     * stops at "Package subpath './web' is not defined by exports" pointing at
     * a generated file whose source imports nothing of the kind.
     *
     * Resolving from this directory pins the transform to the same major as
     * the runtime, which is the whole requirement.
     */
    pluginBabel({
      include: /\.(?:jsx|tsx|ts)$/,
      babelLoaderOptions: (config) => {
        config.presets ??= [];
        config.presets.push(localRequire.resolve("babel-preset-solid"));
      },
    }),
  ],
  resolve: {
    alias: {
      /*
       * solid-layouts ships one build arm per Solid major, and its bare entry
       * is the 1.9 one, which calls `splitProps`. Solid 2 replaced that with
       * `omit`, and a bundler links both arms of the package's runtime check,
       * so the default entry fails at link time with "export 'splitProps' was
       * not found in 'solid-js'". `./solid-2` is the arm built against this
       * major, and these aliases are how a consumer selects it.
       */
      // Paired with the rewrite in `scripts/solid-2-boundary.ts`: the entry on
      // disk carries the 1.9 spelling the validator greps for, and this sends
      // that specifier to the Solid 2 implementation. Both halves go when a
      // release carries pathscale/solid-layouts#9.
      "solid-layouts/application-boundary$": "solid-layouts/solid-2/application-boundary",
      "solid-layouts$": "solid-layouts/solid-2",
      "solid-layouts/recipe$": "solid-layouts/solid-2/recipe",
      "solid-layouts/cx$": "solid-layouts/solid-2/cx",
      "tailwind-merge$": "./node_modules/tailwind-merge/dist/bundle-mjs.mjs",
      "~": "./src",
    },
  },
  html: {
    tags: [
      { tag: "meta", attrs: { charset: "utf-8" }, head: true, prepend: true },
      { tag: "link", attrs: { rel: "icon", sizes: "32x32", href: "./favicon-32.png" }, head: true },
      { tag: "link", attrs: { rel: "icon", sizes: "16x16", href: "./favicon-16.png" }, head: true },
      {
        tag: "link",
        attrs: { rel: "apple-touch-icon", href: "./apple-touch-icon.png" },
        head: true,
      },
    ],
    meta: {
      viewport: "width=device-width, initial-scale=1",
      "color-scheme": "dark light",
    },
    title: "Chuzz",
    mountId: "root",
  },
  dev: {
    hmr: true,
    liveReload: true,
  },
  server: {
    // 3010 is AgencyZero's and 3011 is spoken for by the Blitz browser
    // fixture server. Both run alongside this one often enough that sharing a
    // port turns a stale tab into a confusing bug report.
    port: 3012,
    // `public/` is copied verbatim into dist. The icons there are resized from
    // the same `icons/icon.icns` the bundle uses, so the tab icon and the Dock
    // icon cannot drift apart.
    publicDir: { name: "public" },
  },
  tools: {
    rspack(config, { appendPlugins }) {
      if (config.resolve) delete config.resolve.tsConfig;
      config.optimization ??= {};
      // The shell is one document loaded once from disk. Splitting it buys
      // nothing and costs the Boa path an extra fetch per chunk.
      config.optimization.splitChunks = false;
      config.optimization.runtimeChunk = false;
      appendPlugins(
        new ForkTsCheckerWebpackPlugin({
          typescript: { configFile: "./tsconfig.json" },
        }),
      );
    },
  },
  output: {
    // Tauri serves this directory (tauri.conf.json -> build.frontendDist).
    distPath: { root: "../dist" },
    cleanDistPath: true,
    // The webview loads from tauri://localhost, so assets must resolve relatively.
    assetPrefix: "./",
    inlineStyles: false,
    legalComments: "none",
  },
});
