import { Page, Viewport } from "@chuzz/ui";
import { For, type JSX } from "solid-js";
import { useBrowser } from "~/stores/browser";

/**
 * Where pages live: one `<web-view>` per tab, only the active one displayed.
 *
 * This is the seam between the two documents. The interface renders the element
 * and nothing else; the shell finds it by `data-tab-id` and attaches the
 * `Document` it already loaded. That split is deliberate — under Dioxus the
 * page document was passed straight through as an `rsx!` attribute
 * (`tab.rs`'s `__webview_document`), which JavaScript cannot do, so the
 * element is a rendezvous point rather than a carrier.
 *
 * Tabs stay mounted while hidden, matching `TabView`: a background tab that
 * unmounted would drop its document and reload on every visit.
 */
export function PageArea(): JSX.Element {
  const browser = useBrowser();

  return (
    <Viewport>
      <For each={browser.state.tabs}>
        {(tab) => (
          <Page tabId={String(tab.id)} active={tab.id === browser.state.activeTabId} />
        )}
      </For>
    </Viewport>
  );
}
