import { Button, Flex } from "@pathscale/test-ui";
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
    <Flex as="div" align="center" class="chrome-toolbar">
      <Button
        variant="ghost"
        squareSize={30}
        title={t("chrome.back")}
        isDisabled={!browser.activeTab()?.canGoBack}
        onClick={() => browser.goBack()}
      >
        {"←"}
      </Button>
      <Button
        variant="ghost"
        squareSize={30}
        title={t("chrome.forward")}
        isDisabled={!browser.activeTab()?.canGoForward}
        onClick={() => browser.goForward()}
      >
        {"→"}
      </Button>
      <Button
        variant="ghost"
        squareSize={30}
        title={t("chrome.reload")}
        onClick={() => browser.reload()}
      >
        {"↻"}
      </Button>
      <input
        class="chrome-url-bar"
        type="text"
        value={typed()}
        data-refused={refused() ? "" : undefined}
        placeholder={t("chrome.addressPlaceholder")}
        onInput={(event) => {
          setTyped(event.currentTarget.value);
          setRefused(false);
        }}
        onKeyDown={(event) => {
          if (event.key === "Enter") void submit();
        }}
      />
    </Flex>
  );
}
