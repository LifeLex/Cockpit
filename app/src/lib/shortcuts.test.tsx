import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import {
  SHORTCUTS,
  shortcutById,
  comboFor,
  Kbd,
  type ShortcutId,
} from "./shortcuts";

describe("shortcut registry", () => {
  it("has a unique id per entry", () => {
    const ids = SHORTCUTS.map((s) => s.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("has a unique combo per entry (no accidental collisions)", () => {
    const combos = SHORTCUTS.map((s) => s.combo);
    expect(new Set(combos).size).toBe(combos.length);
  });
});

describe("shortcutById", () => {
  it("resolves a known id to its declaration", () => {
    const found = shortcutById("command-palette");
    expect(found).toBeDefined();
    // Real narrowing, not a cast.
    if (found === undefined) throw new Error("expected a shortcut");
    expect(found.combo).toBe("meta+k");
    expect(found.label).toBe("Command Palette");
  });

  it("returns undefined for an id not in the registry", () => {
    // Cast is confined to the test to feed a deliberately bogus id; the
    // production API only accepts ShortcutId.
    const bogus = "does-not-exist" as ShortcutId;
    expect(shortcutById(bogus)).toBeUndefined();
  });
});

describe("comboFor", () => {
  it("returns the combo string for a known id", () => {
    expect(comboFor("nav-settings")).toBe("meta+5");
    expect(comboFor("escape")).toBe("escape");
  });
});

describe("Kbd", () => {
  it("renders each combo token as a separate glyph", () => {
    // navigator.platform in jsdom is not Mac, so meta -> 'Ctrl'.
    render(<Kbd combo="meta+1" />);
    const kbd = screen.getByText("Ctrl").closest("kbd");
    expect(kbd).not.toBeNull();
    expect(kbd).toHaveTextContent("Ctrl");
    expect(kbd).toHaveTextContent("1");
  });

  it("renders single-key combos", () => {
    render(<Kbd combo="escape" />);
    expect(screen.getByText("Esc")).toBeInTheDocument();
  });
});
