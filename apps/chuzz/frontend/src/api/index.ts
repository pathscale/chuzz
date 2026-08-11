import type { BrowserApi } from "./client";
import { createMockApi } from "./mock";
import { createTauriApi } from "./tauri";

export type { BrowserApi, BrowserEvent, BrowserEvents, Unlisten } from "./client";

/**
 * Whether a Tauri shell is hosting this document.
 *
 * `__TAURI_INTERNALS__` is what the API package itself looks for, so this asks
 * the same question `invoke` will ask rather than sniffing a user agent that
 * Boa and WebKit answer differently.
 */
function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/**
 * One implementation, chosen once.
 *
 * Deliberately not per-method: AgencyZero's equivalent falls back to the mock
 * for any method missing from its command map, and its own comment records what
 * that cost — a Reset that reported success while doing nothing, and a composer
 * quoting mock prices. A method that cannot reach Rust should fail loudly here,
 * not succeed against a stand-in.
 */
export const api: BrowserApi = isTauri() ? createTauriApi() : createMockApi();
