import { recipe } from "solid-layouts";

export const page = recipe({
  component: "page",
  element: "web-view",
  slots: { root: { base: "page" } },
  props: {
    tabId: {},
    active: {
      true: "page-active",
      false: "page-hidden",
    },
    /**
     * Nothing has been loaded here yet.
     *
     * The only thing this decides is what shows behind the document. A blank
     * tab is part of the window and takes the theme's surface; a real page is a
     * document and takes the canvas white every engine defaults to, because a
     * page that declares no background of its own is written against white.
     */
    blank: {
      true: "page-blank",
      false: {},
    },
  },
});
