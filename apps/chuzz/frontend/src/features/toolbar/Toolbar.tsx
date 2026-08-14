import { AddressBar, NavigationBar } from "@chuzz/ui";
import { createEffect, createSignal, type JSX } from "solid-js";
import { useBrowser } from "~/stores/browser";
import { t } from "~/stores/i18n";

/**
 * The address bar. History navigation and reload remain keyboard actions; the
 * old buttons were inert visual duplicates in the accelerated chrome.
 *
 * Ported from `toolbar.rs`, including the one subtlety there: the field tracks
 * the active tab's URL, but only when that URL actually changes. An effect that
 * re-ran on every render would overwrite each keystroke with the current
 * address, and the field could never hold anything typed.
 */
export function Toolbar(): JSX.Element {
  const browser = useBrowser();
  const [typed, setTyped] = createSignal("");

  createEffect((previous: string | undefined) => {
    const url = browser.activeTab()?.url ?? "";
    if (url !== previous) {
      setTyped(url === "about:blank" ? "" : url);
    }
    return url;
  });

  return (
    <NavigationBar>
      <AddressBar
        id="chuzz-address-bar"
        value={typed()}
        invalid={false}
        placeholder={t("browser.addressPlaceholder")}
        onInput={(event) => {
          setTyped((event.target as HTMLInputElement).value);
        }}
      />
    </NavigationBar>
  );
}
