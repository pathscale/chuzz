import { InspectorRow, InspectorSection, SidePanel as Panel } from "@chuzz/ui";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";
import { useBrowser } from "~/stores/browser";
import { t } from "~/stores/i18n";
import type { InspectorSection as InspectorSectionModel, PanelSections } from "~/types";

/**
 * The right-hand inspector, ported from `side_panel.rs` (itself a port of
 * AgencyZero's `ProjectPanel`).
 *
 * Collapsed, the panel keeps the seam and the handle attached to it but gives
 * up its width and contents. A section's body only renders while open, so a
 * collapsed section costs a header and nothing else.
 *
 * Rows are still mock, exactly as they were in Rust. The shape is real.
 */
const SECTIONS: InspectorSectionModel[] = [
  {
    key: "page",
    title: t("browser.page"),
    count: 4,
    tone: "primary",
    rows: [
      { label: "Title", value: "New Tab" },
      { label: "Nodes", value: "18" },
      { label: "Stylesheets", value: "1" },
      { label: "Images", value: "0" },
    ],
  },
  {
    key: "history",
    title: t("browser.history"),
    count: 3,
    tone: "neutral",
    rows: [
      { label: "Back", value: "2 entries" },
      { label: "Forward", value: "0 entries" },
      { label: "Current", value: "about:blank" },
    ],
  },
  {
    key: "network",
    title: t("browser.network"),
    count: 6,
    tone: "neutral",
    rows: [
      { label: "Requests", value: "6" },
      { label: "Transferred", value: "1.2 kB" },
      { label: "Cached", value: "0" },
    ],
  },
  {
    key: "console",
    title: t("browser.console"),
    count: 0,
    tone: "neutral",
    rows: [{ label: "Messages", value: "0" }],
  },
];

export function SidePanel(): JSX.Element {
  const browser = useBrowser();
  const panel = () => browser.state.panel;

  return (
    <Panel title={t("browser.inspector")} collapsed={panel().collapsed}>
      {/* An element between the Layout and the lists, for the reason spelled
          out in `PageArea`: a Layout resolves its children once, so a `For`
          placed directly here would be frozen at whatever it saw on the first
          render, which for the debugging stream is nothing at all. */}
      <div class="side-panel__sections">
        <DebuggingSection isOpen={panel().sections.debugging} />
        <For each={SECTIONS}>
          {(section) => <Section section={section} isOpen={panel().sections[section.key]} />}
        </For>
      </div>
    </Panel>
  );
}

/**
 * The verbose stream: every navigation, fetch, script and module, as it happens.
 *
 * First in the panel because it is the section anyone opens the panel for. The
 * others are a fixed shape with mock numbers behind them; this one is the only
 * live thing in here.
 *
 * The counter is the number of lines held, capped at what the shell keeps, and
 * the tone follows the worst level in the buffer: a panel that has to be opened
 * and read to find out something failed is a panel that gets ignored.
 */
function DebuggingSection(props: { isOpen: boolean }): JSX.Element {
  const browser = useBrowser();
  const entries = () => browser.state.debug;
  const worst = () =>
    entries().some((entry) => entry.level === "error" || entry.level === "warn")
      ? "primary"
      : "neutral";

  return (
    <InspectorSection
      id="inspector-debugging"
      title={t("browser.debugging")}
      count={entries().length}
      tone={worst()}
      open={props.isOpen}
      onOpenChange={(open) => {
        if (open !== props.isOpen) browser.toggleSection("debugging");
      }}
    >
      <div class="debug-log">
        <Show
          when={entries().length > 0}
          fallback={<div class="debug-empty">{t("browser.debuggingEmpty")}</div>}
        >
          <For each={entries()}>
            {(entry) => (
              <div class={`debug-line debug-line-${entry.level}`}>
                <span class="debug-line__source">{entry.source}</span>
                <span class="debug-line__message">{entry.message}</span>
              </div>
            )}
          </For>
        </Show>
      </div>
    </InspectorSection>
  );
}

function Section(props: { section: InspectorSectionModel; isOpen: boolean }): JSX.Element {
  const browser = useBrowser();
  const toggle = () => browser.toggleSection(props.section.key as keyof PanelSections);

  return (
    <InspectorSection
      id={`inspector-${props.section.key}`}
      title={props.section.title}
      count={props.section.count}
      tone={props.section.tone}
      open={props.isOpen}
      onOpenChange={(open) => {
        if (open !== props.isOpen) toggle();
      }}
    >
      <For each={props.section.rows}>
        {(row) => <InspectorRow label={row.label} value={row.value} />}
      </For>
    </InspectorSection>
  );
}
