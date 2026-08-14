import { AppShell, LoadingBar, MainContent, PanelHandle } from "@chuzz/ui";
import { createSignal, type JSX } from "solid-js";
import { PageArea } from "~/features/page/PageArea";
import { SidePanel } from "~/features/panel/SidePanel";
import { SettingsPanel } from "~/features/settings/SettingsPanel";
import { StatusStrip } from "~/features/status/StatusStrip";
import { BrowserHeader } from "~/features/tabs/BrowserHeader";
import { Toolbar } from "~/features/toolbar/Toolbar";
import { BrowserProvider, useBrowser } from "~/stores/browser";
import { t } from "~/stores/i18n";

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
    <AppShell tabindex={0}>
      <BrowserHeader onOpenSettings={() => setSettingsOpen(true)} />
      <Toolbar />
      <LoadingBar
        loading={browser.state.status.status === "loading"}
        label={t("browser.loading")}
      />
      <MainContent>
        <PageArea />
        <PanelHandle
          title={browser.state.panel.collapsed ? "Show inspector" : "Hide inspector"}
          collapsed={browser.state.panel.collapsed}
          onClick={() => browser.setPanelCollapsed(!browser.state.panel.collapsed)}
        />
        <SidePanel />
      </MainContent>
      <StatusStrip />
      <SettingsPanel isOpen={settingsOpen()} onClose={() => setSettingsOpen(false)} />
    </AppShell>
  );
}
