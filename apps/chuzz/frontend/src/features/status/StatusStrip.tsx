import { Flex } from "@pathscale/test-ui";
import type { JSX } from "solid-js";
import { useBrowser } from "~/stores/browser";
import { t } from "~/stores/i18n";

/**
 * The strip along the bottom: a live dot and the current state on the left,
 * monospace counters on the right.
 *
 * Ported from `status_strip.rs`. Node count and transferred bytes are still
 * mock on the Rust side; they are wired through the real readout anyway so the
 * day they become real is a change in one function, not in this file.
 */
export function StatusStrip(): JSX.Element {
  const browser = useBrowser();
  const status = () => browser.state.status;

  return (
    <Flex as="div" align="center" gap="sm" class="chrome-status-strip">
      <span
        class="chrome-status-dot"
        data-loading={status().status === "loading" ? "" : undefined}
      />
      <span>{status().status === "loading" ? t("chrome.loading") : t("chrome.idle")}</span>
      <span>·</span>
      <span class="chrome-status-url">{status().url}</span>
      <div class="chrome-status-spacer" />
      <span>
        {status().tabCount} {t("chrome.tabs")}
      </span>
      <span>·</span>
      <span>
        {status().nodeCount} {t("chrome.nodes")}
      </span>
      <span>·</span>
      <span class="chrome-status-accent">{status().transferred}</span>
    </Flex>
  );
}
