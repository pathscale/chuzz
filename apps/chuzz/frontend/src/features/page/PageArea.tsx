import { Page, SleepView, Viewport } from "@chuzz/ui";
import { Button } from "@pathscale/ui";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";
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
      {/* The inner element is load-bearing, not decoration. A Layout component
          resolves `children` once, when it is first rendered, so whatever the
          children evaluate to at that instant is what stays in the document.
          `tabs` is empty until the shell answers `list_tabs`, so a `For` placed
          directly here resolves to nothing and never runs again: no
          `<web-view>` is ever created, the shell's `page_node` lookup finds no
          mount, and every page loads into a document that is never attached.
          That is the blank window.

          Keeping a plain element between the Layout and the `For` fixes it.
          The element is what gets resolved and inserted once; the `For` lives
          inside it, where its updates are ordinary Solid inserts that keep
          working. Same rule applies in `BrowserHeader`. */}
      <div class="page-stack">
        <For each={browser.state.tabs}>
          {(tab) => (
            <>
              <Page
                tabId={String(tab.id)}
                active={tab.id === browser.state.activeTabId}
                blank={tab.status === "blank"}
              />
              <Show when={tab.id === browser.state.activeTabId && ["sleeping", "waking", "discarding", "restore-failed"].includes(tab.lifecycle ?? "")}>
                <SleepView
                  tabId={String(tab.id)}
                  label={tab.lifecycle === "waking" ? "Waking, saved view" : tab.lifecycle === "discarding" ? "Releasing page" : tab.lifecycle === "restore-failed" ? "Reload failed, saved view" : "Sleeping, saved view"}
                  reason={tab.lifecycleReason ?? ""}
                >
                  <div>
                    <Button id={`chuzz-wake-${tab.id}`} disabled={tab.lifecycle === "waking" || tab.lifecycle === "discarding"} onClick={() => browser.wakeTab(tab.id)}>
                      {tab.lifecycleReason === "reload-requires-resubmission" ? "Open URL to resubmit" : "Wake tab"}
                    </Button>
                  </div>
                </SleepView>
              </Show>
            </>
          )}
        </For>
      </div>
    </Viewport>
  );
}
