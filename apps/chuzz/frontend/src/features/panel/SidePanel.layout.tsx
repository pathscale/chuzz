import { Chip, Icon } from "@pathscale/ui";
import type { Layout } from "solid-layouts";
import { For, Show, type JSX } from "solid-js";
import { t } from "~/stores/i18n";
import type { InspectorSection } from "~/types";
import { panelRow, panelSection, sidePanel } from "./SidePanel.recipe";
import type { SidePanelModel } from "./sidePanel";

/**
 * The inspector's markup, and nothing else.
 *
 * Nothing here computes a class, writes a `data-` attribute by hand, or
 * decides anything. Every value arrives already resolved: the readout from
 * `sidePanel.ts`, the attributes from the recipes.
 */
export const SidePanelLayout: Layout<typeof sidePanel> = ({ slot }, props) => (
  <Show
    when={!props.collapsed}
    fallback={<div {...sidePanel.resolve({ collapsed: true }).root} />}
  >
    <div {...slot.root}>
      <div {...slot.header}>
        <div {...slot.title}>{t("chrome.inspector")}</div>
      </div>
      <div {...slot.scroll}>
        <For each={props.sections as InspectorSection[]}>
          {(section) => (
            <PanelSection section={section} model={props.model as SidePanelModel} />
          )}
        </For>
      </div>
    </div>
  </Show>
);

function PanelSection(props: {
  section: InspectorSection;
  model: SidePanelModel;
}): JSX.Element {
  const slot = () =>
    panelSection.resolve({
      open: props.model.isOpen(props.section.key),
      tone: props.section.tone,
    });

  return (
    <div {...slot().root}>
      <button
        {...slot().trigger!}
        type="button"
        aria-expanded={props.model.isOpen(props.section.key)}
        onClick={() => props.model.toggle(props.section.key)}
      >
        <span {...slot().title!}>{props.section.title}</span>
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
          {...slot().indicator!}
          name={
            props.model.isOpen(props.section.key)
              ? "icon-[mdi--chevron-down]"
              : "icon-[mdi--chevron-right]"
          }
          width={16}
          height={16}
        />
      </button>
      <Show when={props.model.isOpen(props.section.key)}>
        <div {...slot().body!}>
          <For each={props.section.rows}>
            {(row) => <PanelRow label={row.label} value={row.value} />}
          </For>
        </div>
      </Show>
    </div>
  );
}

function PanelRow(props: { label: string; value: string }): JSX.Element {
  const slot = panelRow.resolve({});
  return (
    <div {...slot.root}>
      <span {...slot.label!}>{props.label}</span>
      <span {...slot.value!}>{props.value}</span>
    </div>
  );
}
