import { AppShell, MainContent, PanelHandle } from "@chuzz/ui";
import { createSignal, type JSX } from "solid-js";
import { PageArea } from "~/features/page/PageArea";
import { SidePanel } from "~/features/panel/SidePanel";
import { SettingsPanel } from "~/features/settings/SettingsPanel";
import { BrowserHeader } from "~/features/tabs/BrowserHeader";
import { Toolbar } from "~/features/toolbar/Toolbar";
import { BrowserProvider, useBrowser } from "~/stores/browser";
import { t } from "~/stores/i18n";

/**
 * The window: titlebar, toolbar, then the content row. Loading is each tab's
 * live backend status rather than a bar of its own.
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
      <MainContent>
        <PageArea />
        <PanelHandle
          title={t(
            browser.state.panel.collapsed ? "browser.showInspector" : "browser.hideInspector",
          )}
          collapsed={browser.state.panel.collapsed}
          onClick={() => browser.setPanelCollapsed(!browser.state.panel.collapsed)}
        />
        <SidePanel />
      </MainContent>
      <SettingsPanel isOpen={settingsOpen()} onClose={() => setSettingsOpen(false)} />
    </AppShell>
  );
}
