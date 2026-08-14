export type BrowserShortcut =
  | "new-tab"
  | "close-tab"
  | "previous-tab"
  | "next-tab"
  | "reload"
  | "focus-address"
  | "back"
  | "forward";

export interface ShortcutInput {
  key: string;
  code: string;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}

/** Keep the browser bindings in the Solid chrome, where both Tauri and mock builds use them. */
export function resolveBrowserShortcut(event: ShortcutInput): BrowserShortcut | undefined {
  if (event.altKey) return undefined;
  if (event.key === "F5" || event.code === "F5") return "reload";

  const key = event.key.toLowerCase();
  if (
    event.ctrlKey &&
    !event.metaKey &&
    !event.shiftKey &&
    (key === "t" || event.code === "KeyT")
  ) {
    return "new-tab";
  }
  if (!event.metaKey) return undefined;

  if (event.shiftKey) {
    if (key === "{" || event.code === "BracketLeft") return "previous-tab";
    if (key === "}" || event.code === "BracketRight") return "next-tab";
    return undefined;
  }

  if (key === "t" || event.code === "KeyT") return "new-tab";
  if (key === "w" || event.code === "KeyW") return "close-tab";
  if (key === "1" || event.code === "Digit1") return "previous-tab";
  if (key === "2" || event.code === "Digit2") return "next-tab";
  if (key === "r" || event.code === "KeyR") return "reload";
  if (key === "l" || key === "d" || event.code === "KeyL" || event.code === "KeyD") {
    return "focus-address";
  }
  if (key === "[" || event.code === "BracketLeft") return "back";
  if (key === "]" || event.code === "BracketRight") return "forward";
  return undefined;
}
