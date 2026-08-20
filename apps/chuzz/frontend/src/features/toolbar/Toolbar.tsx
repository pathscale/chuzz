import { AddressBar, NavigationBar } from "@chuzz/ui";
import type { JSX } from "@solidjs/web";
import { createEffect, createSignal } from "solid-js";
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

  // Solid 2 splits this in two: the compute function tracks, and the effect
  // function receives its value and the previous one. The single-argument
  // Solid 1 form throws at runtime, because the missing effect argument is
  // called as a function. See the note in `stores/browser.tsx`.
  createEffect(
    () => browser.activeTab()?.url ?? "",
    (url, previous) => {
      if (url !== previous) {
        setTyped(url === "about:blank" ? "" : url);
      }
    },
  );

  return (
    <NavigationBar>
      <AddressBar
        id="chuzz-address-bar"
        value={typed()}
        invalid={false}
        placeholder={t("browser.addressPlaceholder")}
        onInput={(event: Event) => {
          setTyped((event.target as HTMLInputElement).value);
        }}
      />
    </NavigationBar>
  );
}
