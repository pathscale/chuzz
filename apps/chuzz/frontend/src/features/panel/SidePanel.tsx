import { Button, Chip, Flex, Icon } from "@pathscale/test-ui";
import { For, Show, type JSX } from "solid-js";
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
    <Show
      when={!panel().collapsed}
      fallback={<div class="chrome-side-panel" data-collapsed="" />}
    >
      <div class="chrome-side-panel">
        <Flex as="div" align="center" class="chrome-side-panel-header">
          <div class="chrome-side-panel-title">{t("chrome.inspector")}</div>
        </Flex>
        <div class="chrome-side-panel-scroll">
          <For each={SECTIONS}>
            {(section) => (
              <Section section={section} isOpen={panel().sections[section.key]} />
            )}
          </For>
        </div>
      </div>
    </Show>
  );
}

function Section(props: { section: InspectorSection; isOpen: boolean }): JSX.Element {
  const browser = useBrowser();
  const toggle = () => browser.toggleSection(props.section.key as keyof PanelSections);

  return (
    <div class="chrome-section" data-open={props.isOpen ? "" : undefined}>
      <Button
        variant="ghost"
        size="sm"
        fullWidth
        justify="start"
        radius="none"
        aria-expanded={props.isOpen}
        onClick={toggle}
      >
        <span class="chrome-section-title">{props.section.title}</span>
        {/* Chip, not Badge: Badge defaults to `placement: top-right`, which
            positions it absolutely as an overlay on a host element. This is an
            inline count beside a title. */}
        <Chip
          variant="flat"
          color={props.section.tone === "primary" ? "primary" : "default"}
          size="sm"
        >
          {props.section.count}
        </Chip>
        <Icon
          class="chrome-section-indicator"
          name={props.isOpen ? "icon-[mdi--chevron-down]" : "icon-[mdi--chevron-right]"}
          width={16}
          height={16}
        />
      </Button>
      <Show when={props.isOpen}>
        <div class="chrome-section-body">
          <For each={props.section.rows}>
            {(row) => (
              <Flex as="div" align="center" justify="between" class="chrome-section-row">
                <span class="chrome-section-row-label">{row.label}</span>
                <span class="chrome-section-row-value">{row.value}</span>
              </Flex>
            )}
          </For>
        </div>
      </Show>
    </div>
  );
}
