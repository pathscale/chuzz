import type { BrowserApi } from "./client";
import { createIzumoApi } from "./izumo";
import { createMockApi } from "./mock";

export type { BrowserApi, BrowserEvent, BrowserEvents, Unlisten } from "./client";

/**
 * Whether izumo is hosting this document.
 *
 * izumo injects `__IZUMO__` before the chrome scripts execute, so the global
 * directly identifies the shell without relying on a user agent.
 */
function isIzumo(): boolean {
  return typeof window !== "undefined" && "__IZUMO__" in window;
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
export const api: BrowserApi = isIzumo() ? createIzumoApi() : createMockApi();

