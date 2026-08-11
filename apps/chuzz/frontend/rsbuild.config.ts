import { defineConfig } from "@rsbuild/core";
import { pluginBabel } from "@rsbuild/plugin-babel";
import { pluginSolid } from "@rsbuild/plugin-solid";
import ForkTsCheckerWebpackPlugin from "fork-ts-checker-webpack-plugin";

export default defineConfig({
  plugins: [pluginBabel({ include: /\.(?:jsx|tsx|ts)$/ }), pluginSolid()],
  resolve: {
    alias: {
      "~": "./src",
    },
  },
  html: {
    tags: [{ tag: "meta", attrs: { charset: "utf-8" }, head: true, prepend: true }],
    meta: {
      viewport: "width=device-width, initial-scale=1",
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
