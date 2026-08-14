/// <reference types="@rsbuild/core/types" />

declare global {
  interface Window {
    __CHUZZ_BLITZ__?: boolean;
  }
}

import type { JSX } from "solid-js";

declare module "solid-js" {
  namespace JSX {
    interface IntrinsicElements {
      /**
       * The shell's page mount.
       *
       * Not a web component and not registered with `customElements`: Blitz
       * gives this element a child document from the Rust side, and the tag is
       * how the shell finds the node. Declared here because Solid's JSX types
       * are closed, and an undeclared tag is a type error rather than a
       * silently accepted unknown element.
       */
      "web-view": JSX.HTMLAttributes<HTMLElement> & {
        "data-tab-id"?: string;
      };
    }
  }
}
