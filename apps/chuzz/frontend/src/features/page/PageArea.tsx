import { For, type JSX } from "solid-js";
import { useBrowser } from "~/stores/browser";

/**
 * Where pages live: one `<web-view>` per tab, only the active one displayed.
 *
 * This is the seam between the two documents. The chrome renders the element
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
    <div class="chrome-page-area">
      <For each={browser.state.tabs}>
        {(tab) => (
          <web-view
            class="chrome-page"
            data-tab-id={String(tab.id)}
            style={{ display: tab.id === browser.state.activeTabId ? "block" : "none" }}
          />
        )}
      </For>
    </div>
  );
}
