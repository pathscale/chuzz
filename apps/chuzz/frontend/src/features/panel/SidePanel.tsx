import { InspectorRow, InspectorSection, SidePanel as Panel } from "@chuzz/ui";
import { For, type JSX } from "solid-js";
import { useBrowser } from "~/stores/browser";
import { t } from "~/stores/i18n";
import type { InspectorSection, PanelSections } from "~/types";

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
const SECTIONS: InspectorSection[] = [
  {
    key: "page",
    title: "Page",
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
    title: "History",
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
    title: "Network",
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
    title: "Console",
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
      <For each={SECTIONS}>
        {(section) => <Section section={section} isOpen={panel().sections[section.key]} />}
      </For>
    </Panel>
  );
}

function Section(props: { section: InspectorSection; isOpen: boolean }): JSX.Element {
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
