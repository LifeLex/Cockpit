import { useEffect } from "react";

/**
 * A map from normalized shortcut keys to handler functions.
 *
 * Key format uses `+` as separator: `"meta+k"`, `"meta+shift+r"`, `"escape"`.
 * Modifiers appear in a fixed order: meta, shift, alt.
 * The `meta` modifier maps to Cmd on macOS and Ctrl on Windows/Linux.
 */
type ShortcutMap = Readonly<Record<string, () => void>>;

/**
 * Normalize a keyboard event into a canonical shortcut string.
 *
 * Uses metaKey on macOS and ctrlKey on Windows/Linux so all shortcuts feel
 * native on both platforms. Returns lowercase keys in a fixed modifier order
 * (meta, shift, alt) to guarantee stable map lookups.
 */
function normalizeKeyEvent(e: KeyboardEvent): string {
  const parts: string[] = [];

  // On macOS, metaKey is Cmd; on Windows/Linux, ctrlKey is the equivalent.
  const isMac = navigator.platform.startsWith("Mac");
  const hasMeta = isMac ? e.metaKey : e.ctrlKey;

  if (hasMeta) parts.push("meta");
  if (e.shiftKey) parts.push("shift");
  if (e.altKey) parts.push("alt");

  // Map special key names to our canonical form.
  let key = e.key.toLowerCase();
  if (key === ",") key = "comma";
  if (key === ".") key = "period";
  if (key === " ") key = "space";

  parts.push(key);

  return parts.join("+");
}

/**
 * Register global keyboard shortcuts via a single `keydown` listener.
 *
 * Looks up the normalized key combo in the provided `ShortcutMap`. When a
 * match is found, the handler fires and `preventDefault` blocks the browser
 * default. Cleans up on unmount.
 *
 * Ignores events originating from text input elements to avoid hijacking
 * normal typing.
 */
export function useKeyboardShortcuts(handlers: ShortcutMap): void {
  useEffect(() => {
    function onKeyDown(e: KeyboardEvent): void {
      // Do not intercept shortcuts while the user is typing in an input,
      // textarea, or contenteditable element.
      const target = e.target;
      if (target instanceof HTMLElement) {
        const tag = target.tagName;
        if (
          tag === "INPUT" ||
          tag === "TEXTAREA" ||
          target.isContentEditable
        ) {
          // Allow Escape to still work inside inputs (e.g. to close dialogs).
          if (e.key !== "Escape") return;
        }
      }

      const combo = normalizeKeyEvent(e);
      const handler = handlers[combo];
      if (handler !== undefined) {
        e.preventDefault();
        handler();
      }
    }

    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [handlers]);
}

export type { ShortcutMap };
