import { SettingsDialog } from "@chuzz/ui";
import type { JSX } from "solid-js";
import { t } from "~/stores/i18n";
import { isDefaultTheme, prefs, resetTheme, setColorMode, setTheme } from "~/stores/prefs";
import { DiagnosticsSection } from "./DiagnosticsSection";
import { ThemePicker } from "./ThemePicker";

/**
 * Settings: Appearance, and the two diagnostics switches.
 *
 * `ThemePicker` is AgencyZero's, taken unmodified. Everything it needs arrives
 * as props, so the only thing this file owns is where a picked value goes —
 * which for now is `localStorage` and the document, not the shell. Persisting
 * through Rust is a later change to `stores/prefs`, not to the picker.
 */
export function SettingsPanel(props: { isOpen: boolean; onClose: () => void }): JSX.Element {
  return (
    <SettingsDialog
      open={props.isOpen}
      onClose={props.onClose}
      title={t("appearance.title")}
      modeLabel={t("appearance.mode")}
      darkLabel={t("appearance.dark")}
      lightLabel={t("appearance.light")}
      mode={prefs.colorMode}
      onModeChange={setColorMode}
    >
      <ThemePicker
        theme={prefs.theme}
        onSurface={(hex) => setTheme("surface", hex)}
        onAccent={(hex) => setTheme("accent", hex)}
        onSoftness={(value) => setTheme("softness", value)}
        onWash={(value) => setTheme("wash", value)}
        onBrightness={(value) => setTheme("textBrightness", value)}
        onReset={resetTheme}
        isDefault={isDefaultTheme()}
      />
      <DiagnosticsSection />
    </SettingsDialog>
  );
}
