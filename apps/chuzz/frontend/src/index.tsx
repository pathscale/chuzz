/* @refresh reload */
import "./index.css";
import { enablePopmotion } from "@pathscale/ui/motion";
import { animate } from "popmotion";
import { render } from "solid-js/web";
import { App } from "./App";
import { syncTheme } from "./stores/prefs";

// Without a driver, every @pathscale/ui animation snaps to its end state.
enablePopmotion(animate);

// `data-theme` is the stable identity @pathscale/ui and Tailwind resolve
// against; `data-color-mode` is the light/dark axis. Both are set here, before
// the first render, so the window never paints once at the default palette and
// then again at the stored one.
syncTheme();

const root = document.getElementById("root");

if (import.meta.env.DEV && !(root instanceof HTMLElement)) {
  throw new Error("Root element #root not found — check rsbuild's html.mountId.");
}

render(() => <App />, root!);
