import { createStore } from "solid-js/store";
import { api } from "~/api";
import type { DiagnosticsState } from "~/types";

/**
 * The inspection and profiling switches.
 *
 * Both are off until asked for. The inspection plane binds a local socket that
 * lets any process running as you read this window and drive it: synthesize
 * keys and clicks, read the DOM, quit the browser. That is worth having behind
 * a switch you set, not something a browser holding live sessions turns on
 * because it started.
 *
 * The store mirrors the runtime rather than commanding it, and the shell owns
 * both the applying and the remembering. Two reasons it is not kept here: the
 * window's `localStorage` is an in-memory shim, so a choice stored in it would
 * be forgotten on quit; and the runtime refuses deep profiling while inspection
 * is off, so the pair that took effect is not always the pair that was asked
 * for. Writing back what the shell reports keeps the switches honest about
 * what is actually running.
 */
const [state, setState] = createStore<DiagnosticsState>({
  inspection: false,
  profiling: false,
  locked: false,
});

export { state as diagnostics };

export async function syncDiagnostics(): Promise<void> {
  setState(await api.diagnostics());
}

export async function setDiagnostics(inspection: boolean, profiling: boolean): Promise<void> {
  if (state.locked) return;
  setState(await api.setDiagnostics(inspection, profiling));
}
