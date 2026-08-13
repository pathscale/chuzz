import { StatusBar } from "@chuzz/ui";
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
    <StatusBar
      loading={status().status === "loading"}
      loadingLabel={t("browser.loading")}
      idleLabel={t("browser.idle")}
      url={status().url}
      tabCount={status().tabCount}
      tabsLabel={t("browser.tabs")}
      nodeCount={status().nodeCount}
      nodesLabel={t("browser.nodes")}
      transferred={status().transferred}
    />
  );
}
