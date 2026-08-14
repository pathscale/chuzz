import { describe, expect, it } from "vitest";
import { resolveBrowserShortcut, type ShortcutInput } from "./shortcuts";

const key = (patch: Partial<ShortcutInput>): ShortcutInput => ({
  key: "",
  code: "",
  metaKey: false,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  ...patch,
});

describe("browser shortcuts", () => {
  it("keeps the AgencyZero tab accelerators", () => {
    expect(resolveBrowserShortcut(key({ key: "t", code: "KeyT", metaKey: true }))).toBe("new-tab");
    expect(resolveBrowserShortcut(key({ key: "w", code: "KeyW", metaKey: true }))).toBe("close-tab");
    expect(resolveBrowserShortcut(key({ key: "1", code: "Digit1", metaKey: true }))).toBe(
      "previous-tab",
    );
  });

  it("supports browser navigation and address focus", () => {
    expect(resolveBrowserShortcut(key({ key: "l", code: "KeyL", metaKey: true }))).toBe(
      "focus-address",
    );
    expect(resolveBrowserShortcut(key({ key: "[", code: "BracketLeft", metaKey: true }))).toBe(
      "back",
    );
    expect(resolveBrowserShortcut(key({ key: "F5", code: "F5" }))).toBe("reload");
  });

  it("leaves unmodified and Alt chords to the page", () => {
    expect(resolveBrowserShortcut(key({ key: "t", code: "KeyT" }))).toBeUndefined();
    expect(
      resolveBrowserShortcut(key({ key: "t", code: "KeyT", metaKey: true, altKey: true })),
    ).toBeUndefined();
  });
});
