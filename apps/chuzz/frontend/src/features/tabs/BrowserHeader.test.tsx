import { readFileSync } from "node:fs";
import { join } from "node:path";
import { Tab as BrowserTab } from "@chuzz/ui";
import { Button } from "@pathscale/ui";
import { render } from "@solidjs/testing-library";
import { describe, expect, it, vi } from "vitest";
import type { LoadStatus } from "~/types";

// Read off disk rather than imported. Vite's CSS pipeline claims `.css` before
// `?raw` can and hands back an empty string, which made four assertions pass
// vacuously against a stylesheet nobody had read. `tsconfig.test.json` is what
// makes `node:fs` legal here; the app's own compilation excludes these files.
const HERE = join(process.cwd(), "src/features/tabs");
// Comments stripped first. `ruleFor` reads to the next `}`, and these comments
// quote CSS at each other: a rule whose comment mentions
// `.button--width-square.button--sm { width: 2.25rem }` was truncated at that
// brace and every declaration in it read as absent.
const styles = readFileSync(join(process.cwd(), "local-ui/src/styles.css"), "utf8").replace(
  /\/\*[\s\S]*?\*\//g,
  "",
);
const headerSource = readFileSync(join(HERE, "BrowserHeader.tsx"), "utf8");

/**
 * Regressions for the tab strip and title bar.
 *
 * These are the seven defects reported together on 2026-08-16. Each one is
 * pinned by the smallest thing that would have caught it, which for the layout
 * items is the stylesheet: jsdom has no layout engine, so an overlap cannot be
 * measured here. The measurement that *can* be made lives outside this file, in
 * `chuzz-inspect`, which reads the real boxes back out of a running window;
 * these assertions stop the declaration that produced the right boxes from
 * being deleted.
 */

/** The block of `styles.css` a selector introduces, so a rule can be read. */
function ruleFor(selector: string): string {
  const start = styles.indexOf(`${selector} {`);
  expect(start, `${selector} is no longer in the stylesheet`).toBeGreaterThan(-1);
  return styles.slice(start, styles.indexOf("}", start));
}

describe("the title bar controls respond at all", () => {
  /**
   * Bug 4, and the reason the whole strip was inert: solid-layouts 0.1.2
   * dropped every plain-HTML prop a caller passed to a compiled component, so
   * `onClick` and `title` never reached the DOM. Settings could not be clicked,
   * new-tab did nothing, and no button in the window had a tooltip.
   *
   * Asserted against a library button rather than a chuzz one on purpose: the
   * defect was in the boundary between the two, and a chuzz component that
   * happened to place its own handler would pass while the boundary stayed
   * broken.
   */
  it("delivers a caller's onClick and title to a library button", async () => {
    const onClick = vi.fn();
    const { getByTitle } = render(() => (
      <Button title="Settings" onClick={onClick}>
        cog
      </Button>
    ));

    const button = getByTitle("Settings");
    expect(button.tagName).toBe("BUTTON");
    button.click();
    expect(onClick).toHaveBeenCalledOnce();
  });
});

describe("the tab indicator", () => {
  const statuses: LoadStatus[] = ["blank", "loading", "ready", "warning", "error"];

  /**
   * Bug 7. Five states, five distinguishable dots. The previous indicator had
   * two: a ternary that mapped `loading` to primary and *everything else* to
   * success, so a page that had failed to load wore the same green as one that
   * had worked.
   */
  it("gives each of the five states its own flavour", () => {
    const flavours = statuses.map((status) => {
      const { container, unmount } = render(() => (
        <BrowserTab
          id={1}
          title="Example"
          status={status}
          active
          closeLabel="Close tab"
          onClose={() => {}}
        />
      ));
      const dot = container.querySelector(".tab-dot");
      expect(dot, `no dot rendered for ${status}`).not.toBeNull();
      const flavour = [...(dot?.classList ?? [])].find((name) => name.startsWith("badge--flavor-"));
      expect(flavour, `${status} has no badge flavour`).toBeDefined();
      unmount();
      return flavour;
    });

    expect(new Set(flavours).size, `two states share a colour: ${flavours.join(", ")}`).toBe(
      statuses.length,
    );
  });

  /**
   * A dot, not a pill. The badge ships a min-width, a line-height and inline
   * padding because it is built to hold a count; the previous 7px square lost
   * to all three and rendered as a 15x22 capsule in the real window.
   */
  it("is sized square and round against the badge's own metrics", () => {
    const rule = ruleFor(".badge.tab-dot");
    for (const declaration of [
      "width: 7px",
      "min-width: 7px",
      "height: 7px",
      "min-height: 7px",
      "padding: 0",
      "line-height: 0",
      "border-radius: 9999px",
      // A badge anchors itself to a parent by default, which put the dot at the
      // far end of the pill instead of before the title.
      "position: static",
    ]) {
      expect(rule, `the dot needs ${declaration} to beat the badge's own metrics`).toContain(
        declaration,
      );
    }
  });

  /** The state has to be readable without colour vision. */
  it("names its state for a screen reader", () => {
    const { container } = render(() => (
      <BrowserTab
        id={1}
        title="Example"
        status="error"
        active
        closeLabel="Close tab"
        onClose={() => {}}
      />
    ));
    expect(container.querySelector(".tab-dot")?.getAttribute("aria-label")).toBe("error");
  });
});

describe("the tab title", () => {
  /**
   * Bug 3. Measured in the running window before the fix: the title's box ran
   * from x=146 to x=225 and the close button's from x=194 to x=233, so the last
   * third of every long title was drawn underneath the ×.
   *
   * `min-width: 0` is the whole fix. A flex item's automatic minimum is its
   * content, so `flex-grow: 1` grew the title and nothing ever shrank it;
   * `overflow: hidden` and `text-overflow: ellipsis` were already present and
   * had nothing to act on, because the box was never smaller than the text.
   */
  it("can shrink, so it truncates instead of running under the close button", () => {
    const rule = ruleFor(".tab-title");
    expect(rule, "without min-width:0 a flex item never shrinks below its text").toContain(
      "min-width: 0",
    );
    expect(rule).toContain("overflow: hidden");
    expect(rule).toContain("text-overflow: ellipsis");
  });

  /**
   * `aspect-ratio: 1` on a square button resolved against the component's own
   * 36px height rather than the 16px the tab sets, so the button came back
   * 38.88 wide and 17.28 tall and the × went straight back under the title.
   */
  it("does not let the square aspect ratio restore the close button's width", () => {
    expect(ruleFor(".tab-shell .button.tab-close")).toContain("aspect-ratio: auto");
  });

  /** The pill's right padding has to clear the close button, not sit under it. */
  it("reserves room for the close button in the pill's padding", () => {
    const closeWidth = Number(
      /width:\s*(\d+)px/.exec(ruleFor(".tab-shell .button.tab-close"))?.[1],
    );
    const closeInset = Number(
      /right:\s*(\d+)px/.exec(ruleFor(".tab-shell .button.tab-close"))?.[1],
    );
    const padding = Number(/padding:\s*0\s+(\d+)px/.exec(ruleFor(".tab-shell .tab"))?.[1]);

    expect(closeWidth).toBeGreaterThan(0);
    expect(
      padding,
      `${padding}px of padding cannot clear a ${closeWidth}px button`,
    ).toBeGreaterThan(closeWidth + closeInset);
  });

  /**
   * Both tab rules have to outrank the library's own, which styles the same
   * element as `.tabs__tab`. A bare `.tab` or `.button.tab-close` ties with it
   * on specificity and loses on source order, which is how the padding stayed
   * at 16px and the close button stayed 36px wide after the geometry here had
   * already been corrected once.
   */
  it("outranks the library's own rules for the same element", () => {
    expect(styles, "a bare .tab loses to .tabs__tab").not.toMatch(/^ {2}\.tab \{/m);
    expect(
      styles,
      "a bare .button.tab-close loses to .button--width-square.button--sm",
    ).not.toMatch(/^ {2}\.button\.tab-close \{/m);
  });

  /** A cut-short title still has to be readable. */
  it("carries the full title as a tooltip", () => {
    const { container } = render(() => (
      <BrowserTab
        id={1}
        title="A very long page title that will not fit"
        status="ready"
        active
        closeLabel="Close tab"
        onClose={() => {}}
      />
    ));
    expect(container.querySelector(".tab-title")?.getAttribute("title")).toBe(
      "A very long page title that will not fit",
    );
  });
});

describe("the title bar's round buttons", () => {
  /**
   * Bug 5. `size="sm"` on a Button is 36px, which squared is a 36px ring in a
   * 52px title bar holding 34px tabs. Measured at 38.88 device pixels under the
   * interface zoom before the fix.
   */
  it("are smaller than the tabs beside them", () => {
    const rule = ruleFor(".title-bar .button.button--width-square:not(.tab-close)");
    const size = Number(/width:\s*(\d+)px/.exec(rule)?.[1]);
    const tabHeight = Number(/height:\s*(\d+)px/.exec(ruleFor(".tab-shell .tab"))?.[1]);

    expect(size, "the title bar has to override the component's own 36px").toBeGreaterThan(0);
    expect(size, `a ${size}px control does not belong beside a ${tabHeight}px tab`).toBeLessThan(
      tabHeight,
    );
    expect(rule).toContain("min-width");
  });
});

describe("the inspector sections", () => {
  /**
   * A section that will not close is a section whose trigger does nothing.
   * `Collapsible` animates `grid-template-rows` between `1fr` and `0fr`, and
   * the body kept its full height in both states, so all five sections were
   * permanently open.
   */
  it("gives a closed section no height at all", () => {
    expect(
      styles,
      "a closed section has to be removed from layout, not animated to zero",
    ).toContain('.inspector-section .collapsible__content[data-expanded="false"]');
    expect(ruleFor('.inspector-section .collapsible__content[data-expanded="false"]')).toContain(
      "display: none",
    );
  });
});

describe("the title bar", () => {
  /**
   * Bug 6. The "N" was `<Avatar label="N" />`, a placeholder for an account
   * system this browser does not have. Asserted against the source rather than
   * a render, because the component it used has been deleted and the thing to
   * prevent is it coming back.
   */
  it("shows no account placeholder", () => {
    expect(headerSource, "the avatar placeholder is back").not.toMatch(/<Avatar/);
    expect(headerSource, 'the hardcoded "N" is back').not.toMatch(/label="N"/);
  });
});
