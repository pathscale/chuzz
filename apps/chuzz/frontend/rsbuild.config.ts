import { defineConfig } from "@rsbuild/core";
import { pluginBabel } from "@rsbuild/plugin-babel";
import { pluginSolid } from "@rsbuild/plugin-solid";
import ForkTsCheckerWebpackPlugin from "fork-ts-checker-webpack-plugin";
import { pluginSolidLayoutsApplication } from "../../../../solid-layouts/packages/solid-layouts-oxc/application.js";

export default defineConfig({
  plugins: [
    pluginSolidLayoutsApplication({
      layouts: [
        {
          module: "@pathscale/test-ui",
          root: "../../../../solid-layouts/Test-UI/bundle",
        },
      ],
      runtime: "../../../../solid-layouts/packages/solid-layouts/src/index.ts",
    }),
    pluginBabel({ include: /\.(?:jsx|tsx|ts)$/ }),
    pluginSolid(),
  ],
  resolve: {
    alias: {
      "solid-layouts$": "../../../../solid-layouts/packages/solid-layouts/src/index.ts",
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
    rspack: {
      optimization: {
        // The shell is one document loaded once from disk. Splitting it buys
        // nothing and costs the Boa path an extra fetch per chunk.
        splitChunks: false,
        runtimeChunk: false,
      },
      plugins: [
        new ForkTsCheckerWebpackPlugin({
          typescript: { configFile: "./tsconfig.json" },
        }),
      ],
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
