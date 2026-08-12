import { createSignal, Show, type JSX } from "solid-js";
import { PageArea } from "~/features/page/PageArea";
import { SidePanel } from "~/features/panel";
import { SettingsPanel } from "~/features/settings/SettingsPanel";
import { StatusStrip } from "~/features/status";
import { TitleBar } from "~/features/tabs/TitleBar";
import { Toolbar } from "~/features/toolbar";
import { BrowserProvider, useBrowser } from "~/stores/browser";

/**
 * The window, in the order `main.rs` lays it out: titlebar, toolbar, an
 * optional loading bar, the content row, then the status strip.
 */
export function App(): JSX.Element {
  return (
    <BrowserProvider>
      <Shell />
    </BrowserProvider>
  );
}

function Shell(): JSX.Element {
  const browser = useBrowser();
  const [settingsOpen, setSettingsOpen] = createSignal(false);

  return (
    <div class="chrome-frame" tabindex={0}>
      <TitleBar onOpenSettings={() => setSettingsOpen(true)} />
      <Toolbar />
      <Show when={browser.state.status.status === "loading"}>
        <div class="chrome-loading-bar" />
      </Show>
      <div class="chrome-content-row">
        <PageArea />
        <button
          type="button"
          class="chrome-panel-handle"
          title="Toggle inspector"
          onClick={() => browser.setPanelCollapsed(!browser.state.panel.collapsed)}
        />
        <SidePanel />
      </div>
      <StatusStrip />
      <Show when={settingsOpen()}>
        <SettingsPanel onClose={() => setSettingsOpen(false)} />
      </Show>
    </div>
  );
}
