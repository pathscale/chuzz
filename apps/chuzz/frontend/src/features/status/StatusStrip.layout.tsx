import { Flex } from "@pathscale/ui";
import type { Layout } from "solid-layouts";
import { t } from "~/stores/i18n";
import { statusStrip } from "./StatusStrip.recipe";

/**
 * The strip along the bottom: a live dot and the current state on the left,
 * monospace counters on the right.
 *
 * Markup only, in the conformant form: `Layout<typeof recipe>` is what types
 * `p`, and the two-parameter signature is what makes `slot` safe to
 * destructure while leaving reactive reads visibly behind `p`.
 */
export const StatusStripLayout: Layout<typeof statusStrip> = ({ slot }, props) => (
  <Flex as="div" align="center" gap="sm" {...slot.root}>
    <span {...slot.dot} />
    <span>{props.loading ? t("chrome.loading") : t("chrome.idle")}</span>
    <span>·</span>
    <span {...slot.url}>{props.url as string}</span>
    <div {...slot.spacer} />
    <span>
      {props.tabCount as number} {t("chrome.tabs")}
    </span>
    <span>·</span>
    <span>
      {props.nodeCount as number} {t("chrome.nodes")}
    </span>
    <span>·</span>
    <span {...slot.accent}>{props.transferred as string}</span>
  </Flex>
);
