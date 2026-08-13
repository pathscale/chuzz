import { AddressBar, NavigationBar } from "@chuzz/ui";
import { Button } from "@pathscale/ui";
import { createEffect, createSignal, type JSX } from "solid-js";
import { useBrowser } from "~/stores/browser";
import { t } from "~/stores/i18n";

/**
 * Back, forward, reload, and the address bar.
 *
 * Ported from `toolbar.rs`, including the one subtlety there: the field tracks
 * the active tab's URL, but only when that URL actually changes. An effect that
 * re-ran on every render would overwrite each keystroke with the current
 * address, and the field could never hold anything typed.
 */
export function Toolbar(): JSX.Element {
  const browser = useBrowser();
  const [typed, setTyped] = createSignal("");
  const [refused, setRefused] = createSignal(false);

  createEffect((previous: string | undefined) => {
    const url = browser.activeTab()?.url ?? "";
    if (url !== previous) {
      setTyped(url);
      setRefused(false);
    }
    return url;
  });

  const submit = async () => {
    // `nav.rs` decides what a typed string means. A value that is neither a URL
    // nor a bare hostname is not a search, and the shell says so rather than
    // navigating somewhere unrelated; showing that beats looking broken.
    const accepted = await browser.navigate(typed());
    setRefused(!accepted);
  };

  return (
    <NavigationBar>
      <Button
        variant="ghost"
        size="sm"
        isIconOnly
        title={t("browser.back")}
        isDisabled={!browser.activeTab()?.canGoBack}
        onClick={() => browser.goBack()}
      >
        {"←"}
      </Button>
      <Button
        variant="ghost"
        size="sm"
        isIconOnly
        title={t("browser.forward")}
        isDisabled={!browser.activeTab()?.canGoForward}
        onClick={() => browser.goForward()}
      >
        {"→"}
      </Button>
      <Button
        variant="ghost"
        size="sm"
        isIconOnly
        title={t("browser.reload")}
        onClick={() => browser.reload()}
      >
        {"↻"}
      </Button>
      <AddressBar
        value={typed()}
        invalid={refused()}
        placeholder={t("browser.addressPlaceholder")}
        onInput={(event) => {
          setTyped(event.currentTarget.value);
          setRefused(false);
        }}
        onKeyDown={(event) => {
          if (event.key === "Enter") void submit();
        }}
      />
    </NavigationBar>
  );
}
