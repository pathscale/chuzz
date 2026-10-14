import { recipe } from "solid-layouts";

export const sleepView = recipe({
  component: "sleep-view",
  element: "section",
  slots: {
    root: { base: "sleep-view" },
    image: { base: "sleep-view__image" },
    caption: { base: "sleep-view__caption" },
    actions: { base: "sleep-view__actions" },
  },
  props: { tabId: {}, label: {}, reason: {}, ageSeconds: {}, hasPreview: {} },
});
