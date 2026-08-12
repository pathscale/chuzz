import { useBrowser } from "~/stores/browser";
import type { InspectorSection, PanelSections } from "~/types";

/**
 * The inspector's behaviour. No JSX, no class names, no presentation props.
 *
 * The payoff is that this file is testable on its own: which sections exist,
 * which are open, and what toggling one does are all answerable without
 * rendering anything.
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

export function createSidePanel() {
  const browser = useBrowser();

  const model = {
    isOpen: (key: string) =>
      Boolean(browser.state.panel.sections[key as keyof PanelSections]),
    toggle: (key: string) => browser.toggleSection(key as keyof PanelSections),
  };

  return {
    // A `state` key of the `sidePanel` recipe, so it reaches the class map by
    // name rather than by a hand-written `data-collapsed` on one element.
    collapsed: () => browser.state.panel.collapsed,
    sections: () => SECTIONS,
    // The per-section helpers, handed over as the object rather than an
    // accessor to it. Only names the recipe declares as state are unwrapped,
    // so an accessor here would arrive as the function itself.
    model,
  };
}

export type SidePanelModel = {
  isOpen: (key: string) => boolean;
  toggle: (key: string) => void;
};
