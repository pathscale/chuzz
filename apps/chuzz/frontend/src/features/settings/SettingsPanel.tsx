import { Flex } from "@pathscale/ui";
import type { JSX } from "solid-js";
import { ThemePicker } from "./ThemePicker";
import { t } from "~/stores/i18n";
import {
  isDefaultTheme,
  prefs,
  resetTheme,
  setColorMode,
  setTheme,
} from "~/stores/prefs";

/**
 * Settings, which is Appearance and nothing else so far.
 *
 * `ThemePicker` is AgencyZero's, taken unmodified. Everything it needs arrives
 * as props, so the only thing this file owns is where a picked value goes —
 * which for now is `localStorage` and the document, not the shell. Persisting
 * through Rust is a later change to `stores/prefs`, not to the picker.
 */
export function SettingsPanel(props: { onClose: () => void }): JSX.Element {
  return (
    <div class="chrome-settings-scrim" onClick={props.onClose}>
      {/* The sheet swallows clicks so a click inside it does not close it. */}
      <div class="chrome-settings" onClick={(event) => event.stopPropagation()}>
        <Flex as="div" align="center" justify="between" class="chrome-settings-header">
          <div class="chrome-settings-title">{t("appearance.title")}</div>
          <button type="button" class="chrome-titlebar-button" onClick={props.onClose}>
            {"×"}
          </button>
        </Flex>

        <Flex as="div" align="center" gap="sm" class="chrome-settings-row">
          <span class="chrome-settings-label">{t("appearance.mode")}</span>
          <Flex as="div" class="chrome-mode-group">
            <button
              type="button"
              class="chrome-mode-button"
              data-selected={prefs.colorMode === "dark" ? "" : undefined}
              onClick={() => setColorMode("dark")}
            >
              {t("appearance.dark")}
            </button>
            <button
              type="button"
              class="chrome-mode-button"
              data-selected={prefs.colorMode === "light" ? "" : undefined}
              onClick={() => setColorMode("light")}
            >
              {t("appearance.light")}
            </button>
          </Flex>
        </Flex>

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
      </div>
    </div>
  );
}
