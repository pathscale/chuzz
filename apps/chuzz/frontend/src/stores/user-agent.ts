import { createRoot, createStore } from "solid-js";
import { api } from "~/api";
import type { UserAgentState } from "~/types";

const [state, setState] = createRoot(() =>
  createStore<UserAgentState>({ spoofing: true, locked: false, userAgent: "" }),
);

export { state as userAgent };

export async function syncUserAgent(): Promise<void> {
  const next = await api.userAgent();
  setState((draft) => Object.assign(draft, next));
}

export async function setUserAgentSpoofing(spoofing: boolean): Promise<void> {
  if (state.locked) return;
  const next = await api.setUserAgentSpoofing(spoofing);
  setState((draft) => Object.assign(draft, next));
}
