import { Flex } from "@pathscale/ui";
import type { JSX } from "solid-js";
import { t } from "~/stores/i18n";
import { toolbar } from "./Toolbar.recipe";
import type { ToolbarModel } from "./toolbar";

/**
 * Back, forward, reload, and the address bar.
 *
 * Markup only. The address-field behaviour, including the subtlety about only
 * tracking the active tab's URL when it actually changes, lives in
 * `toolbar.ts` where it can be read and tested without a DOM.
 */
export function ToolbarLayout(props: { model: ToolbarModel }): JSX.Element {
  const slot = () => toolbar.resolve({ refused: props.model.refused() });

  return (
    <Flex as="div" align="center" {...slot().root}>
      <button
        {...slot().button!}
        type="button"
        title={t("chrome.back")}
        disabled={!props.model.canGoBack()}
        onClick={props.model.goBack}
      >
        {"←"}
      </button>
      <button
        {...slot().button!}
        type="button"
        title={t("chrome.forward")}
        disabled={!props.model.canGoForward()}
        onClick={props.model.goForward}
      >
        {"→"}
      </button>
      <button
        {...slot().button!}
        type="button"
        title={t("chrome.reload")}
        onClick={props.model.reload}
      >
        {"↻"}
      </button>
      <input
        {...slot().url!}
        type="text"
        value={props.model.typed()}
        placeholder={t("chrome.addressPlaceholder")}
        onInput={(event) => props.model.setTyped(event.currentTarget.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter") void props.model.submit();
        }}
      />
    </Flex>
  );
}
