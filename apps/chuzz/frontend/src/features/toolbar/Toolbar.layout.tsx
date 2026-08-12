import { Flex } from "@pathscale/ui";
import type { Layout } from "solid-layouts";
import { t } from "~/stores/i18n";
import { toolbar } from "./Toolbar.recipe";

/**
 * Back, forward, reload, and the address bar.
 *
 * Markup only. The address-field behaviour, including the subtlety about only
 * tracking the active tab's URL when it actually changes, lives in
 * `toolbar.ts` where it can be read and tested without a DOM.
 */
export const ToolbarLayout: Layout<typeof toolbar> = ({ slot }, props) => (
  <Flex as="div" align="center" {...slot.root}>
    <button
      {...slot.button}
      type="button"
      title={t("chrome.back")}
      disabled={!props.canGoBack}
      onClick={props.goBack as () => void}
    >
      {"←"}
    </button>
    <button
      {...slot.button}
      type="button"
      title={t("chrome.forward")}
      disabled={!props.canGoForward}
      onClick={props.goForward as () => void}
    >
      {"→"}
    </button>
    <button
      {...slot.button}
      type="button"
      title={t("chrome.reload")}
      onClick={props.reload as () => void}
    >
      {"↻"}
    </button>
    <input
      {...slot.url}
      type="text"
      value={props.typed as string}
      placeholder={t("chrome.addressPlaceholder")}
      onInput={(event) =>
        (props.setTyped as (v: string) => void)(event.currentTarget.value)
      }
      onKeyDown={(event) => {
        if (event.key === "Enter") void (props.submit as () => Promise<void>)();
      }}
    />
  </Flex>
);
