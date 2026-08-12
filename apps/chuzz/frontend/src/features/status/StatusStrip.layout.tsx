import { Flex } from "@pathscale/ui";
import type { JSX } from "solid-js";
import { t } from "~/stores/i18n";
import { statusStrip } from "./StatusStrip.recipe";
import type { StatusStripModel } from "./statusStrip";

/**
 * The strip along the bottom: a live dot and the current state on the left,
 * monospace counters on the right.
 *
 * Markup only. The readout comes from `statusStrip.ts`, the attributes from
 * the recipe. Nothing here computes a class or writes a `data-` attribute by
 * hand, which is the whole difference from the version this replaces.
 */
export function StatusStripLayout(props: {
  model: StatusStripModel;
}): JSX.Element {
  const slot = () => statusStrip.resolve({ loading: props.model.loading() });

  return (
    <Flex as="div" align="center" gap="sm" {...slot().root}>
      <span {...slot().dot!} />
      <span>{props.model.loading() ? t("chrome.loading") : t("chrome.idle")}</span>
      <span>·</span>
      <span {...slot().url!}>{props.model.url()}</span>
      <div {...slot().spacer!} />
      <span>
        {props.model.tabCount()} {t("chrome.tabs")}
      </span>
      <span>·</span>
      <span>
        {props.model.nodeCount()} {t("chrome.nodes")}
      </span>
      <span>·</span>
      <span {...slot().accent!}>{props.model.transferred()}</span>
    </Flex>
  );
}
