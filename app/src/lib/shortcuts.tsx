import type { ReactNode } from "react";

/**
 * Stable identifier for a single shortcut, used to bind the registry entry to
 * a concrete handler in `App.tsx` and to look it up in the command palette.
 */
export type ShortcutId =
  | "command-palette"
  | "nav-prs"
  | "nav-projects"
  | "nav-skills"
  | "nav-agents"
  | "nav-settings"
  | "refresh"
  | "toggle-sidebar"
  | "open-in-ide"
  | "escape";

/**
 * A single shortcut declaration. This registry is the *source of truth* for
 * both the global keydown binding (via `combo`) and the on-screen key hint
 * (via `<Kbd>` reading the same `combo`).
 *
 * Handlers are intentionally *not* stored here: they depend on component state
 * (store actions, current view) and are wired in `App.tsx` keyed by `id`.
 */
export interface Shortcut {
  /** Stable identifier used to attach a handler. */
  readonly id: ShortcutId;
  /**
   * Normalized key combo, matching the format consumed by
   * `useKeyboardShortcuts` (e.g. `"meta+1"`, `"meta+comma"`, `"escape"`).
   */
  readonly combo: string;
  /** Human-readable label shown in menus and the command palette. */
  readonly label: string;
}

/**
 * The single shortcut registry. Both the keydown binding and every `<Kbd>`
 * hint derive from this list, so there is exactly one place to change a
 * shortcut.
 */
export const SHORTCUTS = [
  { id: "command-palette", combo: "meta+k", label: "Command Palette" },
  { id: "nav-prs", combo: "meta+1", label: "PRs" },
  { id: "nav-projects", combo: "meta+2", label: "Projects" },
  { id: "nav-skills", combo: "meta+3", label: "Skills" },
  { id: "nav-agents", combo: "meta+4", label: "Agents" },
  { id: "nav-settings", combo: "meta+5", label: "Settings" },
  { id: "refresh", combo: "meta+r", label: "Refresh" },
  { id: "toggle-sidebar", combo: "meta+b", label: "Toggle Sidebar" },
  { id: "open-in-ide", combo: "meta+o", label: "Open in IDE" },
  { id: "escape", combo: "escape", label: "Back" },
] as const satisfies readonly Shortcut[];

/** Look up a shortcut declaration by its id. */
export function shortcutById(id: ShortcutId): Shortcut | undefined {
  return SHORTCUTS.find((s) => s.id === id);
}

/** The combo string for a shortcut id, or `undefined` if unknown. */
export function comboFor(id: ShortcutId): string | undefined {
  return shortcutById(id)?.combo;
}

/** Detect macOS so the meta key renders as ⌘ rather than Ctrl. */
function isMac(): boolean {
  return navigator.platform.startsWith("Mac");
}

/** Render a single combo token (modifier or key) as a display glyph. */
function tokenGlyph(token: string): string {
  switch (token) {
    case "meta":
      return isMac() ? "⌘" : "Ctrl";
    case "shift":
      return "⇧";
    case "alt":
      return isMac() ? "⌥" : "Alt";
    case "comma":
      return ",";
    case "period":
      return ".";
    case "space":
      return "Space";
    case "escape":
      return "Esc";
    default:
      return token.length === 1 ? token.toUpperCase() : token;
  }
}

interface KbdProps {
  /**
   * A shortcut combo string (e.g. `"meta+1"`). Rendered as styled key caps.
   */
  readonly combo: string;
}

/**
 * Render a shortcut combo (e.g. `"meta+1"`) as a compact key-cap hint such as
 * `⌘1`. Reads the same combo format as the registry so hints never drift from
 * the actual binding.
 */
export function Kbd({ combo }: KbdProps): ReactNode {
  const glyphs = combo.split("+").map(tokenGlyph);
  return (
    <kbd className="ml-auto inline-flex items-center gap-0.5 rounded border border-border bg-muted px-1.5 py-0.5 font-mono text-[10px] leading-none text-muted-foreground">
      {glyphs.map((g, i) => (
        <span key={`${combo}-${String(i)}`}>{g}</span>
      ))}
    </kbd>
  );
}
